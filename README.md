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

The useful metric is not raw latency — it is **how fast you get the right neighbors**.

Local PG17 (`make bench-kigoto`): **318,375 rows**, 384-dim cosine. Recall@20 vs exact seqscan.  
**Effective QPS** = `(recall / 20) × (1000 / latency_ms)` — queries/sec weighted by recall. A method that returns in 40ms with 0/20 recall scores **0**.

### Effective QPS (speed × recall) — Kigoto spatial queries

```
medium bbox (~22% / 70K in-box)    narrow bbox (~6% / 20K in-box)
─────────────────────────────────  ─────────────────────────────────
geo_vec      ████████████████ 23.1  geo_vec      ████████████████ 23.1   (20/20 @ 43ms)
DiskANN+GiST ███████████░░░░ 16.1  DiskANN+GiST █████████████░░ 19.3   (13–16/20)
HNSW+GiST    █░░░░░░░░░░░░░░  1.3  HNSW+GiST    ···············  0.0   (1/20, 0/20)

wide bbox (~57% / 182K in-box)     vector-only (no spatial filter)
─────────────────────────────────  ─────────────────────────────────
HNSW+GiST    ████████████████ 25.3  geo_vec / HNSW / DiskANN ≈ 39 QPS  (all 20/20 @ ~25ms)
DiskANN+GiST █████████████░░ 20.9  seqscan                     ≈ 10 QPS  (exact @ ~99ms)
geo_vec      ██████░░░░░░░░░ 10.2
```

Reading: on **selective** boxes (medium ~22% / narrow ~6% of rows), post-filter ANN is “fast” in EXPLAIN but almost useless once recall is folded in. `geo_vec` wins the product metric because it keeps **20/20**. **Wide (~57% of rows)** is a weak filter — closer to unfiltered ANN — so HNSW can keep 20/20 and win on raw speed; that is expected, not a geo_vec failure.

### Speed bars — longer = faster, recall underneath (full bar = 100%)

Latency bars mislead (longer looks “better”). These bars are **speed** (`∝ 1/latency`), so longer is genuinely better. Absolute ms stay on the right.  
**Recall bar is fixed-width: 20 blocks = 100% (20/20).** geo_vec fills the bar on every query below.

```
medium bbox (~22% / 70K)          scale: longer speed bar = faster
  HNSW+GiST
    speed   ████████████████████····  37.4 ms
    recall  █···················   5% (1/20)   ← fast, wrong
  DiskANN+GiST
    speed   ██████████████████······  40.3 ms
    recall  █████████████·······  65% (13/20)
  geo_vec
    speed   █████████████████·······  43.3 ms
    recall  ████████████████████ 100% (20/20)  ← slightly slower, correct

narrow bbox (~6% / 20K)
  HNSW+GiST
    speed   ████████████████████····  38.3 ms
    recall  ····················   0% (0/20)   ← fastest, empty result
  DiskANN+GiST
    speed   ██████████████████······  41.5 ms
    recall  ████████████████····  80% (16/20)
  geo_vec
    speed   █████████████████·······  43.2 ms
    recall  ████████████████████ 100% (20/20)

wide bbox (~57% / 182K) — weak filter; closer to unfiltered ANN
  HNSW+GiST
    speed   ████████████████████····  39.5 ms
    recall  ████████████████████ 100% (20/20)  ← legit win: fast + correct
  DiskANN+GiST
    speed   ███████████████████·····  40.7 ms
    recall  █████████████████···  85% (17/20)
  geo_vec
    speed   ████████················  97.6 ms
    recall  ████████████████████ 100% (20/20)

vector-only (no spatial filter)
  geo_vec
    speed   ████████████████████····  25.2 ms
    recall  ████████████████████ 100% (20/20)
  HNSW
    speed   ████████████████████····  25.6 ms
    recall  ████████████████████ 100% (20/20)
  DiskANN
    speed   ████████████████████····  25.9 ms
    recall  ████████████████████ 100% (20/20)
  seqscan
    speed   █████···················  98.6 ms
    recall  ████████████████████ 100% (exact)
```

### Same data as a recall × latency map

```
Recall
  100% ┤
       │
       │   H(w)                     G(m)  G(n)                    G(w)
       │
       │
   85% ┤        D(w)
       │
       │
   80% ┤              D(n)
       │
       │
   65% ┤                    D(m)
       │
       │
   50% ┤
       │
       │
   25% ┤
       │
       │
    5% ┤     H(m)
       │
       │
    0% ┤           H(n)
       │
       └──────────────────────────────────────────────────────► latency (ms)
           25          40           60           80          100

  G = geo_vec   H = HNSW+GiST   D = DiskANN+GiST
  (w) wide≈57%  (m) medium≈22%  (n) narrow≈6%   of 318K rows

  Y = Recall@20 as percent (20/20 = 100%). geo_vec sits on the 100% line for all three bboxes.
  Ideal corner = top-left (100% recall, low latency).
  Bottom-left  = fast and wrong.  Top-right = correct but slow.
```

### Raw numbers (transparency)

| Query | Method | Latency | Recall@20 | Effective QPS |
|-------|--------|--------:|----------:|--------------:|
| medium | **geo_vec** | 43.3 ms | **20/20** | **23.1** |
| medium | DiskANN+GiST | 40.3 ms | 13/20 | 16.1 |
| medium | HNSW+GiST | 37.4 ms | 1/20 | 1.3 |
| narrow | **geo_vec** | 43.2 ms | **20/20** | **23.1** |
| narrow | DiskANN+GiST | 41.5 ms | 16/20 | 19.3 |
| narrow | HNSW+GiST | 38.3 ms | 0/20 | 0.0 |
| wide | HNSW+GiST | 39.5 ms | 20/20 | 25.3 |
| wide | DiskANN+GiST | 40.7 ms | 17/20 | 20.9 |
| wide | geo_vec | 97.6 ms | 20/20 | 10.2 |

Smaller fixture (`make bench-buildings`, 8.5K × 1024-d): recall stays high for all methods, so the story collapses back toward plain latency. The failure mode shows up at scale + selective filters — re-run with `make bench-kigoto`.

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
