# Architecture

## Overview

`geo_vec` is a PostgreSQL access method extension that combines vector similarity search (DiskANN graph) with spatial filtering (grid-based cell index) and label filtering in a single composite index.

It is a fork of [pgvectorscale](https://github.com/timescale/pgvectorscale), adding the spatial cell index, label filtering, and three-way query routing.

```
                        ┌─────────────────────────┐
                        │      SQL Query           │
                        │  ORDER BY emb <=> q      │
                        │  WHERE geom && bbox      │
                        │    AND labels && '{2}'    │
                        └────────────┬────────────┘
                                     │
                        ┌────────────▼────────────┐
                        │   PostgreSQL Executor    │
                        │   (Index Scan node)      │
                        └────────────┬────────────┘
                                     │
                ┌────────────────────▼────────────────────┐
                │          geo_vec Access Method           │
                │         (amhandler / scan.rs)            │
                ├─────────────────────────────────────────┤
                │                                         │
                │  ┌──────────┐ ┌────────┐ ┌───────────┐ │
                │  │  Graph   │ │Spatial │ │  Label    │ │
                │  │ (DiskANN)│ │Cell Idx│ │ Filter    │ │
                │  └─────┬────┘ └───┬────┘ └─────┬─────┘ │
                │        │          │             │       │
                │  ┌─────▼──────────▼─────────────▼─────┐ │
                │  │       Storage Layer                │ │
                │  │   Plain (f32)  |  SBQ (1-2 bit)    │ │
                │  └────────────────────────────────────┘ │
                │                                         │
                │  ┌────────────────────────────────────┐ │
                │  │   Page / Buffer Utilities          │ │
                │  │   Tape, ChainTape, LRU cache       │ │
                │  └────────────────────────────────────┘ │
                └──────────────────┬──────────────────────┘
                                   │
                     ┌─────────────▼──────────────┐
                     │  PostgreSQL Shared Buffers  │
                     │  (8 KB pages on disk)       │
                     └────────────────────────────┘
```

## Module Map

```
src/
├── lib.rs                    ← Extension entry (_PG_init, distance init, GUC registration)
│
├── access_method/            ← Core index implementation
│   ├── mod.rs                ← amhandler(), operator classes (extension_sql!)
│   ├── build.rs              ← Index build: heap scan → graph insert → spatial finalization
│   ├── build/parallel.rs     ← Parallel build coordination (PG17+)
│   ├── scan.rs               ← Query: three-way routing, amrescan/amgettuple
│   ├── meta_page.rs          ← MetaPage: index metadata (dimensions, flags, pointers)
│   ├── spatial_index.rs      ← SpatialCellIndex: CSR grid, seed sampling, bbox lookup
│   ├── storage.rs            ← Storage trait (abstraction over Plain / SBQ)
│   ├── storage_common.rs     ← Shared helpers (attribute access)
│   │
│   ├── graph/
│   │   ├── mod.rs            ← DiskANN graph: greedy search, robust prune
│   │   ├── neighbor_store.rs ← In-memory neighbor cache during build
│   │   ├── neighbor_with_distance.rs
│   │   └── start_nodes.rs    ← Entry points for graph search
│   │
│   ├── plain/                ← Full-precision storage
│   │   ├── storage.rs        ← PlainStorage: read/write f32 vectors
│   │   ├── node.rs           ← PlainNode: vector + bbox + labels + neighbors
│   │   └── tests.rs
│   │
│   ├── sbq/                  ← Scalar Binary Quantization storage
│   │   ├── storage.rs        ← SbqSpeedupStorage: quantized search + rescore
│   │   ├── node.rs           ← SbqNode: quantized vector + metadata
│   │   ├── quantize.rs       ← SbqQuantizer: online mean/variance training
│   │   ├── cache.rs          ← Quantizer cache
│   │   └── tests.rs
│   │
│   ├── labels/
│   │   ├── mod.rs            ← LabelSet, LabeledVector (sorted smallint sets)
│   │   └── filtering_tests.rs
│   │
│   ├── distance/
│   │   ├── mod.rs            ← DistanceType enum, dispatch
│   │   ├── distance_x86.rs   ← AVX2 + FMA SIMD
│   │   └── distance_aarch64.rs ← NEON SIMD
│   │
│   ├── options.rs            ← TSVIndexOptions (WITH clause parsing)
│   ├── guc.rs                ← GUC parameters (search_list_size, brute_force_threshold, ...)
│   ├── type_utils.rs         ← geometry_type_oid(), is_geometry_column()
│   ├── pg_vector.rs          ← PgVector: wrapper for pgvector's vector type
│   ├── node.rs               ← ReadableNode / WriteableNode traits
│   ├── cost_estimate.rs      ← amcostestimate() for planner
│   ├── vacuum.rs             ← ambulkdelete(), amvacuumcleanup()
│   ├── stats.rs              ← Internal statistics
│   └── debugging.rs
│
├── partition/
│   ├── mod.rs                ← BBox2D (spatial bounding box arithmetic)
│   └── postgis.rs            ← GSERIALIZED bbox extraction, dlsym PostGIS loading
│
└── util/
    ├── mod.rs                ← ItemPointer (BlockNumber, OffsetNumber)
    ├── page.rs               ← PageType enum, ReadablePage / WritablePage
    ├── tape.rs               ← Tape: single-page item allocator
    ├── chain.rs              ← ChainTapeWriter: multi-page large items
    ├── buffer.rs             ← PostgreSQL buffer lock wrappers
    ├── lru.rs                ← LRU cache (neighbor cache during build)
    ├── table_slot.rs         ← TableSlot: heap tuple reader (for rescore)
    └── ports.rs              ← FFI wrappers for PG C functions
```

## Index Build

```
CREATE INDEX ... USING geo_vec (embedding vector_cosine_ops,
                                geom geometry_geo_vec_ops,
                                labels vector_smallint_label_ops);

ambuild()
    │
    ├─ Create MetaPage (detect geometry column, set flags)
    ├─ If geometry: ensure_postgis_loaded() (dlsym force-load .so)
    ├─ Initialize Storage (Plain or SBQ) + Graph
    │
    ▼
Heap Scan  ─── for each row ───────────────────────────────┐
    │                                                       │
    │  Extract vector ──── LabeledVector::from_datums()     │
    │  Extract bbox   ──── postgis_extract_bbox(geom)       │
    │  Extract labels ──── parse smallint[]                 │
    │                                                       │
    │  Storage::create_node(vector, bbox, labels, heap_ptr) │
    │      → Write PlainNode or SbqNode to Tape page        │
    │      → Returns IndexPointer                           │
    │                                                       │
    │  Graph::greedy_search() → find neighbors              │
    │  Graph::robust_prune() → select best edges            │
    │                                                       │
    │  Accumulate (IndexPointer, BBox2D) for spatial index  │
    └───────────────────────────────────────────────────────┘
    │
    ▼
finalize_build()
    │
    ├─ Write neighbor lists to nodes
    ├─ If geometry:
    │     SpatialCellIndexBuilder → CSR grid
    │     ChainTapeWriter → write to pages
    │     MetaPage.spatial_cell_index_start = pointer
    └─ Store MetaPage to block 0
```

### Parallel Build (PG17+)

For large tables, PG17's parallel index build is used. The leader coordinates workers via shared state and condition variables. Workers wait until start nodes are initialized (first N tuples processed sequentially), then build the graph concurrently.

## Three-Way Query Routing

The scan path selects a strategy based on the query predicates and the estimated number of spatial candidates:

```
amrescan(query_vector, query_bbox?, query_labels?)
    │
    ▼
Has bbox AND spatial cell index exists?
    │
    ├── NO ──────────────────────────────────────┐
    │                                             ▼
    │                                   ┌─────────────────┐
    │                                   │   GRAPH SEARCH   │
    │                                   │  Standard DiskANN │
    │                                   │  greedy search    │
    │                                   │  + inline label   │
    │                                   │    post-filter    │
    │                                   └─────────────────┘
    │
    ├── YES
    │     │
    │     ▼
    │   Count candidates in overlapping cells
    │     │
    │     ├── candidates <= threshold (default 5000)
    │     │         │
    │     │         ▼
    │     │   ┌─────────────────────┐
    │     │   │   BRUTE-FORCE SCAN   │
    │     │   │  Load all nodes from  │
    │     │   │  overlapping cells    │
    │     │   │  Read vectors         │
    │     │   │  Compute distances    │
    │     │   │  Sort → top-K         │
    │     │   │  (100% spatial recall)│
    │     │   └─────────────────────┘
    │     │
    │     └── candidates > threshold
    │               │
    │               ▼
    │         ┌──────────────────────┐
    │         │  HYBRID SEARCH        │
    │         │  sample_seeds_in_bbox │
    │         │  → seed entry points  │
    │         │  → graph traversal    │
    │         │  + spatial post-filter│
    │         │  + label post-filter  │
    │         │  O(search_list) cost  │
    │         └──────────────────────┘
```

### Why Three Paths?

| Path | When | Cost | Recall |
|---|---|---|---|
| **Brute-force** | Small bbox (few candidates) | O(candidates) | 100% |
| **Hybrid** | Large bbox (many candidates) | O(search_list_size) | ~95-100% |
| **Graph** | No bbox | O(search_list_size) | ~95-100% |

Brute-force is exact but expensive for large regions. Hybrid seeds the DiskANN graph with spatially-relevant entry points, achieving near-perfect recall at fixed cost. The threshold (GUC `geo_vec.spatial_brute_force_threshold`, default 5000) controls the crossover.

## Spatial Cell Index

A grid-based auxiliary structure stored in CSR (Compressed Sparse Row) format alongside the DiskANN graph.

```
Grid over data extent
┌───┬───┬───┬───┐
│ 0 │ 1 │ 2 │ 3 │     CSR Arrays:
├───┼───┼───┼───┤
│ 4 │ 5 │ 6 │ 7 │     cell_offsets: [0, 12, 25, 38, ...]  (num_cells + 1)
├───┼───┼───┼───┤     node_pointers: [ptr_a, ptr_b, ...]   (all nodes, grouped by cell)
│ 8 │ 9 │10 │11 │
└───┴───┴───┴───┘     Nodes in cell i: node_pointers[cell_offsets[i]..cell_offsets[i+1]]
```

- Auto-sized to ~100 points per cell
- Nodes assigned to cells by geometry centroid
- `nodes_in_bbox()`: union of all nodes in overlapping grid cells
- `sample_seeds_in_bbox()`: evenly-spaced samples from overlapping cells (for hybrid search entry points)
- Serialized to chained pages via `ChainTapeWriter`

## Storage Layer

Two storage backends, selected at index creation (default: SBQ).

```
┌──────────────────────────────────────────────────┐
│                 Storage Trait                     │
│  create_node()         get_query_distance_measure│
│  visit_lsn()           get_full_distance_for_resort│
│  return_lsn() → (HeapPointer, BBox2D)           │
├────────────────────┬─────────────────────────────┤
│   PlainStorage     │   SbqSpeedupStorage         │
│                    │                              │
│  PlainNode:        │  SbqNode:                   │
│  ┌──────────────┐  │  ┌─────────────────────┐    │
│  │ f32[] vector  │  │  │ u64[] quantized vec  │    │
│  │ BBox2D        │  │  │ BBox2D               │    │
│  │ labels[]      │  │  │ labels[]             │    │
│  │ neighbors[]   │  │  │ neighbors[]          │    │
│  │ heap_pointer  │  │  │ heap_pointer         │    │
│  └──────────────┘  │  └─────────────────────┘    │
│                    │                              │
│  Distance:         │  Distance:                   │
│  f32-to-f32        │  XOR Hamming (approximate)   │
│  (exact)           │  + rescore from heap (exact)  │
└────────────────────┴─────────────────────────────┘
```

**SBQ (Scalar Binary Quantization)** compresses each vector dimension to 1-2 bits using per-dimension mean/variance thresholds. Approximate distances are computed via SIMD XOR + popcount on quantized representations. The top-K candidates are then rescored using full-precision vectors fetched from the heap.

## DiskANN Graph

The graph module implements the DiskANN algorithm:

- **Build**: Incremental insertion with `greedy_search` (find neighbors) + `robust_prune` (select best edges, respecting max degree R)
- **Search**: `greedy_search_streaming()` with a best-first candidate queue, expanding neighbors until `search_list_size` candidates visited
- **Hybrid entry**: `greedy_search_streaming_init_with_seeds()` accepts custom seed nodes from the spatial cell index instead of using fixed start nodes

The graph is fully connected (no partitioning) to maintain high recall for pure vector queries.

## PostGIS Integration

`geo_vec` reads bounding boxes from PostGIS `geometry` values without a link-time dependency:

```
postgis_extract_bbox(geom_datum)
    │
    ▼
Read GSERIALIZED header bytes
    │
    ├── gflags bit 2 set (BBOX cached)?
    │       │
    │       ├── YES → read 4 x f32 directly from header → BBox2D
    │       │         (no PostGIS function call, fast path)
    │       │
    │       └── NO → dlsym("LWGEOM_to_BOX2DF")
    │                 → DirectFunctionCall1Coll → BBox2D
    │
    └── ensure_postgis_loaded() called once at build start
          → SPI query pg_extension → load_file(postgis .so path)
```

The geometry column is identified by `format_type_be()` returning `"geometry"`, and uses operator strategy 6 for the `&&` operator in the access method.

## Label Filtering

Labels are stored as sorted `smallint[]` arrays (type `LabelSet`). Filtering uses the `&&` (overlap) operator:

- **Build**: Labels extracted from index datums, stored in each node (PlainNode/SbqNode)
- **Scan**: Query labels parsed from scan key (strategy 1). During graph traversal, nodes whose labels don't overlap the query labels are skipped.
- **Operator**: `geo_vec_smallint_array_overlap()` implements `&&` for `smallint[]`, registered in the `vector_smallint_label_ops` operator class.

## Page Layout

All data is stored in standard 8 KB PostgreSQL pages, managed through the shared buffer pool:

| PageType | Contents | Written via |
|---|---|---|
| `Meta` | MetaPage (index metadata) | ChainTape (block 0) |
| `Node` | PlainNode (full-precision vector nodes) | Tape |
| `SbqNode` | SbqNode (quantized vector nodes) | Tape |
| `SbqMeans` | SbqQuantizer parameters | ChainTape |
| `SpatialCellIndex` | CSR grid arrays | ChainTape |

**Tape**: Allocates items sequentially in pages; each item must fit in a single page.
**ChainTape**: Links pages for large items that span multiple pages (MetaPage, quantizer, spatial index).
