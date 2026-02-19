//! Spatial cell index for geo_vec.
//!
//! A grid-based auxiliary structure mapping spatial cells to node IndexPointers.
//! Built during index creation and stored in chained pages (CSR format).
//!
//! For queries with a bbox filter, the spatial cell index provides 100% spatial
//! recall by finding all nodes in overlapping cells, computing vector distances,
//! and returning top-K results.

use std::collections::HashSet;

use rkyv::{Archive, Deserialize, Serialize};

use crate::partition::BBox2D;
use crate::util::ItemPointer;

/// Grid configuration for the spatial cell index.
/// Divides the spatial extent into a uniform grid of `cols x rows` cells.
#[derive(Clone, Debug, PartialEq, Archive, Deserialize, Serialize)]
pub struct SpatialGridConfig {
    pub xmin: f32,
    pub xmax: f32,
    pub ymin: f32,
    pub ymax: f32,
    pub cols: u32,
    pub rows: u32,
}

impl SpatialGridConfig {
    /// Auto-size grid to have approximately `target_per_cell` points per cell.
    /// Grid dimensions are chosen to produce roughly square cells.
    pub fn from_extent(bbox: BBox2D, num_points: usize) -> Self {
        let target_per_cell: usize = 100;
        let num_cells = ((num_points as f64) / (target_per_cell as f64)).max(1.0);

        let width = (bbox.xmax - bbox.xmin) as f64;
        let height = (bbox.ymax - bbox.ymin) as f64;

        let (cols, rows) = if width < 1e-9 && height < 1e-9 {
            (1u32, 1u32)
        } else if width < 1e-9 {
            (1, num_cells.ceil() as u32)
        } else if height < 1e-9 {
            (num_cells.ceil() as u32, 1)
        } else {
            let aspect = width / height;
            let c = (num_cells * aspect).sqrt();
            let r = (num_cells / aspect).sqrt();
            (c.ceil().max(1.0) as u32, r.ceil().max(1.0) as u32)
        };

        Self {
            xmin: bbox.xmin,
            xmax: bbox.xmax,
            ymin: bbox.ymin,
            ymax: bbox.ymax,
            cols,
            rows,
        }
    }

    /// Total number of cells in the grid.
    #[inline]
    pub fn num_cells(&self) -> usize {
        (self.cols as usize) * (self.rows as usize)
    }

    /// Linear cell index from (col, row). Row-major order.
    #[inline]
    pub fn cell_index(&self, col: u32, row: u32) -> usize {
        (row as usize) * (self.cols as usize) + (col as usize)
    }

    /// Map a point (x, y) to its (col, row). Points outside the grid are clamped.
    #[inline]
    pub fn point_to_cell(&self, x: f32, y: f32) -> (u32, u32) {
        let width = (self.xmax - self.xmin).max(f32::MIN_POSITIVE);
        let height = (self.ymax - self.ymin).max(f32::MIN_POSITIVE);

        let col = ((x - self.xmin) / width * self.cols as f32)
            .floor()
            .clamp(0.0, (self.cols - 1) as f32) as u32;
        let row = ((y - self.ymin) / height * self.rows as f32)
            .floor()
            .clamp(0.0, (self.rows - 1) as f32) as u32;
        (col, row)
    }

    /// Find all cell indices that overlap with the given bbox.
    pub fn bbox_to_cells(&self, bbox: &BBox2D) -> Vec<usize> {
        let width = (self.xmax - self.xmin).max(f32::MIN_POSITIVE);
        let height = (self.ymax - self.ymin).max(f32::MIN_POSITIVE);

        let min_col = ((bbox.xmin - self.xmin) / width * self.cols as f32)
            .floor()
            .clamp(0.0, (self.cols - 1) as f32) as u32;
        let max_col = ((bbox.xmax - self.xmin) / width * self.cols as f32)
            .floor()
            .clamp(0.0, (self.cols - 1) as f32) as u32;
        let min_row = ((bbox.ymin - self.ymin) / height * self.rows as f32)
            .floor()
            .clamp(0.0, (self.rows - 1) as f32) as u32;
        let max_row = ((bbox.ymax - self.ymin) / height * self.rows as f32)
            .floor()
            .clamp(0.0, (self.rows - 1) as f32) as u32;

        let mut cells = Vec::new();
        for row in min_row..=max_row {
            for col in min_col..=max_col {
                cells.push(self.cell_index(col, row));
            }
        }
        cells
    }
}

// ---------------------------------------------------------------------------
// CSR-based spatial cell index
// ---------------------------------------------------------------------------

/// A spatial cell index stored in CSR (Compressed Sparse Row) format.
///
/// `cell_offsets[i]..cell_offsets[i+1]` gives the range of `node_pointers`
/// belonging to cell `i`.
#[derive(Clone, Debug, PartialEq, Archive, Deserialize, Serialize)]
pub struct SpatialCellIndex {
    pub grid: SpatialGridConfig,
    pub cell_offsets: Vec<u32>,
    pub node_pointers: Vec<ItemPointer>,
}

impl SpatialCellIndex {
    /// Return all node pointers whose cells overlap the query bbox.
    /// Deduplicates because a node may appear in multiple cells.
    pub fn nodes_in_bbox(&self, query_bbox: &BBox2D) -> Vec<ItemPointer> {
        let cells = self.grid.bbox_to_cells(query_bbox);
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        for cell_idx in cells {
            let start = self.cell_offsets[cell_idx] as usize;
            let end = self.cell_offsets[cell_idx + 1] as usize;
            for &ip in &self.node_pointers[start..end] {
                if seen.insert(ip) {
                    result.push(ip);
                }
            }
        }
        result
    }

    /// Sample seed nodes from cells overlapping the query bbox.
    ///
    /// Returns `(seeds, total_candidate_count)` where `total_candidate_count` is the
    /// total number of nodes in overlapping cells (used for threshold decisions).
    /// Seeds are evenly-spaced samples from each cell (up to `max_per_cell` per cell).
    pub fn sample_seeds_in_bbox(
        &self,
        query_bbox: &BBox2D,
        max_per_cell: usize,
    ) -> (Vec<ItemPointer>, usize) {
        let cells = self.grid.bbox_to_cells(query_bbox);
        let mut seeds = Vec::new();
        let mut total_count: usize = 0;

        for cell_idx in cells {
            let start = self.cell_offsets[cell_idx] as usize;
            let end = self.cell_offsets[cell_idx + 1] as usize;
            let cell_len = end - start;
            total_count += cell_len;

            if cell_len == 0 {
                continue;
            }

            if cell_len <= max_per_cell {
                seeds.extend_from_slice(&self.node_pointers[start..end]);
            } else {
                // Evenly-spaced sampling
                for i in 0..max_per_cell {
                    let idx = start + i * cell_len / max_per_cell;
                    seeds.push(self.node_pointers[idx]);
                }
            }
        }

        (seeds, total_count)
    }

    /// Serialize to bytes via rkyv.
    pub fn serialize_to_bytes(&self) -> Vec<u8> {
        rkyv::to_bytes::<_, 4096>(self).unwrap().to_vec()
    }

    /// Deserialize from bytes via rkyv.
    pub fn deserialize_from_bytes(bytes: &[u8]) -> Self {
        unsafe { rkyv::from_bytes_unchecked::<Self>(bytes).unwrap() }
    }
}

/// Builder for constructing a `SpatialCellIndex` from (ItemPointer, BBox2D) pairs.
pub struct SpatialCellIndexBuilder {
    grid: SpatialGridConfig,
    entries: Vec<Vec<ItemPointer>>,
}

impl SpatialCellIndexBuilder {
    pub fn new(grid: SpatialGridConfig) -> Self {
        let num_cells = grid.num_cells();
        let entries = vec![Vec::new(); num_cells];
        Self { grid, entries }
    }

    /// Add a node to every cell its bbox overlaps, guaranteeing that any
    /// query bbox overlapping the node's bbox will find it.
    pub fn add(&mut self, index_pointer: ItemPointer, bbox: &BBox2D) {
        if bbox.is_empty() {
            return;
        }
        for cell_idx in self.grid.bbox_to_cells(bbox) {
            self.entries[cell_idx].push(index_pointer);
        }
    }

    /// Build the CSR structure.
    pub fn build(self) -> SpatialCellIndex {
        let num_cells = self.grid.num_cells();
        let mut cell_offsets = Vec::with_capacity(num_cells + 1);
        let mut node_pointers = Vec::new();

        let mut offset: u32 = 0;
        for cell in &self.entries {
            cell_offsets.push(offset);
            node_pointers.extend_from_slice(cell);
            offset += cell.len() as u32;
        }
        cell_offsets.push(offset);

        SpatialCellIndex {
            grid: self.grid,
            cell_offsets,
            node_pointers,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Real buildings extent: 8,494 building polygons in Washington state
    fn buildings_extent() -> BBox2D {
        BBox2D::new(-117.6022, -117.5821, 47.6499, 47.6561)
    }

    #[test]
    fn test_grid_cell_index() {
        let grid = SpatialGridConfig::from_extent(buildings_extent(), 8494);
        assert_eq!(grid.cell_index(0, 0), 0);
        assert_eq!(grid.cell_index(1, 0), 1);
        assert_eq!(grid.cell_index(0, 1), grid.cols as usize);
    }

    #[test]
    fn test_point_to_cell() {
        let grid = SpatialGridConfig::from_extent(buildings_extent(), 8494);
        // Building centroid roughly in the middle of the extent
        let (col, row) = grid.point_to_cell(-117.594, 47.653);
        assert!(col < grid.cols, "col {} should be < {}", col, grid.cols);
        assert!(row < grid.rows, "row {} should be < {}", row, grid.rows);
    }

    #[test]
    fn test_point_outside_grid() {
        let grid = SpatialGridConfig::from_extent(buildings_extent(), 8494);
        // Point far outside the extent should clamp to boundary
        let (col, row) = grid.point_to_cell(-118.0, 48.0);
        assert_eq!(col, 0); // far left clamps to 0
        assert_eq!(row, grid.rows - 1); // far above clamps to max row
    }

    #[test]
    fn test_bbox_overlapping_cells() {
        let grid = SpatialGridConfig::from_extent(buildings_extent(), 8494);
        // ~10% sub-bbox
        let sub_bbox = BBox2D::new(-117.596, -117.590, 47.652, 47.654);
        let cells = grid.bbox_to_cells(&sub_bbox);
        assert!(!cells.is_empty(), "should overlap at least one cell");
        // All cell indices should be valid
        for &c in &cells {
            assert!(c < grid.num_cells(), "cell {} out of range", c);
        }
    }

    #[test]
    fn test_single_cell_grid() {
        // Use a square extent so aspect ratio doesn't create extra cols/rows
        let bbox = BBox2D::new(0.0, 1.0, 0.0, 1.0);
        let grid = SpatialGridConfig::from_extent(bbox, 50);
        assert_eq!(grid.cols, 1);
        assert_eq!(grid.rows, 1);
        assert_eq!(grid.num_cells(), 1);

        let (col, row) = grid.point_to_cell(0.5, 0.5);
        assert_eq!(col, 0);
        assert_eq!(row, 0);
    }

    #[test]
    fn test_grid_auto_sizing() {
        let grid = SpatialGridConfig::from_extent(buildings_extent(), 8494);
        // With ~100 pts/cell we expect ~85 cells total
        let total = grid.num_cells();
        assert!(total > 10, "expected more than 10 cells, got {}", total);
        assert!(total < 500, "expected fewer than 500 cells, got {}", total);
    }

    // ---- CSR Builder tests ----

    #[test]
    fn test_builder_single_node() {
        let grid = SpatialGridConfig::from_extent(buildings_extent(), 100);
        let mut builder = SpatialCellIndexBuilder::new(grid);
        let ip = ItemPointer::new(1, 1);
        let bbox = BBox2D::new(-117.594, -117.593, 47.653, 47.654);
        builder.add(ip, &bbox);
        let index = builder.build();
        assert_eq!(index.node_pointers.len(), 1);
    }

    #[test]
    fn test_builder_empty_cells() {
        let grid = SpatialGridConfig::from_extent(buildings_extent(), 100);
        let num_cells = grid.num_cells();
        let mut builder = SpatialCellIndexBuilder::new(grid);
        // Add one node to just one cell
        let ip = ItemPointer::new(1, 1);
        let bbox = BBox2D::new(-117.594, -117.593, 47.653, 47.654);
        builder.add(ip, &bbox);
        let index = builder.build();

        // All but one cell should be empty
        let mut non_empty = 0;
        for i in 0..num_cells {
            if index.cell_offsets[i + 1] > index.cell_offsets[i] {
                non_empty += 1;
            }
        }
        assert_eq!(non_empty, 1);
    }

    #[test]
    fn test_builder_multi_cell() {
        let grid = SpatialGridConfig::from_extent(buildings_extent(), 100);
        let mut builder = SpatialCellIndexBuilder::new(grid);

        // Two nodes in different parts of the extent
        let ip1 = ItemPointer::new(1, 1);
        let bbox1 = BBox2D::new(-117.600, -117.599, 47.650, 47.651);
        builder.add(ip1, &bbox1);

        let ip2 = ItemPointer::new(2, 1);
        let bbox2 = BBox2D::new(-117.584, -117.583, 47.655, 47.656);
        builder.add(ip2, &bbox2);

        let index = builder.build();
        assert_eq!(index.node_pointers.len(), 2);
    }

    #[test]
    fn test_roundtrip_serialization() {
        let grid = SpatialGridConfig::from_extent(buildings_extent(), 100);
        let mut builder = SpatialCellIndexBuilder::new(grid);

        for i in 0..50 {
            let ip = ItemPointer::new(i, 1);
            let frac = i as f32 / 50.0;
            let x = -117.602 + frac * 0.020;
            let y = 47.650 + frac * 0.006;
            let bbox = BBox2D::new(x, x + 0.001, y, y + 0.001);
            builder.add(ip, &bbox);
        }
        let index = builder.build();

        let bytes = index.serialize_to_bytes();
        let index2 = SpatialCellIndex::deserialize_from_bytes(&bytes);
        assert_eq!(index, index2);
    }

    #[test]
    fn test_cell_query() {
        let extent = buildings_extent();
        let grid = SpatialGridConfig::from_extent(extent, 8494);
        let mut builder = SpatialCellIndexBuilder::new(grid);

        // Populate 1000 nodes spread across the extent
        for i in 0..1000u32 {
            let ip = ItemPointer::new(i, 1);
            let frac = i as f32 / 1000.0;
            let x = extent.xmin + frac * (extent.xmax - extent.xmin);
            let y = extent.ymin + ((i % 37) as f32 / 37.0) * (extent.ymax - extent.ymin);
            let bbox = BBox2D::new(x, x + 0.0001, y, y + 0.0001);
            builder.add(ip, &bbox);
        }
        let index = builder.build();

        // Query a sub-bbox covering roughly the left quarter
        let quarter_x = extent.xmin + (extent.xmax - extent.xmin) * 0.25;
        let query = BBox2D::new(extent.xmin, quarter_x, extent.ymin, extent.ymax);
        let nodes = index.nodes_in_bbox(&query);
        assert!(!nodes.is_empty(), "should find nodes in left quarter");
        assert!(
            nodes.len() < 1000,
            "shouldn't find all nodes, found {}",
            nodes.len()
        );
        // Should find roughly 25% of nodes (±tolerance for grid alignment)
        assert!(
            nodes.len() < 500,
            "should find fewer than half, found {}",
            nodes.len()
        );
    }

    #[test]
    fn test_8494_buildings() {
        let extent = buildings_extent();
        let grid = SpatialGridConfig::from_extent(extent, 8494);
        let mut builder = SpatialCellIndexBuilder::new(grid);

        for i in 0..8494u32 {
            let ip = ItemPointer::new(i, 1);
            let frac = i as f32 / 8494.0;
            let x = extent.xmin + frac * (extent.xmax - extent.xmin);
            let y = extent.ymin + ((i % 100) as f32 / 100.0) * (extent.ymax - extent.ymin);
            let bbox = BBox2D::new(x, x + 0.0001, y, y + 0.0001);
            builder.add(ip, &bbox);
        }
        let index = builder.build();
        // With multi-cell assignment, nodes at cell boundaries may appear in
        // multiple cells, so total entries >= unique node count.
        assert!(
            index.node_pointers.len() >= 8494,
            "expected at least 8494 entries, got {}",
            index.node_pointers.len()
        );
    }

    #[test]
    fn test_sample_seeds_basic() {
        let extent = buildings_extent();
        let grid = SpatialGridConfig::from_extent(extent, 8494);
        let mut builder = SpatialCellIndexBuilder::new(grid);

        // Populate 1000 nodes spread across the extent
        for i in 0..1000u32 {
            let ip = ItemPointer::new(i, 1);
            let frac = i as f32 / 1000.0;
            let x = extent.xmin + frac * (extent.xmax - extent.xmin);
            let y = extent.ymin + ((i % 37) as f32 / 37.0) * (extent.ymax - extent.ymin);
            let bbox = BBox2D::new(x, x + 0.0001, y, y + 0.0001);
            builder.add(ip, &bbox);
        }
        let index = builder.build();

        // Sample seeds from the full extent
        let (seeds, total) = index.sample_seeds_in_bbox(&extent, 2);
        // total counts CSR entries (may include multi-cell duplicates)
        assert!(total >= 1000, "total count should be at least 1000, got {}", total);
        // Seeds should be much fewer than total
        assert!(seeds.len() <= index.grid.num_cells() * 2);
        assert!(!seeds.is_empty());
    }

    #[test]
    fn test_sample_seeds_small_cells() {
        let extent = buildings_extent();
        let grid = SpatialGridConfig::from_extent(extent, 100);
        let mut builder = SpatialCellIndexBuilder::new(grid);

        // Only 3 nodes, some cells will have ≤ max_per_cell
        let ip1 = ItemPointer::new(1, 1);
        let bbox1 = BBox2D::new(-117.600, -117.599, 47.650, 47.651);
        builder.add(ip1, &bbox1);
        let ip2 = ItemPointer::new(2, 1);
        let bbox2 = BBox2D::new(-117.600, -117.599, 47.650, 47.651); // same cell
        builder.add(ip2, &bbox2);
        let ip3 = ItemPointer::new(3, 1);
        let bbox3 = BBox2D::new(-117.584, -117.583, 47.655, 47.656); // different cell
        builder.add(ip3, &bbox3);

        let index = builder.build();
        let (seeds, total) = index.sample_seeds_in_bbox(&extent, 2);
        assert_eq!(total, 3);
        // With max_per_cell=2, cells with ≤2 nodes return all nodes
        assert_eq!(seeds.len(), 3);
    }

    #[test]
    fn test_sample_seeds_empty_bbox() {
        let extent = buildings_extent();
        let grid = SpatialGridConfig::from_extent(extent, 100);
        let builder = SpatialCellIndexBuilder::new(grid);
        let index = builder.build();

        // No nodes at all
        let (seeds, total) = index.sample_seeds_in_bbox(&extent, 2);
        assert_eq!(total, 0);
        assert!(seeds.is_empty());
    }

    #[test]
    fn test_sample_seeds_narrow_bbox() {
        let extent = buildings_extent();
        let grid = SpatialGridConfig::from_extent(extent, 8494);
        let mut builder = SpatialCellIndexBuilder::new(grid);

        for i in 0..8494u32 {
            let ip = ItemPointer::new(i, 1);
            let frac = i as f32 / 8494.0;
            let x = extent.xmin + frac * (extent.xmax - extent.xmin);
            let y = extent.ymin + ((i % 100) as f32 / 100.0) * (extent.ymax - extent.ymin);
            let bbox = BBox2D::new(x, x + 0.0001, y, y + 0.0001);
            builder.add(ip, &bbox);
        }
        let index = builder.build();

        // Narrow bbox — only a small portion
        let narrow = BBox2D::new(-117.596, -117.594, 47.652, 47.654);
        let (seeds, total) = index.sample_seeds_in_bbox(&narrow, 2);
        assert!(total < 8494, "narrow bbox should not contain all nodes");
        assert!(seeds.len() <= total);
        assert!(!seeds.is_empty());
    }

    #[test]
    fn test_skip_empty_bbox() {
        let grid = SpatialGridConfig::from_extent(buildings_extent(), 100);
        let mut builder = SpatialCellIndexBuilder::new(grid);
        // Empty bbox should be skipped
        builder.add(ItemPointer::new(1, 1), &BBox2D::empty());
        let index = builder.build();
        assert_eq!(index.node_pointers.len(), 0);
    }

    #[test]
    fn test_large_geom_found_in_all_overlapping_cells() {
        let extent = buildings_extent();
        let grid = SpatialGridConfig::from_extent(extent, 8494);
        let mut builder = SpatialCellIndexBuilder::new(grid);

        // Large geometry spanning roughly half the extent (many cells)
        let large_bbox = BBox2D::new(-117.600, -117.585, 47.650, 47.656);
        let ip = ItemPointer::new(1, 1);
        builder.add(ip, &large_bbox);

        let index = builder.build();

        // Query a small bbox at the far-right edge of the large geom,
        // far from its centroid. Must still find the node.
        let edge_query = BBox2D::new(-117.586, -117.585, 47.655, 47.656);
        let nodes = index.nodes_in_bbox(&edge_query);
        assert!(
            nodes.contains(&ip),
            "large geom should be found when querying its edge, got {} nodes",
            nodes.len()
        );
    }

    #[test]
    fn test_multi_cell_assignment_duplicates() {
        let extent = buildings_extent();
        let grid = SpatialGridConfig::from_extent(extent, 8494);

        // Build a bbox that spans exactly 4 cells (2x2 block)
        let cell_w = (extent.xmax - extent.xmin) / grid.cols as f32;
        let cell_h = (extent.ymax - extent.ymin) / grid.rows as f32;
        // Span from middle of cell (1,1) to middle of cell (2,2)
        let bbox = BBox2D::new(
            extent.xmin + 1.5 * cell_w,
            extent.xmin + 2.5 * cell_w,
            extent.ymin + 1.5 * cell_h,
            extent.ymin + 2.5 * cell_h,
        );

        let mut builder = SpatialCellIndexBuilder::new(grid);
        let ip = ItemPointer::new(42, 1);
        builder.add(ip, &bbox);
        let index = builder.build();

        // Node should appear in 4 cells (2x2)
        assert_eq!(
            index.node_pointers.len(),
            4,
            "node spanning 2x2 cells should appear 4 times in CSR"
        );

        // nodes_in_bbox should return the node deduplicated
        let found = index.nodes_in_bbox(&bbox);
        assert_eq!(
            found.iter().filter(|&&p| p == ip).count(),
            1,
            "nodes_in_bbox should deduplicate"
        );
    }
}
