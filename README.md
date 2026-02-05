# pg_geo_vec

`pg_geo_vec` is a PostgreSQL extension that adds geospatial-aware behavior to graph-based vector indexing.

It uses the `geo_vec` access method and combines:
- vector ANN search (`ORDER BY embedding <->|<=> query_vec`)
- spatial pruning from PostGIS geometry bounding boxes (`geom && envelope`)

This is intended for queries like:
- "find nearest similar points inside this map window"
- "semantic nearest neighbors, but only in this region"

## What It Does

- Stores per-row geometry bounding boxes alongside vector nodes.
- Builds spatial partitions (uniform grid) from global geometry extent.
- Restricts graph start nodes to overlapping partitions at query time.
- Fails fast when the required PostGIS bbox helper API is unavailable.

## Requirements

- PostgreSQL 17+ (project currently validated on pg17)
- Rust toolchain
- `cargo-pgrx`
- PostGIS
- Vector type support (`vector` type/opclasses used in index examples)

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

Create a geometry opclass for spatial filtering (if not already created):

```sql
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_opclass c
        JOIN pg_am a ON a.oid = c.opcmethod
        WHERE c.opcname = 'geometry_geo_vec_ops'
          AND a.amname = 'geo_vec'
    ) THEN
        CREATE OPERATOR CLASS geometry_geo_vec_ops
        FOR TYPE geometry USING geo_vec AS
        OPERATOR 1 && (geometry, geometry);
    END IF;
END;
$$;
```

## Example

```sql
CREATE TABLE places (
    id bigserial PRIMARY KEY,
    embedding vector(3),
    geom geometry(Point, 4326)
);

CREATE INDEX idx_places_geo
ON places
USING geo_vec (embedding vector_l2_ops, geom geometry_geo_vec_ops)
WITH (
    num_neighbors = 50,
    search_list_size = 100,
    storage_layout = 'memory_optimized'
);

-- Spatial + vector query
SELECT id,
       embedding <-> '[0.1, 0.2, 0.3]'::vector(3) AS dist
FROM places
WHERE geom && ST_MakeEnvelope(-122.5, 37.5, -122.0, 38.0, 4326)
ORDER BY embedding <-> '[0.1, 0.2, 0.3]'::vector(3)
LIMIT 20;
```

## Index Options

The index supports standard DiskANN options used by this extension:

- `storage_layout`: `plain` or `memory_optimized` (alias for bq compression)
- `num_neighbors`: max graph out-degree (`-1` uses default)
- `search_list_size`: build/search candidate width
- `num_dimensions`: index first N dimensions (`0` = all)
- `num_bits_per_dimension`: compression bits per dimension
- `max_alpha`: pruning aggressiveness during build

## Spatial Behavior

At build time:
- The extension extracts geometry bboxes via PostGIS datum helper APIs.
- It computes a global bbox and derives a grid partitioner.
- Rows are assigned to partitions, and partition metadata is written to meta pages.

At query time:
- The `&&` geometry predicate bbox is read from scan keys.
- Overlapping partitions are selected.
- Search is restricted to start nodes from those partitions.

## Limitations

- Spatial pruning depends on a geometry bbox helper symbol provided by PostGIS at runtime.
- Current testing focus is pg17.
- `cargo pgrx test` may require write access to PostgreSQL extension install paths in your environment.

## Development

Useful commands:

```bash
RUSTFLAGS='-C target-feature=+avx2,+fma' cargo check --no-default-features --features pg17
RUSTFLAGS='-C target-feature=+avx2,+fma' cargo test --no-default-features --features pg17 --no-run
```

## Project Layout

- `src/access_method/`: AM build/scan/graph/meta-page logic
- `src/partition/`: bbox model, grid partitioning, PostGIS bbox extraction
- `sql/`: extension SQL definitions

## License

MIT OR Apache-2.0
