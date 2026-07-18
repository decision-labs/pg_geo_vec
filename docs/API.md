# API Reference

## Extension

```sql
CREATE EXTENSION geo_vec;
```

Requires: `postgis`, `vector` (pgvector).

The extension registers the `geo_vec` access method and associated operator classes.

## Access Method

```sql
CREATE INDEX ON <table> USING geo_vec (<columns>);
```

`geo_vec` is a custom PostgreSQL index access method based on the DiskANN graph algorithm with SBQ (scalar binary quantization) compression. It supports composite indexes spanning vector, geometry, and label columns.

## Operator Classes

### Vector distance (shared with pgvector/pgvectorscale)

| Operator Class | Type | Operator | Distance |
|---|---|---|---|
| `vector_cosine_ops` | `vector` | `<=>` | Cosine distance |
| `vector_l2_ops` | `vector` | `<->` | Euclidean (L2) distance |
| `vector_ip_ops` | `vector` | `<#>` | Negative inner product |

`vector_cosine_ops` is the default for `vector` type.

### Spatial filter (geo_vec only)

| Operator Class | Type | Operator | Meaning |
|---|---|---|---|
| `geometry_geo_vec_ops` | `geometry` | `&&` | Bounding box overlap |

### Label filter (geo_vec only)

| Operator Class | Type | Operator | Meaning |
|---|---|---|---|
| `vector_smallint_label_ops` | `smallint[]` | `&&` | Array overlap |

## Index Configurations

### Vector only

```sql
CREATE INDEX ON t USING geo_vec (embedding vector_cosine_ops);
```

### Vector + spatial

```sql
CREATE INDEX ON t USING geo_vec (embedding vector_cosine_ops, geom geometry_geo_vec_ops);
```

### Vector + label

```sql
CREATE INDEX ON t USING geo_vec (embedding vector_cosine_ops, labels vector_smallint_label_ops);
```

### Vector + spatial + label

```sql
CREATE INDEX ON t USING geo_vec (
    embedding vector_cosine_ops,
    geom geometry_geo_vec_ops,
    labels vector_smallint_label_ops
);
```

## Index Build Options (WITH clause)

| Parameter | Default | Description |
|---|---|---|
| `num_neighbors` | auto | Graph connectivity. Higher = better recall, larger index. Auto scales with dimensions. |
| `search_list_size` | 100 | Build-time search list size. Higher = better graph quality, slower build. |
| `max_alpha` | 1.2 | Pruning parameter. Controls graph sparsity. |
| `num_dimensions` | auto | Override vector dimensions (rarely needed). |
| `storage_layout` | `memory_optimized` | `memory_optimized` (SBQ compression) or `plain` (full f32 vectors). |

Example:

```sql
CREATE INDEX ON t USING geo_vec (embedding vector_cosine_ops)
WITH (num_neighbors = 50, search_list_size = 100, max_alpha = 1.2);
```

## GUC Parameters

### Query parameters (SET per session/transaction)

| Parameter | Default | Range | Description |
|---|---|---|---|
| `geo_vec.query_search_list_size` | 100 | 1-10000 | Search list size for queries. Higher = better recall, slower. |
| `geo_vec.query_rescore` | 50 | 0-1000 | Number of candidates rescored with exact distance. 0 disables rescoring. |
| `geo_vec.spatial_brute_force_threshold` | 5000 | 0-MAX | Candidate count below which brute-force is used instead of hybrid. |
| `geo_vec.spatial_overflow_compact_threshold` | 10000 | 1–MAX | Overflow postings that trigger CSR compact on insert; VACUUM also compacts when overflow is non-empty |
| `geo_vec.spatial_seeds_per_cell` | 2 | 1-100 | Seed nodes sampled per grid cell for hybrid spatial search. |

### Build parameters (superuser only)

| Parameter | Default | Range | Description |
|---|---|---|---|
| `geo_vec.min_vectors_for_parallel_build` | 65536 | 1-MAX | Minimum rows to enable parallel index build. |
| `geo_vec.force_parallel_workers` | -1 | -1-1024 | Force specific worker count. -1 = automatic. |
| `geo_vec.parallel_flush_interval` | 0.05 | 0.0-1.0 | Fraction of vectors processed before flushing neighbor cache. |
| `geo_vec.parallel_initial_start_nodes_count` | 1024 | 1-10000 | Nodes processed by lead worker before parallel workers start. |

### Tuning examples

```sql
-- Higher recall for important queries
SET geo_vec.query_search_list_size = 200;
SET geo_vec.query_rescore = 100;

-- Force brute-force for all spatial queries (100% recall, slower)
SET geo_vec.spatial_brute_force_threshold = 999999999;

-- Force hybrid for all spatial queries (faster, approximate)
SET geo_vec.spatial_brute_force_threshold = 0;
```

## Query Routing

When a query includes both a spatial filter (`&&`) and vector ordering (`ORDER BY ... <=> ...`), `geo_vec` automatically routes through one of three paths:

### 1. No spatial filter -> Graph search

Standard DiskANN greedy search. Same behavior as pgvectorscale.

### 2. Small bbox (candidates < threshold) -> Brute-force

Loads all matching nodes from the spatial cell index (base CSR plus overflow), computes exact distances, returns top-K. See [Limitations](../README.md#limitations).

### 3. Large bbox (candidates >= threshold) -> Hybrid spatial-seeded graph

Samples seed nodes from overlapping grid cells, feeds them as entry points into the DiskANN graph, applies spatial post-filtering during traversal. Much faster than brute-force for large regions.

> **Note:** Inserts after build stay visible through the overflow segment. `VACUUM` or `geo_vec.spatial_overflow_compact_threshold` folds overflow back into the CSR. See [Limitations](../README.md#limitations).

## SQL Helper Functions

These functions provide a two-index workflow (separate GiST + vector indexes) as an alternative to the composite index. Installed automatically when PostGIS is available.

### Bounding box + vector search

```sql
-- L2 distance
SELECT * FROM geo_vec_hybrid_bbox_l2(
    'places'::regclass,     -- table
    'id',                   -- id column
    'geom',                 -- geometry column
    'embedding',            -- vector column
    ST_MakeEnvelope(...),   -- bounding box
    query_vec,              -- query vector
    20,                     -- k (top-K results)
    2000                    -- candidate_limit
);

-- Cosine distance
SELECT * FROM geo_vec_hybrid_bbox_cosine(...);

-- Inner product
SELECT * FROM geo_vec_hybrid_bbox_ip(...);
```

### Radius + vector search

```sql
-- L2 distance
SELECT * FROM geo_vec_hybrid_dwithin_l2(
    'places'::regclass,     -- table
    'id',                   -- id column
    'geom',                 -- geometry column
    'embedding',            -- vector column
    ST_MakePoint(-122, 37), -- center point
    0.02,                   -- radius (in CRS units)
    query_vec,              -- query vector
    20,                     -- k
    2000                    -- candidate_limit
);

-- Cosine distance
SELECT * FROM geo_vec_hybrid_dwithin_cosine(...);

-- Inner product
SELECT * FROM geo_vec_hybrid_dwithin_ip(...);
```

Returns: `TABLE(row_id text, distance double precision)`

### Reinstall helpers

```sql
SELECT geo_vec_install_hybrid_api();
```

## Internal Functions

These functions are used internally by operator classes and should not be called directly:

| Function | Purpose |
|---|---|
| `geo_vec_amhandler(internal)` | Access method handler |
| `geo_vec_distance_type_cosine()` | Returns cosine distance type ID |
| `geo_vec_distance_type_l2()` | Returns L2 distance type ID |
| `geo_vec_distance_type_inner_product()` | Returns inner product distance type ID |
| `geo_vec_smallint_array_overlap(smallint[], smallint[])` | Array overlap for label filtering |

## Compatibility with pgvectorscale

`geo_vec` is a fork of pgvectorscale and uses the same DiskANN algorithm. Both can be installed in the same database:

```sql
CREATE EXTENSION geo_vec;      -- registers access method: geo_vec
CREATE EXTENSION vectorscale;  -- registers access method: diskann
```

All geo_vec SQL functions are prefixed with `geo_vec_` to avoid name collisions with pgvectorscale's `distance_type_cosine()`, `smallint_array_overlap()`, etc.

Operator classes (`vector_cosine_ops`, etc.) share names but are scoped per access method — `USING geo_vec` vs `USING diskann` — so they don't conflict.

## Coexistence with PostGIS GiST

PostGIS registers the `&&` (bounding box overlap) operator for geometry in the **GiST** access method (`gist_geometry_ops_2d`, strategy 3). geo_vec registers the same `&&` operator for geometry in the **geo_vec** access method (`geometry_geo_vec_ops`, strategy 6).

They don't conflict because PostgreSQL's operator class system is **scoped per access method**. For spatial+vector queries, the planner compares path costs. geo_vec’s `amcostestimate` uses spatial-qual selectivity, LIMIT, and brute/hybrid/graph work estimates so selective bboxes favor the rich index (see [ARCHITECTURE — Planner Cost Estimation](ARCHITECTURE.md#planner-cost-estimation) and [README benchmarks](../README.md#benchmarks)).

```sql
-- Both indexes on the same table
CREATE INDEX buildings_gist_idx ON buildings USING gist (geom);
CREATE INDEX buildings_hnsw_idx ON buildings USING hnsw (embedding vector_cosine_ops);
CREATE INDEX buildings_geo_vec_idx ON buildings USING geo_vec (embedding vector_cosine_ops, geom);

-- Pure spatial query → planner picks GiST (PostGIS)
SELECT id FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326);

-- Spatial + vector query → planner compares geo_vec vs HNSW(+GiST)
-- Selective bbox: expect Index Scan using …geo_vec…
EXPLAIN (COSTS)
SELECT id FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> query_vec
LIMIT 20;
```

The `&&` operator is the same PostGIS function in both cases — the routing is determined by which access method's operator class claims it in `pg_amop`, and which path wins on cost.