# Roadmap

## Planned

- [ ] **3D bounding box support** — extend `BBox2D` to `BBox3D` (6 x f32), extract Z bounds from GSERIALIZED, add 3D grid partitioner, and support PostGIS `&&&` overlap operator (https://arxiv.org/pdf/2507.09459)
- [ ] **Configurable `target_per_cell`** — expose the spatial grid density (currently hardcoded at 100 nodes/cell) as a `WITH` clause parameter on `CREATE INDEX`, e.g. `WITH (spatial_target_per_cell = 50)`. Useful for tuning grid granularity on non-uniform data distributions.