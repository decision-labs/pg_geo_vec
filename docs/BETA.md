# Private beta — `0.1.0-beta.1`

Internal Decision Labs / GeoBase release of `geo_vec` for packaging into **geobase-postgres** (PG17).

## Tag

- Git tag: `v0.1.0-beta.1`
- Extension version: `0.1.0-beta.1` (from `Cargo.toml` → `geo_vec.control`)
- Supported build: **PostgreSQL 17**, pgrx **0.16.1**
- Default Cargo features: `pg17`, `build_parallel`

## What’s in this beta

- Composite DiskANN + spatial CSR index (`USING geo_vec`)
- Brute / hybrid / graph query routing
- Planner cardinality-aware `amcostestimate` (issue #4)
- Incremental CSR MVP (INSERT → overflow; VACUUM / threshold → compact)
- Coexistence with `vector` + `vectorscale` (DiskANN AM)

## Known limits

- Overflow rewrite-on-insert (not append-chained segments yet)
- Outside-extent inserts clamp until CSR compact
- Geometry UPDATE relies on delete+insert + later compact
- Wide-bbox planner may still over-prefer geo_vec vs HNSW on effective QPS
- CI may require org billing; validate with `make bench-kigoto` / Docker install locally

## Build (release)

```bash
# x86_64
export RUSTFLAGS="-C target-feature=+avx2,+fma"
cargo pgrx install --release --features pg17
# (default features already include pg17 on this tag)
```

Installed artifacts (typical Debian layout):

- `$libdir/geo_vec*.so`
- `share/extension/geo_vec.control`
- `share/extension/geo_vec--0.1.0-beta.1.sql` (name follows cargo version)

## GeoBase packaging

Clone this **tag** in the `geo_vec-source` Docker stage (private GitHub auth required), then `cargo pgrx install` + `.deb` like `pgvectorscale`. Pin:

```yaml
geo_vec_release: "v0.1.0-beta.1"
geo_vec_repo: "https://github.com/decision-labs/pg_geo_vec.git"
```

Smoke:

```sql
CREATE EXTENSION postgis;
CREATE EXTENSION vector;
CREATE EXTENSION geo_vec;
CREATE INDEX ON t USING geo_vec (embedding vector_cosine_ops, geom geometry_geo_vec_ops);
```

## Not for public crates.io / public GitHub release yet

Still open from public-release issue #2: regenerate committed SQL if needed, parquet hosting, checked rkyv, etc. This beta is **private** distribution via git tag + GeoBase image only.
