# Architecture

## Overview

`geo_vec` is a PostgreSQL access method extension that combines vector similarity search (DiskANN graph) with spatial filtering (grid-based cell index) and label filtering in a single composite index.

It is a fork of [pgvectorscale](https://github.com/timescale/pgvectorscale), adding the spatial cell index, label filtering, and three-way query routing.

```mermaid
flowchart TB
    subgraph query [SQL Query]
        Q["ORDER BY emb &lt;=&gt; q\nWHERE geom && bbox\nAND labels && '{2}'"]
    end

    query --> Executor["PostgreSQL Executor\n(Index Scan node)"]
    Executor --> AM

    subgraph AM ["geo_vec Access Method (amhandler / scan.rs)"]
        direction TB
        subgraph components [Components]
            direction LR
            Graph["Graph\n(DiskANN)"]
            SpatialIdx["Spatial\nCell Index"]
            LabelFilter["Label\nFilter"]
        end
        components --> Storage["Storage Layer\nPlain (f32) | SBQ (1-2 bit)"]
        Storage --> Pages["Page / Buffer Utilities\nTape, ChainTape, LRU cache"]
    end

    AM --> Buffers["PostgreSQL Shared Buffers\n(8 KB pages on disk)"]
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
│   ├── cost_estimate.rs      ← amcostestimate(): spatial sel + LIMIT + brute/hybrid/graph work
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

```mermaid
flowchart TD
    CreateIdx["CREATE INDEX ... USING geo_vec"] --> ambuild
    ambuild --> MetaPage["Create MetaPage\n(detect geometry column, set flags)"]
    MetaPage --> PostGIS{"Has geometry?"}
    PostGIS -->|Yes| LoadPostGIS["ensure_postgis_loaded()\n(dlsym force-load .so)"]
    PostGIS -->|No| InitStorage
    LoadPostGIS --> InitStorage["Initialize Storage (Plain or SBQ) + Graph"]

    InitStorage --> HeapScan

    subgraph HeapScan ["Heap Scan (for each row)"]
        direction TB
        Extract["Extract vector, bbox, labels\nfrom row datums"]
        Extract --> CreateNode["Storage::create_node()\nWrite node to Tape page\nReturns IndexPointer"]
        CreateNode --> GraphInsert["Graph::greedy_search()\nGraph::robust_prune()\nSelect best edges"]
        GraphInsert --> Accumulate["Accumulate\n(IndexPointer, BBox2D)\nfor spatial index"]
    end

    HeapScan --> Finalize

    subgraph Finalize ["finalize_build()"]
        direction TB
        WriteNeighbors["Write neighbor lists to nodes"]
        WriteNeighbors --> BuildSpatial{"Has geometry?"}
        BuildSpatial -->|Yes| CSR["SpatialCellIndexBuilder\nMulti-cell assignment\nCSR grid -> ChainTapeWriter"]
        BuildSpatial -->|No| StoreMeta
        CSR --> StoreMeta["Store MetaPage to block 0"]
    end
```

### Parallel Build (PG17+)

For large tables, PG17's parallel index build is used. The leader coordinates workers via shared state and condition variables. Workers wait until start nodes are initialized (first N tuples processed sequentially), then build the graph concurrently.

## Three-Way Query Routing

The scan path selects a strategy based on the query predicates and the estimated number of spatial candidates:

```mermaid
flowchart TD
    amrescan["amrescan(query_vector, query_bbox?, query_labels?)"]
    amrescan --> HasBBox{"Has bbox AND\nspatial cell index?"}

    HasBBox -->|No| GraphSearch["GRAPH SEARCH\nStandard DiskANN greedy search\n+ inline label post-filter"]

    HasBBox -->|Yes| CountCandidates["Count candidates\nin overlapping cells"]

    CountCandidates --> ThresholdCheck{"candidates <= threshold?\n(default 5000)"}

    ThresholdCheck -->|Yes| BruteForce["BRUTE-FORCE SCAN\nLoad all nodes from overlapping cells\nDeduplicate (multi-cell)\nExact bbox filter\nCompute exact vector distances\nSort -> top-K\n(100% spatial recall)"]

    ThresholdCheck -->|No| Hybrid["HYBRID SEARCH\nsample_seeds_in_bbox()\nSeed DiskANN graph entry points\nGraph traversal\n+ spatial post-filter\n+ label post-filter\nO(search_list_size) cost"]
```

### Why Three Paths?

| Path | When | Cost | Recall |
|---|---|---|---|
| **Brute-force** | Small bbox (few candidates) | O(candidates) | 100% |
| **Hybrid** | Large bbox (many candidates) | O(search_list_size) | ~95-100% |
| **Graph** | No bbox | O(search_list_size) | ~95-100% |

Brute-force is exact but expensive for large regions. Hybrid seeds the DiskANN graph with spatially-relevant entry points, achieving near-perfect recall at fixed cost. The threshold (GUC `geo_vec.spatial_brute_force_threshold`, default 5000) controls the crossover.

## Incremental CSR (inserts after build)

After `CREATE INDEX`, the base CSR blob is not rewritten on every insert. Instead:

1. **INSERT** — node is written to the DiskANN graph as before; its `(cell_id, IndexPointer)` postings are appended to a `SpatialOverflowIndex` ChainTape (`PageType::SpatialOverflow`), referenced from MetaPage.
2. **Scan** — `nodes_in_bbox` / `sample_seeds_in_bbox` union base CSR + overflow (deleted nodes still filtered at read time).
3. **Compact** — when overflow postings ≥ `geo_vec.spatial_overflow_compact_threshold`, or on index `VACUUM`, rebuild CSR via graph walk and clear overflow.

If the index was built with no rows, the first geometry insert creates a one-node CSR. Inserts outside the original grid extent clamp into boundary cells until the next compact recomputes extent.

## Planner Cost Estimation

Runtime already knows CSR candidate counts when the scan starts. The planner must decide **before** that — via `amcostestimate` in `cost_estimate.rs` — whether to use `geo_vec` vs HNSW+GiST (or seqscan).

### What the cost model estimates

1. **Spatial selectivity** — `clauselist_selectivity` on indexclause RestrictInfos (typically `geom && bbox`).
2. **Candidate count** — `cand ≈ selectivity × n`.
3. **Work (numIndexTuples)** aligned with scan routing:
   - no spatial qual → graph work ~ `search_list` (not `n/100`)
   - `cand ≤ spatial_brute_force_threshold` → brute → examine ~`cand`
   - else → hybrid → seed + graph work (grows with `√cand`, capped by `cand`)
4. **Return selectivity** — ANN `ORDER BY … LIMIT k` returns ≈ `k` rows (`PlannerInfo.limit_tuples`, else `search_list`).

```mermaid
flowchart LR
    Quals["indexclauses\n(geom && bbox)"] --> Sel["clauselist_selectivity"]
    Sel --> Cand["cand = sel × n"]
    Limit["limit_tuples / search_list"] --> Ret["indexSelectivity ≈ k/n"]
    Cand --> Route{"cand ≤ threshold?"}
    Route -->|yes| Brute["numIndexTuples ≈ cand"]
    Route -->|no| Hybrid["numIndexTuples ≈ √cand + search_list"]
    Brute --> Generic["genericcostestimate"]
    Hybrid --> Generic
    Generic --> Costs["startup / total cost\n+ ann selectivity"]
```

### Why this matters (Kigoto)

On ~318K rows, a **narrow/medium** bbox needs geo_vec for recall (HNSW+GiST often returns 0–1 of the true top-20). A **wide** bbox (~57% of rows) is a weak filter: both AMs can hit 20/20 recall, so cost/cardinality should not treat every bbox the same.

| Bbox | Competing indexes — old `n/100` | Competing indexes — current model |
|---|---|---|
| tiny | geo_vec | geo_vec |
| narrow | HNSW | geo_vec |
| medium | HNSW | geo_vec |
| wide | HNSW | geo_vec (cost rises vs narrow) |

Smoke: `make bench-planner-choice` (builds `kigoto_planner` with geo_vec + HNSW + GiST). Details and numbers: [README Benchmarks](../README.md#benchmarks).

### Limits

- Selectivity quality depends on Postgres/PostGIS stats for `&&`; bad stats ⇒ wrong `cand`.
- The model approximates CSR counts; it does not read the CSR at plan time.
- Preferring geo_vec on very wide filters may overshoot effective QPS — tune thresholds / revisit costs if production plans look wrong.

Further planner and scan improvements (wide-bbox penalty, MetaPage histograms, incremental CSR, spatial node layout): [ROADMAP](ROADMAP.md).

## Spatial Cell Index

A grid-based auxiliary structure stored in CSR (Compressed Sparse Row) format alongside the DiskANN graph.

```mermaid
block-beta
    columns 4
    block:grid:4
        columns 4
        c0["Cell 0"] c1["Cell 1"] c2["Cell 2"] c3["Cell 3"]
        c4["Cell 4"] c5["Cell 5"] c6["Cell 6"] c7["Cell 7"]
        c8["Cell 8"] c9["Cell 9"] c10["Cell 10"] c11["Cell 11"]
    end
```

**CSR storage**: `cell_offsets[i]..cell_offsets[i+1]` indexes into `node_pointers[]` to give all nodes in cell `i`.

- **Auto-sized** to ~100 points per cell
- **Multi-cell assignment**: each node is inserted into every cell its bounding box overlaps (not just its centroid cell). This guarantees that any query bbox overlapping a node's bbox will find it, regardless of geometry size. A small point-like building lands in 1 cell; a large polygon spanning many cells appears in all of them. Typical overhead is ~15-20% extra entries for building-scale data.
- **`nodes_in_bbox()`**: union of all nodes in overlapping grid cells, deduplicated via `HashSet` (since a node may appear in multiple cells)
- **`sample_seeds_in_bbox()`**: evenly-spaced samples from overlapping cells (for hybrid search entry points)
- Serialized to chained pages via `ChainTapeWriter`

## Storage Layer

Two storage backends, selected at index creation (default: SBQ).

```mermaid
classDiagram
    class StorageTrait {
        <<trait>>
        +create_node()
        +visit_lsn()
        +return_lsn() HeapPointer, BBox2D
        +get_query_distance_measure()
        +get_full_distance_for_resort()
    }

    class PlainStorage {
        PlainNode: f32 vector + BBox2D + labels + neighbors + heap_pointer
        Distance: f32-to-f32 (exact)
    }

    class SbqSpeedupStorage {
        SbqNode: u64 quantized vec + BBox2D + labels + neighbors + heap_pointer
        Distance: XOR Hamming (approximate) + rescore from heap (exact)
    }

    StorageTrait <|-- PlainStorage
    StorageTrait <|-- SbqSpeedupStorage
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

```mermaid
flowchart TD
    Extract["postgis_extract_bbox(geom_datum)"]
    Extract --> ReadHeader["Read GSERIALIZED header bytes"]
    ReadHeader --> BBoxCached{"gflags bit 2 set?\n(BBOX cached)"}
    BBoxCached -->|Yes| FastPath["Read 4 x f32 directly\nfrom header -> BBox2D\n(no PostGIS function call)"]
    BBoxCached -->|No| SlowPath["dlsym LWGEOM_to_BOX2DF\nDirectFunctionCall1Coll\n-> BBox2D"]

    note["ensure_postgis_loaded() called once at build start\nSPI query pg_extension -> load_file(postgis .so path)"]
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
