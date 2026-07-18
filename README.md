# geo_vec

[![CI](https://github.com/decision-labs/pg_geo_vec/actions/workflows/ci.yml/badge.svg)](https://github.com/decision-labs/pg_geo_vec/actions/workflows/ci.yml)

A PostgreSQL extension for **combined vector similarity search + spatial filtering** using a single composite index.

Built on the DiskANN graph algorithm (forked from [pgvectorscale](https://github.com/timescale/pgvectorscale)), `geo_vec` adds a spatial cell index that enables fast approximate nearest neighbor queries constrained to a geographic bounding box — all in one index scan.

## Key Features

- **Single composite index** for vector + geometry columns (no separate GiST index needed)
- **Three-way query routing**: brute-force for small regions, hybrid spatial-seeded graph for large regions, pure graph for vector-only
- **DiskANN graph** with SBQ compression for vector similarity
- **Spatial cell index** (CSR grid) for high spatial recall on the brute-force path (see [limitations](#limitations))
- **Label filtering** via `smallint[]` overlap operator
- **Coexists with pgvectorscale** — both extensions can be installed in the same database

## Requirements

- PostgreSQL 17+
- [PostGIS](https://postgis.net/)
- [pgvector](https://github.com/pgvector/pgvector) (for the `vector` type)
- Rust toolchain (x86_64 with AVX2+FMA, or aarch64/Apple Silicon with NEON)
- [cargo-pgrx](https://github.com/pgcentralfoundation/pgrx) 0.16.1

## Quick Start

```bash
# Build and install
cargo pgrx init --pg17=/usr/bin/pg_config

# x86_64 (Linux)
RUSTFLAGS="-C target-feature=+avx2,+fma" cargo pgrx install --release --no-default-features --features pg17

# aarch64 / Apple Silicon (macOS)
MACOSX_DEPLOYMENT_TARGET=$(sw_vers -productVersion) \
RUSTFLAGS="-C target-feature=+neon -C link-arg=-Wl,-undefined,dynamic_lookup" \
cargo pgrx install --release --no-default-features --features pg17
```

```sql
CREATE EXTENSION postgis;
CREATE EXTENSION vector;
CREATE EXTENSION geo_vec;

-- Create a composite index (vector + spatial)
CREATE INDEX ON places USING geo_vec (embedding vector_cosine_ops, geom);

-- Or with label filtering
CREATE INDEX ON places USING geo_vec (embedding vector_cosine_ops, labels);

-- All three: vector + spatial + labels
CREATE INDEX ON places USING geo_vec (embedding vector_cosine_ops, geom, labels);

-- Query: spatial filter + vector ANN in one index scan
SELECT id, embedding <=> query_vec AS dist
FROM places
WHERE geom && ST_MakeEnvelope(-122.5, 37.7, -122.4, 37.8, 4326)
ORDER BY embedding <=> query_vec
LIMIT 20;
```

See [docs/QUICKSTART.md](docs/QUICKSTART.md) for a full walkthrough and [docs/API.md](docs/API.md) for the complete API reference.

## How It Works

When you create a `geo_vec` index on `(embedding, geom)`, the build phase constructs:

1. A **DiskANN graph** over all vectors (with optional SBQ compression)
2. A **spatial cell index** — a grid-based auxiliary structure that maps geographic cells to graph nodes

At query time, `geo_vec` routes spatial+vector queries through three paths:

| Condition | Path | Recall | Speed |
|---|---|---|---|
| No bbox filter | Graph search | ~20/20 | Fastest |
| Small bbox (< threshold candidates) | Brute-force scan | 20/20 | Fast |
| Large bbox (>= threshold candidates) | Hybrid spatial-seeded graph | ~19-20/20 | Fast |

The threshold is controlled by `geo_vec.spatial_brute_force_threshold` (default: 5000).

## Benchmarks

Local PG17 run (`make bench-kigoto`): **318,375 rows**, 384-dim cosine, Recall@20 vs exact seqscan ground truth.

### Recall@20 — who actually finds the right neighbors?

```
                wide   medium  narrow  vector-only
                ────   ──────  ──────  ───────────
geo_vec         ████   ████    ████    ████         20  20  20  20
HNSW + GiST     ████   ░       ·       ████         20   1   0  20
DiskANN + GiST  ███░   ███·    ███░    ████         17  13  16  20

████ = 20/20   ███░ ≈ 15–19   ███· ≈ 10–14   ░ = 1–5   · = 0
```

On selective spatial filters, post-filtering ANN (HNSW/DiskANN + GiST) can look fine on latency while quietly returning the wrong set. `geo_vec` keeps full recall because spatial + vector share one index scan.

### Latency (EXPLAIN Analyze, ms) — Kigoto 318K

```
wide bbox (~182K candidates)
  geo_vec         ████████████████████████████████████████  97.6 ms   ★ 20/20
  HNSW+GiST       ████████████████                          39.5 ms     20/20
  DiskANN+GiST    █████████████████                         40.7 ms     17/20
  seqscan         █████████████████████████████████████     91.5 ms   (exact)

medium bbox (~70K candidates)
  geo_vec         █████████████████                         43.3 ms   ★ 20/20
  HNSW+GiST       ███████████████                           37.4 ms      1/20
  DiskANN+GiST    ████████████████                          40.3 ms     13/20

narrow bbox (~20K candidates)
  geo_vec         █████████████████                         43.2 ms   ★ 20/20
  HNSW+GiST       ███████████████                           38.3 ms      0/20
  DiskANN+GiST    ████████████████                          41.5 ms     16/20
  seqscan         ██████████████████████████████████        85.4 ms   (exact)

vector-only (no spatial filter)
  geo_vec         ██████████                                25.2 ms   ★ 20/20
  HNSW            ██████████                                25.6 ms     20/20
  DiskANN         ██████████                                25.9 ms     20/20
  seqscan         ███████████████████████████████████████   98.6 ms   (exact)
```

`★` = best recall at that query shape. HNSW/DiskANN win raw ms on some spatial queries, but on medium/narrow Kigoto boxes that speed is mostly empty calories.

Smaller fixture (`make bench-buildings`, 8.5K × 1024-d): all three approaches hit 20/20 on most cases; DiskANN+GiST dipped to 16–19/20 on wider boxes. Re-run anytime with the `make bench-*` targets.

### Planner cardinality (index choice)

When **geo_vec**, **HNSW**, and **GiST** all exist on the same table, Postgres picks a path using `amcostestimate`. geo_vec’s cost model uses spatial-qual selectivity (`clauselist_selectivity`), LIMIT, and the brute/hybrid/graph routing thresholds — not a flat `n/100` heuristic.

Measured on Kigoto with competing indexes on one table (`kigoto_planner`):

| Bbox | Before (`numIndexTuples = n/100`) | After (spatial + LIMIT-aware) |
|---|---|---|
| tiny | **geo_vec** (cost ~3335) | **geo_vec** (cost ~248) |
| narrow | HNSW (cost ~1413) | **geo_vec** (cost ~179) |
| medium | HNSW (cost ~963) | **geo_vec** (cost ~234) |
| wide | HNSW (cost ~850) | **geo_vec** (cost ~307) |

After the change, estimated geo_vec cost **rises with bbox width** (narrow → wide), and the planner prefers the rich index on selective queries where recall matters. Wide still picking geo_vec may be aggressive vs effective-QPS; treat as a tunable signal (issue [#4](https://github.com/decision-labs/pg_geo_vec/issues/4)).

Reproduce:

```bash
make bench-kigoto              # recall / latency (separate tables per AM)
make bench-planner-choice      # EXPLAIN choice with competing indexes
# optional: before/after amcostestimate A/B
bash test_data/run_planner_before_after.sh   # inside geo_vec_test:pg17 container
```

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#planner-cost-estimation) for how `amcostestimate` works.

## Limitations

### Incremental spatial index

Rows inserted after `CREATE INDEX` go into a spatial overflow segment. Scans union that segment with the base CSR, so new rows stay visible to brute-force and hybrid paths without `REINDEX`. `VACUUM`, or an insert that pushes overflow postings past `geo_vec.spatial_overflow_compact_threshold` (default 10000), rebuilds the CSR and clears overflow.

- Overflow is rewritten on each insert.
- Inserts outside the index extent clamp to boundary cells until compact expands the grid.
- `UPDATE` is delete + insert. Removed CSR slots stay until compact.

## pgvectorscale Coexistence

`geo_vec` can be installed alongside pgvectorscale in the same database. All SQL functions are prefixed with `geo_vec_` to avoid name collisions:

```sql
CREATE EXTENSION geo_vec;      -- access method: geo_vec
CREATE EXTENSION vectorscale;  -- access method: diskann

-- Both work on the same table
CREATE INDEX idx_geovec ON places USING geo_vec (embedding vector_cosine_ops, geom);
CREATE INDEX idx_diskann ON places USING diskann (embedding vector_cosine_ops);
```

## Documentation

- [Quick Start Guide](docs/QUICKSTART.md) — installation, setup, first queries
- [API Reference](docs/API.md) — operator classes, GUC parameters, helper functions
- [Architecture](docs/ARCHITECTURE.md) — internals, index build, query routing, planner costs, storage
- [Roadmap](docs/ROADMAP.md) — planned work and **what will move performance most**

## Development

```bash
# Set RUSTFLAGS for your architecture
# x86_64 (Linux):
export RUSTFLAGS="-C target-feature=+avx2,+fma"
# aarch64 / Apple Silicon (macOS):
export MACOSX_DEPLOYMENT_TARGET=$(sw_vers -productVersion)
export RUSTFLAGS="-C target-feature=+neon -C link-arg=-Wl,-undefined,dynamic_lookup"

# Build (debug) — default features already include pg17
cargo build

# Or explicitly:
cargo build --no-default-features --features pg17
cargo build --no-default-features --features pg18

# Build (release)
cargo pgrx install --release --no-default-features --features pg17
# PG18:
# cargo pgrx init --pg18=/usr/bin/pg_config
# cargo pgrx install --release --no-default-features --features pg18

# Integration tests (requires docker/podman) — PG17 and PG18
make test-pg17
make test-pg18
```

## License

This project is licensed under the [PostgreSQL License](LICENSE).

It includes substantial code derived from [pgvectorscale](https://github.com/timescale/pgvectorscale) by Timescale, Inc. See [NOTICE](NOTICE) for attribution.
