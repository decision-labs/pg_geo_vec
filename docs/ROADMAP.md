# Roadmap

Ideas for the `geo_vec` access method: correctness, planner, spatial structure, query path, and ops. Ordered sections match [recommended sequencing](#recommended-sequence). See also [Planner Cost Estimation](ARCHITECTURE.md#planner-cost-estimation) and [README Benchmarks](../README.md#benchmarks).

## Recommended sequence

| Priority | Item | Why |
|---|---|---|
| 1 | ~~Incremental CSR (MVP done)~~ → refine UPDATE/segment chaining | MVP: overflow append + VACUUM compact shipped |
| 2 | Planner wide-bbox penalty + MetaPage cell stats | Stops over-preferring geo_vec on weak filters; better effective QPS |
| 3 | `target_per_cell` WITH + adaptive brute/hybrid threshold | Easy knobs; avoids catastrophic wrong-path latency |
| 4 | Label ∩ spatial posting lists | Unique vs HNSW+GiST when both filters are present |
| 5 | Spatial-aware graph layout / better hybrid seeds | Largest **in-path** latency win once mutability exists |
| 6 | Embedded R-tree spatial layer | Structural upgrade for heterogeneous geometries |
| 7 | 3D bbox / `&&&` | New capability (see arXiv:2507.09459) |

## What is likely to make the biggest performance impact?

Distinguish three meanings of “performance”:

### 1. Effective QPS (user-facing, competing indexes)

**Biggest lever: planner cardinality + wide-bbox cost penalty.**

On Kigoto, HNSW+GiST is *faster* on wide (~57% of rows) when recall stays 20/20, but *wrong* on narrow/medium (0–1/20 recall). Picking the wrong AM dominates any micro-optimization inside geo_vec. Feeding CSR-ish candidate estimates into `amcostestimate` (MetaPage cell histograms, custom `&&` RESTRICT) and **penalizing high `spatial_sel`** will move end-to-end results more than tuning `search_list` alone.

### 2. Latency when the geo_vec path is already chosen

**Biggest lever: I/O locality + hybrid work reduction** (not replacing the CSR with an R-tree first).

Warm Kigoto hybrid is already ~40–45ms. Cold / large indexes are dominated by **random page reads** into DiskANN neighbors. Highest expected returns:

1. **Spatial (or Hilbert) clustering of node storage at build** — keep same-cell / nearby-cell nodes on fewer pages so hybrid seed expansion is less random. Often 2×+ on cold cache; little algorithmic change.
2. **Better hybrid seeds + early terminate** — density- or diversity-aware seeds; stop when top-K is stable. Cuts distance evaluations and neighbor fetches at fixed recall.
3. **Adaptive brute vs hybrid threshold** — forced brute on wide was ~499ms vs hybrid ~40–60ms (~8×). A fixed `5000` is wrong for high-dim / large `search_list`; mis-routing is a cliff, not a nudge.
4. **Parallel scan (`amcanparallel`)** — helps wide hybrid/brute throughput; secondary to locality for single-query latency.
5. **SBQ + spatial entry points** — cheaper first-pass distances if rescore stays correct.

An **STR R-tree** mainly helps when multi-cell duplication and large polygons inflate candidate sets. On building-scale points, CSR + multi-cell is already fine; R-tree is a mid/long-term structural win, not the first latency knob.

### 3. Throughput under writes

**Biggest lever: incremental CSR (or append segments + merge).**

Until INSERT/UPDATE/VACUUM maintain the spatial structure, every mutable workload forces `REINDEX` — that dwarfs query-path tuning for real deployments.

### Bottom line

| Goal | Do this first |
|---|---|
| Ship on live tables | Incremental CSR |
| Beat HNSW+GiST on the right queries | Planner (sel + wide penalty + cell stats) |
| Make geo_vec scans faster | Node clustering / seeds / adaptive threshold |
| Handle big polygons & less CSR bloat | R-tree |
| New query shapes | Labels∩space, then 3D |

---

## Correctness / mutability

- [x] **Incremental CSR on INSERT / VACUUM (MVP)** — Base CSR stays frozen; `aminsert` appends to a `SpatialOverflowIndex` (same grid); scans union base+overflow; `VACUUM` / overflow threshold compact rebuilds CSR from the live graph and clears overflow. GUC: `geo_vec.spatial_overflow_compact_threshold` (default 10000). MetaPage v4 adds `spatial_overflow_*`. Outside-extent inserts still clamp to boundary cells until compact expands the grid.
- [ ] **Geometry UPDATE / smarter dirty tracking** — UPDATE is delete+insert today; soft-deleted CSR slots linger until compact. Optional: skip rewrite-on-every-insert by chaining overflow segments instead of rewriting the overflow blob.
- [ ] **Stale spatial index signaling** — GUC or MetaPage flag (`spatial_index_stale_pct`) → NOTICE / refuse hybrid until rebuild.

## Planner

- [x] **Spatial + LIMIT-aware `amcostestimate`** — Replace flat `n/100` with `clauselist_selectivity`, LIMIT, and brute/hybrid/graph work (issue [#4](https://github.com/decision-labs/pg_geo_vec/issues/4)).
- [ ] **Wide-bbox / high-selectivity penalty** — When `spatial_sel` is high (e.g. >30–40%), inflate hybrid cost so HNSW+GiST can win when recall parity is likely (Kigoto wide ≈57%).
- [ ] **MetaPage cell cardinalities / histogram** — Estimate overlap at plan time from query bbox vs grid without reading full CSR.
- [ ] **Custom RESTRICT for `geometry_geo_vec_ops`** — Bbox-vs-index-extent (or grid) selectivity beats generic PostGIS estimates for this AM.
- [ ] **`geo_vec_estimate_candidates(regclass, geometry)`** — SQL helper for EXPLAIN/debug and app-side path hints.

## Spatial structure

- [ ] **Configurable `target_per_cell`** — Expose grid density (currently hardcoded ~100 nodes/cell) as `WITH (spatial_target_per_cell = 50)` on `CREATE INDEX`. Helps non-uniform distributions.
- [ ] **Adaptive / two-level grid** — Quadtree or coarse+fine grid where density varies (dense cities vs empty cells).
- [ ] **Embedded R-tree spatial index** — Bulk-loaded R-tree (STR packing) for O(log n + k) lookups; cuts multi-cell duplication; better for mixed small buildings + large parcels.
- [ ] **3D bounding box support** — Extend `BBox2D` → `BBox3D`, Z from GSERIALIZED, 3D partitioner, PostGIS `&&&` ([arXiv:2507.09459](https://arxiv.org/pdf/2507.09459)).

## Query path

- [ ] **Adaptive brute ↔ hybrid threshold** — Factor in dimension, `search_list`, and cost of exact distance — not only raw candidate count.
- [ ] **Better hybrid seeds** — Prefer dense or vector-diverse cells; optional per-cell medoids stored at build.
- [ ] **Early terminate hybrid** — Stop graph expansion when top-K is stable and spatial coverage is saturated.
- [ ] **Parallel index scan** — Enable `amcanparallel` for wide hybrid/brute.
- [ ] **Label ∩ spatial posting lists** — AND of label and spatial CSR for `WHERE geom && … AND labels && …` (differentiator vs HNSW+GiST).

## Graph / storage

- [ ] **Spatial-aware node layout** — Cluster graph nodes by cell or Hilbert order at build so hybrid I/O is more sequential.
- [ ] **SBQ + spatial entry points** — Confirm rescore quality; seed SBQ graph from spatial samples.
- [ ] **Spatial-aware graph edges (research)** — Bias neighbor selection toward spatially near nodes during build (trade pure-vector recall vs hybrid I/O).

## Ops / product

- [ ] **Partial indexes / partitioning docs** — Partition by region + geo_vec per partition to avoid huge weak bboxes.
- [ ] **Planner + latency smoke in CI** — `make bench-planner-choice` (and optional warm latency gates) on a small fixture.
- [ ] **Maintenance runbook** — When to `REINDEX`, how to read `EXPLAIN` chosen AM, GUC cheat sheet.
