# Quick Start Guide

## Prerequisites

- PostgreSQL 17+
- PostGIS 3.x
- pgvector 0.7+
- Rust stable toolchain (x86_64 with AVX2+FMA, or aarch64/Apple Silicon with NEON)
- cargo-pgrx 0.16.1

## Installation

### 1. Install cargo-pgrx

```bash
cargo install cargo-pgrx --version 0.16.1 --locked
cargo pgrx init --pg17=/usr/bin/pg_config
```

### 2. Build and install geo_vec

```bash
cd pg_geo_vec

# x86_64 (Linux)
RUSTFLAGS="-C target-feature=+avx2,+fma" cargo pgrx install --release --no-default-features --features pg17

# aarch64 / Apple Silicon (macOS)
MACOSX_DEPLOYMENT_TARGET=$(sw_vers -productVersion) \
RUSTFLAGS="-C target-feature=+neon -C link-arg=-Wl,-undefined,dynamic_lookup" \
cargo pgrx install --release --no-default-features --features pg17
```

### 3. Create extensions

```sql
CREATE EXTENSION postgis;
CREATE EXTENSION vector;
CREATE EXTENSION geo_vec;
```

## Create a Table

```sql
CREATE TABLE places (
    id serial PRIMARY KEY,
    name text,
    embedding vector(384),
    geom geometry(Point, 4326)
);
```

## Create an Index

### Vector + Spatial (composite index)

```sql
CREATE INDEX ON places USING geo_vec (embedding vector_cosine_ops, geom geometry_geo_vec_ops);
```

This builds a DiskANN graph over the vectors and a spatial cell index over the geometries in a single structure.

### Vector only

```sql
CREATE INDEX ON places USING geo_vec (embedding vector_cosine_ops);
```

Equivalent to pgvectorscale's `USING diskann` — same DiskANN algorithm, same performance.

### Vector + Label filtering

```sql
CREATE INDEX ON places USING geo_vec (embedding vector_cosine_ops, categories vector_smallint_label_ops);
```

Where `categories` is a `smallint[]` column. Filters results by label overlap during graph traversal.

## Queries

### Spatial + Vector (one index scan)

```sql
SELECT id, name, embedding <=> query_vec AS dist
FROM places
WHERE geom && ST_MakeEnvelope(-122.5, 37.7, -122.4, 37.8, 4326)
ORDER BY embedding <=> query_vec
LIMIT 20;
```

The `&&` operator triggers spatial filtering within the geo_vec index. Prefer a composite `(embedding, geom)` index so one scan handles both predicates. When HNSW and GiST also exist, the planner chooses using geo_vec’s cardinality-aware costs — selective bboxes should prefer geo_vec (see [ARCHITECTURE](ARCHITECTURE.md#planner-cost-estimation)).

### Pure vector search

```sql
SELECT id, name, embedding <=> query_vec AS dist
FROM places
ORDER BY embedding <=> query_vec
LIMIT 20;
```

Without a spatial filter, geo_vec uses the DiskANN graph search path (same as pgvectorscale).

### Different distance metrics

```sql
-- Cosine distance (default)
ORDER BY embedding <=> query_vec

-- L2 (Euclidean) distance
ORDER BY embedding <-> query_vec

-- Inner product (negative)
ORDER BY embedding <#> query_vec
```

## Tuning

### Search quality

```sql
-- Increase search list for better recall (default: 100)
SET geo_vec.query_search_list_size = 200;

-- Increase rescore count for better precision (default: 50)
SET geo_vec.query_rescore = 100;
```

### Spatial routing threshold

```sql
-- Below this candidate count: brute-force (100% recall)
-- Above this: hybrid spatial-seeded graph (faster, ~95-100% recall)
SET geo_vec.spatial_brute_force_threshold = 5000;  -- default
```

### Index build options

```sql
CREATE INDEX ON places USING geo_vec (embedding vector_cosine_ops)
WITH (
    num_neighbors = 50,           -- graph connectivity (default: auto)
    search_list_size = 100,       -- build-time search list (default: 100)
    max_alpha = 1.2,              -- pruning parameter (default: 1.2)
    storage_layout = 'memory_optimized'  -- SBQ compression (default)
);
```

## Verify the Index

```sql
-- Check index exists
SELECT indexname, pg_size_pretty(pg_relation_size(indexname::regclass)) AS size
FROM pg_indexes WHERE tablename = 'places';

-- Check spatial cell index was built (look for NOTICE during CREATE INDEX)
-- "Built spatial cell index: N cells, M entries"

-- Force index scan for testing
SET enable_seqscan = off;
EXPLAIN (ANALYZE) SELECT ... ORDER BY embedding <=> ... LIMIT 20;

-- With competing indexes: which AM did the planner pick?
EXPLAIN (COSTS)
SELECT id FROM places
WHERE geom && ST_MakeEnvelope(...)
ORDER BY embedding <=> query_vec
LIMIT 20;
```

## Next Steps

- See [API.md](API.md) for the complete API reference
- See [ARCHITECTURE.md](ARCHITECTURE.md#planner-cost-estimation) for planner cardinality / `amcostestimate`
- See the `test_data/` directory for integration tests (`make bench-kigoto`, `make bench-planner-choice`)
