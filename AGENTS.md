# AGENTS.md - Agent Coding Guidelines for pg_geo_vec

## Project Overview

pg_geo_vec is a PostgreSQL extension for combined vector similarity search + spatial filtering using a single composite index. Built on DiskANN graph algorithm with SBQ compression and spatial cell indexing.

## Build Commands

### Prerequisites
- PostgreSQL 17+ with PostGIS and pgvector
- Rust toolchain (latest stable)
- cargo-pgrx 0.16.1

### Architecture Setup

**x86_64 (Linux):**
```bash
export RUSTFLAGS="-C target-feature=+avx2,+fma"
```

**aarch64/Apple Silicon (macOS):**
```bash
export MACOSX_DEPLOYMENT_TARGET=$(sw_vers -productVersion)
export RUSTFLAGS="-C target-feature=+neon -C link-arg=-Wl,-undefined,dynamic_lookup"
```

### Build & Test

```bash
# Build debug
cargo build --no-default-features --features pg17

# Install release
cargo pgrx install --release --no-default-features --features pg17

# Initialize pgrx (one-time)
cargo pgrx init --pg17=/usr/bin/pg_config

# Run all unit tests
RUSTFLAGS="-C target-feature=+avx2,+fma" cargo test --no-default-features --features pg17

# Run single test
RUSTFLAGS="-C target-feature=+avx2,+fma" cargo test test_name --no-default-features --features pg17

# Run tests in module
RUSTFLAGS="-C target-feature=+avx2,+fma" cargo test module_name:: --no-default-features --features pg17

# Integration tests (requires PostgreSQL)
RUSTFLAGS="-C target-feature=+avx2,+fma" cargo pgrx test --no-default-features --features pg17

# Benchmarks
RUSTFLAGS="-C target-feature=+avx2,+fma" cargo bench --no-default-features --features pg17

# Clippy linting
RUSTFLAGS="-C target-feature=+avx2,+fma" cargo clippy --no-default-features --features pg17
```

### Containerized Integration Tests
```bash
podman build -t geo_vec_test -f Containerfile.test .
podman run --rm -v $(pwd):/workspace geo_vec_test bash /workspace/test_data/ci_test.sh
```

## Code Style Guidelines

### General
- **Edition**: Rust 2021
- **Crate types**: cdylib + rlib
- Must compile with `cargo build --no-default-features --features pg17`

### Imports
Use `pgrx::prelude::*` for main code. Group: external, crate, module. Use absolute imports:

```rust
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use pg_sys::{FunctionCall0Coll, InvalidOid};
use pgrx::pg_sys::{index_getprocinfo, pgstat_progress_update_param, Oid};
use pgrx::*;
use crate::access_method::distance::DistanceType;
```

### Naming
- Modules: `snake_case` (e.g., `spatial_index`)
- Types/Structs/Enums: `PascalCase` (e.g., `BuildState`)
- Functions: `snake_case` (e.g., `ambuild`)
- Constants: `SCREAMING_SNAKE_CASE` (e.g., `GEO_VEC_DISTANCE_TYPE_PROC`)
- Traits: `PascalCase` (e.g., `ReadableNode`)

### Error Handling
Use pgrx patterns (PgResult, spi::Result). Use `panic!` for internal errors. Check NULL pointers.

```rust
let result = Spi::get_one::<bool>("SELECT ...")?
    .expect("result was null");

fn foo() -> spi::Result<()> {
    let val = Spi::get_one::<i32>("SELECT 1")?.expect("null");
    Ok(())
}
```

### Unsafe Code
All FFI in `unsafe` blocks. Use `pg_sys` pointers. Document invariants.

```rust
unsafe {
    let tape = Tape::new(index_relation, page_type);
}
```

### pgrx Patterns
- `#[pg_extern]` for SQL functions
- `#[pg_guard]` for callbacks
- `#[pg_schema]` for test modules
- `#[pg_test]` for integration tests
- `extension_sql!` for embedded SQL

```rust
#[pg_extern(immutable, parallel_safe)]
pub fn my_function(arg: i32) -> i32 { arg + 1 }

#[pg_test]
fn test_my_function() -> spi::Result<()> {
    let result = Spi::get_one::<i32>("SELECT my_function(1)")?
        .expect("null");
    assert_eq!(result, 2);
    Ok(())
}
```

### Memory
- Use `PgBox<T>` for owned PostgreSQL allocations
- Use `PgMemoryContexts` for temporary allocations

### Testing
- Unit: `#[test]` in `#[cfg(test)]` modules
- Integration: `#[pg_test]` in `#[cfg(any(test, feature = "pg_test"))]`
- Colocate tests with code in `mod tests` blocks

### PostgreSQL Patterns
- Use `ItemPointer` for (block, offset)
- Use `PgRelation` for index/table relations
- Prefix SQL functions with extension name

### Performance
- Profile before optimizing
- Use SIMD via `simdeez` crate
- Use `rkyv` for zero-copy serialization

### File Organization
```
src/
├── lib.rs              # Extension entry
├── access_method/      # Index access method
│   ├── mod.rs
│   ├── build.rs       # Index build
│   ├── scan.rs       # Index scan
│   ├── graph/        # DiskANN graph
│   ├── distance/     # SIMD distance
│   └── storage/      # Vector storage
├── partition/        # Spatial partitioning
└── util/              # Utilities
```

### Cargo Features
- `pg14`-`pg18`: PostgreSQL version
- `pg_test`: Enable pg_test framework
- `build_parallel`: Parallel builds
- Default: `pg18`, `build_parallel`

### Lints
The project allows specific cfgs in Cargo.toml:
```toml
[lints.rust]
unexpected_cfgs = { level = "allow", check-cfg = [
    'cfg(pgrx_embed)',
    'cfg(pg12)',
] }
```
