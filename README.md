# geo_vec

[![CI](https://github.com/decision-labs/pg_geo_vec/actions/workflows/ci.yml/badge.svg)](https://github.com/decision-labs/pg_geo_vec/actions/workflows/ci.yml)

A PostgreSQL extension for **combined vector similarity search + spatial filtering** using a single composite index.

Built on the DiskANN graph algorithm (forked from [pgvectorscale](https://github.com/timescale/pgvectorscale)), `geo_vec` adds a spatial cell index that enables fast approximate nearest neighbor queries constrained to a geographic bounding box — all in one index scan.

## Key Features

- **Single composite index** for vector + geometry columns (no separate GiST index needed)
- **Three-way query routing**: brute-force for small regions, hybrid spatial-seeded graph for large regions, pure graph for vector-only
- **DiskANN graph** with SBQ compression for vector similarity
- **Spatial cell index** (CSR grid) for 100% spatial recall on brute-force path
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
CREATE INDEX ON places USING geo_vec (embedding vector_cosine_ops, geom geometry_geo_vec_ops);

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

**318K rows, 384-dim vectors, cosine distance:**

| Query | Path | Recall | Latency |
|---|---|---|---|
| Tiny bbox (~1K candidates) | Brute-force | 20/20 | 65ms |
| Narrow bbox (~20K candidates) | Hybrid | 20/20 | 99ms |
| Wide bbox (~180K candidates) | Hybrid | 20/20 | 61ms |
| Pure vector | Graph | 20/20 | 37ms |
| Wide bbox (forced brute-force) | Brute-force | 20/20 | 499ms |

Hybrid search is **8x faster** than brute-force on large bounding boxes while maintaining the same recall.

## pgvectorscale Coexistence

`geo_vec` can be installed alongside pgvectorscale in the same database. All SQL functions are prefixed with `geo_vec_` to avoid name collisions:

```sql
CREATE EXTENSION geo_vec;      -- access method: geo_vec
CREATE EXTENSION vectorscale;  -- access method: diskann

-- Both work on the same table
CREATE INDEX idx_geovec ON places USING geo_vec (embedding vector_cosine_ops, geom geometry_geo_vec_ops);
CREATE INDEX idx_diskann ON places USING diskann (embedding vector_cosine_ops);
```

## Documentation

- [Quick Start Guide](docs/QUICKSTART.md) — installation, setup, first queries
- [API Reference](docs/API.md) — operator classes, GUC parameters, helper functions

## Development

```bash
# Set RUSTFLAGS for your architecture
# x86_64 (Linux):
export RUSTFLAGS="-C target-feature=+avx2,+fma"
# aarch64 / Apple Silicon (macOS):
export MACOSX_DEPLOYMENT_TARGET=$(sw_vers -productVersion)
export RUSTFLAGS="-C target-feature=+neon -C link-arg=-Wl,-undefined,dynamic_lookup"

# Build (debug)
cargo build --no-default-features --features pg17

# Build (release)
cargo pgrx install --release --no-default-features --features pg17

# Integration tests (requires podman/docker)
podman build -t geo_vec_test -f Containerfile.test .
podman run --rm -v $(pwd):/workspace geo_vec_test bash /workspace/test_data/ci_test.sh
```

## License

MIT OR Apache-2.0
