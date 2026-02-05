# pg_geo_vec

A PostgreSQL extension providing a **composite index** that efficiently combines **vector similarity search** with **spatial (geometry) filtering**.

## Overview

pg_geo_vec extends PostgreSQL's indexing capabilities by creating a unified index structure that integrates:
- **Vector similarity search** (via StreamingDiskANN/HNSW graphs)
- **Spatial filtering** (via bounding box partitioning)

This enables efficient queries combining both spatial and semantic similarity:
- "Find restaurants similar to this one, within 10km"
- "Find places like this, in this city"

## Architecture

```
┌─────────────────────────────────────────┐
│           pg_geo_vec Index               │
├─────────────────────────────────────────┤
│  Meta Page                              │
│  ├─ Vector dimension, distance type     │
│  ├─ Number of partitions                │
│  ├─ Global bbox (union of all)          │
│  └─ Partition metadata array            │
├─────────────────────────────────────────┤
│  Partition 0 (Spatial Region)           │
│  ├─ Partition bbox                      │
│  └─ StreamingDiskANN Graph              │
│     └─ Nodes: (vector + bbox + neighbors)│
├─────────────────────────────────────────┤
│  Partition 1 (Spatial Region)           │
│  ...                                    │
└─────────────────────────────────────────┘
```

## Key Features

- **Composite Index**: Single index handles both vector and spatial queries
- **Spatial Partitioning**: Data divided by geographic regions using bbox
- **BBox Filtering**: Each node stores its bounding box for fast spatial filtering
- **StreamingDiskANN**: Efficient graph-based vector search
- **Parallel Build**: Multi-worker index construction
- **PostGIS Integration**: Uses `gserialized_get_gbox_p()` for bbox extraction

## Implementation Status

| Component | Status | Notes |
|----------|--------|-------|
| BBox2D struct | ✅ Done | 16-byte bbox with rkyv derives |
| PostGIS FFI | ✅ Done | `gserialized_get_gbox_p()` bindings |
| PlainNode + bbox | ✅ Done | Added `bbox` field |
| SbqNode + bbox | ✅ Done | Added `bbox` field |
| Storage trait | ✅ Done | `create_node(bbox)` updated |
| Partition metadata | ✅ Done | `PartitionMetadata` struct |
| Spatial partitioning | 🔄 In Progress | Grid/Quadtree logic |
| BBox filtering in search | ⏳ Pending | Graph traversal filter |
| Partition selection | ⏳ Pending | Query bbox → partitions |

## Installation

### Prerequisites

- PostgreSQL 17+
- Rust 1.70+
- cargo-pgrx (`cargo install cargo-pgrx`)
- PostGIS extension

### Build

```bash
cargo pgrx init --pg17 /path/to/pg_config
cargo pgrx install --release
```

### Usage

```sql
-- Create extension
CREATE EXTENSION IF NOT EXISTS postgis;
CREATE EXTENSION IF NOT EXISTS geo_vec;

-- Create table with vector and geometry
CREATE TABLE places (
    id SERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    embedding vector(1536),
    geom geometry(Point, 4326)
);

-- Create composite index
CREATE INDEX idx_places_geo_vec
ON places USING geo_vec (embedding, geom)
WITH (num_partitions = 64);

-- Query with spatial + vector filters
SELECT id, name,
       embedding <=> query_vector AS distance
FROM places
WHERE geom && ST_MakeEnvelope(-122.5, 37.5, -122.0, 38.0, 4326)
ORDER BY embedding <=> query_vector
LIMIT 10;
```

## How It Works

### 1. Index Build

```rust
// For each tuple in the table:
fn build_callback(values, isnull) {
    // Extract vector from first column
    let vector = extract_vector(values[0]);

    // Extract bbox from geometry using PostGIS
    let bbox = unsafe {
        postgis_extract_bbox(values[1])  // Calls gserialized_get_gbox_p()
    };

    // Store node with vector + bbox
    storage.create_node(vector, bbox, heap_ptr, meta_page);

    // Assign to partition based on bbox
    let partition = partition_manager.get_partition(bbox);
}
```

### 2. Node Storage

```rust
struct PlainNode {
    vector: Vec<f32>,      // embedding
    bbox: BBox2D,          // 16 bytes: xmin, xmax, ymin, ymax
    neighbors: Vec<u32>,    // graph neighbors
    heap_ptr: ItemPointer,  // back to heap tuple
}
```

### 3. Query Processing

```sql
SELECT * FROM places
WHERE geom && ST_MakeEnvelope(...)    -- PostGIS spatial filter
ORDER BY embedding <=> query_vec       -- Vector similarity
LIMIT 10;
```

```
Query Flow:
┌─────────────────────────────────────────┐
│ Parse query                              │
│ - Extract query vector                  │
│ - Extract query bbox from ST_MakeEnvelope│
└─────────────────────────────────────────┘
                    ▼
┌─────────────────────────────────────────┐
│ Phase 1: Find overlapping partitions   │
│                                         │
│  Check each partition's bbox:           │
│  ├─ Partition 0: bbox overlaps? YES ✓   │
│  ├─ Partition 1: bbox overlaps? YES ✓   │
│  └─ Partition 2: bbox overlaps? NO ✗    │
│                                         │
│  Result: Search only partitions 0, 1     │
└─────────────────────────────────────────┘
                    ▼
┌─────────────────────────────────────────┐
│ Phase 2: Search graphs (per partition)  │
│                                         │
│  For each candidate node:                │
│  if node.bbox.overlaps(query_bbox) {     │
│      compute_distance(vector, query)     │
│  } else {                               │
│      skip_node()  // Skip non-matching!  │
│  }                                      │
└─────────────────────────────────────────┘
                    ▼
┌─────────────────────────────────────────┐
│ Phase 3: Merge results                 │
│  Return top-K by distance               │
└─────────────────────────────────────────┘
```

## Project Structure

```
pg_geo_vec/
├── src/
│   ├── lib.rs                    # Extension entry point
│   ├── partition/
│   │   ├── mod.rs               # BBox2D, Partition, PartitionManager
│   │   └── postgis.rs           # PostGIS FFI bindings
│   ├── access_method/
│   │   ├── mod.rs               # Access method handler
│   │   ├── build.rs             # Index build (ambuild)
│   │   ├── scan.rs              # Index scan (amgettuple)
│   │   ├── meta_page.rs         # Meta page with partition metadata
│   │   ├── plain/
│   │   │   └── node.rs          # PlainNode with bbox
│   │   ├── sbq/
│   │   │   └── node.rs          # SbqNode with bbox
│   │   └── graph/               # StreamingDiskANN graph
│   └── access_method/
│       └── partition_metadata.rs  # PartitionMetadata, PartitionConfig
├── sql/
│   └── geo-vec--0.1.0.sql       # SQL definitions
└── pg_geo_vec_derive/            # Derive macros
```

## Reference Implementations

- **pgvectorscale**: Rust/pgrx access method patterns, StreamingDiskANN
- **pgvector**: HNSW algorithms, distance functions
- **PostGIS**: GiST patterns, `gserialized_get_gbox_p()`

## License

MIT or Apache 2.0
