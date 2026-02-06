# pg_geo_vec

`pg_geo_vec` is a PostgreSQL extension that provides a vector ANN index access method (`geo_vec`).

Current architecture is intentionally split:
- vector similarity search: `geo_vec` (DiskANN-style graph index)
- geospatial filtering: PostGIS index on `geometry` (typically `GiST`)

This keeps each concern on its strongest native path and avoids fragile mixed opclass behavior.

## Requirements

- PostgreSQL 17+
- Rust toolchain
- `cargo-pgrx`
- PostGIS
- `vector` type support (for embedding columns)

## Build And Install

```bash
cargo pgrx init --pg17 /path/to/pg_config
cargo pgrx install --features pg17 --no-default-features
```

## SQL Setup

```sql
CREATE EXTENSION IF NOT EXISTS postgis;
CREATE EXTENSION IF NOT EXISTS geo_vec;
```

## Recommended Index Strategy

Use two indexes:

1. Spatial index (PostGIS)
```sql
CREATE INDEX idx_places_geom_gist
ON places
USING gist (geom);
```

2. Vector index (`geo_vec`)
```sql
CREATE INDEX idx_places_embedding_geo_vec
ON places
USING geo_vec (embedding vector_l2_ops)
WITH (
  num_neighbors = 50,
  search_list_size = 100,
  storage_layout = 'memory_optimized'
);
```

## Query Pattern

For hybrid search, prefilter spatially, then rank by vector distance:

```sql
WITH spatial AS MATERIALIZED (
  SELECT id, embedding
  FROM places
  WHERE geom && ST_MakeEnvelope(-122.5, 37.5, -122.0, 38.0, 4326)
)
SELECT id,
       embedding <-> '[0.1,0.2,0.3]'::vector(3) AS dist
FROM spatial
ORDER BY embedding <-> '[0.1,0.2,0.3]'::vector(3)
LIMIT 20;
```

Notes:
- Use `&&` for fast bbox pruning.
- For exact geometry semantics, add `ST_Intersects(...)` (or other exact predicate).
- If the spatial window is large, consider an extra coarse cap in the CTE before vector ranking.

## What `geo_vec` Supports

- Vector opclasses: cosine, L2, inner product
- Optional label-aware vector indexing (`smallint[]` label opclass)
- Build/query tuning via `geo_vec.*` GUCs

## Current Limitations

- `geo_vec` does not currently provide a production-ready single composite geometry+vector opclass path.
- Some legacy geo-related code/tests remain from prior composite-index experiments and are being cleaned up.
- Runtime `cargo pgrx test` behavior depends on local install/schema tooling setup.

## Development

```bash
RUSTFLAGS='-C target-feature=+avx2,+fma' cargo check --no-default-features --features pg17
RUSTFLAGS='-C target-feature=+avx2,+fma' cargo test --no-default-features --features pg17 --no-run
```

## Project Layout

- `src/access_method/`: vector access method build/scan/graph/meta-page logic
- `src/partition/`: spatial helper types and PostGIS helper integration (currently not the primary runtime path)
- `sql/`: extension SQL definitions

## License

MIT OR Apache-2.0
