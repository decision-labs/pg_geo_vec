// Spatial partitioning module for pg_geo_vec
// Implements RTree-like partitioning for composite vector + spatial index

use rkyv::{Archive, Deserialize, Serialize};

/// Bounding box structure (similar to PostGIS BOX2DF)
/// 16 bytes: 4 x f32 coordinates (xmin, xmax, ymin, ymax)
#[derive(Copy, Clone, Debug, PartialEq, Archive, Deserialize, Serialize)]
#[repr(C)]
pub struct BBox2D {
    pub xmin: f32,
    pub xmax: f32,
    pub ymin: f32,
    pub ymax: f32,
}

impl BBox2D {
    pub fn new(xmin: f32, xmax: f32, ymin: f32, ymax: f32) -> Self {
        Self {
            xmin,
            xmax,
            ymin,
            ymax,
        }
    }

    pub fn empty() -> Self {
        Self {
            xmin: f32::MAX,
            xmax: f32::MIN,
            ymin: f32::MAX,
            ymax: f32::MIN,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.xmin > self.xmax || self.ymin > self.ymax
    }

    /// Check if this bbox overlaps with another (spatial overlap test)
    /// Used for both partition selection and node filtering
    #[inline(always)]
    pub fn overlaps(&self, other: &BBox2D) -> bool {
        !(self.xmax < other.xmin
            || other.xmax < self.xmin
            || self.ymax < other.ymin
            || other.ymax < self.ymin)
    }

    #[inline(always)]
    pub fn contains(&self, other: &BBox2D) -> bool {
        self.xmin <= other.xmin
            && self.xmax >= other.xmax
            && self.ymin <= other.ymin
            && self.ymax >= other.ymax
    }

    #[inline(always)]
    pub fn union(&self, other: &BBox2D) -> BBox2D {
        BBox2D::new(
            self.xmin.min(other.xmin),
            self.xmax.max(other.xmax),
            self.ymin.min(other.ymin),
            self.ymax.max(other.ymax),
        )
    }

    pub fn area(&self) -> f32 {
        if self.is_empty() {
            0.0
        } else {
            (self.xmax - self.xmin) * (self.ymax - self.ymin)
        }
    }

    pub fn union_area(&self, other: &BBox2D) -> f32 {
        self.union(other).area()
    }

    /// Expand bbox to include a point
    #[inline(always)]
    pub fn expand(&mut self, x: f32, y: f32) {
        self.xmin = self.xmin.min(x);
        self.xmax = self.xmax.max(x);
        self.ymin = self.ymin.min(y);
        self.ymax = self.ymax.max(y);
    }
}

/// Partition metadata stored in the index meta page
#[derive(Debug, Clone)]
pub struct Partition {
    pub id: u32,
    pub bbox: BBox2D,
    pub row_count: u64,
}

impl Partition {
    pub fn new(id: u32, bbox: BBox2D) -> Self {
        Self {
            id,
            bbox,
            row_count: 0,
        }
    }
}

/// Partition manager for spatial partitioning
pub struct PartitionManager {
    partitions: Vec<Partition>,
}

impl PartitionManager {
    pub fn new(_max_partitions: u32) -> Self {
        Self {
            partitions: Vec::new(),
        }
    }

    pub fn add_partition(&mut self, bbox: BBox2D) -> u32 {
        let id = self.partitions.len() as u32;
        self.partitions.push(Partition::new(id, bbox));
        id
    }

    pub fn find_partitions(&self, query_bbox: &BBox2D) -> Vec<u32> {
        self.partitions
            .iter()
            .filter(|p| p.bbox.overlaps(query_bbox))
            .map(|p| p.id)
            .collect()
    }

    pub fn len(&self) -> usize {
        self.partitions.len()
    }

    pub fn partitions(&self) -> &[Partition] {
        &self.partitions
    }
}

/// Grid-based spatial partitioning
/// Divides space into a uniform grid of cells
pub struct GridPartitioner {
    pub xmin: f32,
    pub ymin: f32,
    pub xmax: f32,
    pub ymax: f32,
    pub grid_cols: u32,
    pub grid_rows: u32,
}

impl GridPartitioner {
    /// Create a new grid partitioner covering the given bounds
    pub fn new(bbox: BBox2D, num_partitions: u32) -> Self {
        // Calculate grid dimensions to approximate square cells
        let bbox_width = (bbox.xmax - bbox.xmin).max(1e-6);
        let bbox_height = (bbox.ymax - bbox.ymin).max(1e-6);
        let aspect_ratio = bbox_width as f64 / bbox_height as f64;

        // Calculate cols and rows to get approximately num_partitions cells
        let cols = ((num_partitions as f64 * aspect_ratio).sqrt() as u32).max(1);
        let rows = ((num_partitions as f64 / aspect_ratio).sqrt() as u32).max(1);

        // Expand bounds slightly to ensure all points are covered
        let expanded = bbox.expand_bounds(0.01);

        Self {
            xmin: expanded.xmin,
            ymin: expanded.ymin,
            xmax: expanded.xmax,
            ymax: expanded.ymax,
            grid_cols: cols,
            grid_rows: rows,
        }
    }

    /// Create a GridPartitioner from stored configuration (used when loading from MetaPage)
    pub fn from_config(bbox: BBox2D, grid_cols: u32, grid_rows: u32) -> Self {
        Self {
            xmin: bbox.xmin,
            ymin: bbox.ymin,
            xmax: bbox.xmax,
            ymax: bbox.ymax,
            grid_cols,
            grid_rows,
        }
    }

    /// Get the partition ID for a bbox's centroid
    #[inline(always)]
    pub fn partition_for_bbox(&self, bbox: &BBox2D) -> u32 {
        // Use centroid of bbox for partition assignment
        let cx = (bbox.xmin + bbox.xmax) / 2.0;
        let cy = (bbox.ymin + bbox.ymax) / 2.0;
        self.cell_index(cx, cy)
    }

    /// Get the cell index for a given point
    #[inline(always)]
    pub fn cell_index(&self, x: f32, y: f32) -> u32 {
        let width = (self.xmax - self.xmin).max(1e-6);
        let height = (self.ymax - self.ymin).max(1e-6);

        let col = ((x - self.xmin) / width * self.grid_cols as f32)
            .clamp(0.0, self.grid_cols as f32 - 1.0) as u32;
        let row = ((y - self.ymin) / height * self.grid_rows as f32)
            .clamp(0.0, self.grid_rows as f32 - 1.0) as u32;
        row * self.grid_cols + col
    }

    /// Get the bounding box for a cell
    pub fn cell_bbox(&self, cell_index: u32) -> BBox2D {
        let col = cell_index % self.grid_cols;
        let row = cell_index / self.grid_cols;

        let cell_width = (self.xmax - self.xmin) / self.grid_cols as f32;
        let cell_height = (self.ymax - self.ymin) / self.grid_rows as f32;

        BBox2D::new(
            self.xmin + col as f32 * cell_width,
            self.xmin + (col + 1) as f32 * cell_width,
            self.ymin + row as f32 * cell_height,
            self.ymin + (row + 1) as f32 * cell_height,
        )
    }

    /// Get the bounding box for a point's containing cell
    #[inline(always)]
    pub fn point_cell_bbox(&self, x: f32, y: f32) -> BBox2D {
        self.cell_bbox(self.cell_index(x, y))
    }

    /// Find all cells that overlap with a query bbox
    pub fn overlapping_cells(&self, query_bbox: &BBox2D) -> Vec<u32> {
        let width = (self.xmax - self.xmin).max(1e-6);
        let height = (self.ymax - self.ymin).max(1e-6);

        // Compute inclusive cell ranges and clamp to valid cell coordinates.
        let min_col = (((query_bbox.xmin - self.xmin) / width * self.grid_cols as f32).floor()
            as i32)
            .clamp(0, self.grid_cols as i32 - 1) as u32;
        let max_col = (((query_bbox.xmax - self.xmin) / width * self.grid_cols as f32).floor()
            as i32)
            .clamp(0, self.grid_cols as i32 - 1) as u32;
        let min_row = (((query_bbox.ymin - self.ymin) / height * self.grid_rows as f32).floor()
            as i32)
            .clamp(0, self.grid_rows as i32 - 1) as u32;
        let max_row = (((query_bbox.ymax - self.ymin) / height * self.grid_rows as f32).floor()
            as i32)
            .clamp(0, self.grid_rows as i32 - 1) as u32;

        let mut cells = Vec::new();
        for row in min_row..=max_row {
            for col in min_col..=max_col {
                let cell_idx = row * self.grid_cols + col;
                if self.cell_bbox(cell_idx).overlaps(query_bbox) {
                    cells.push(cell_idx);
                }
            }
        }
        cells
    }

    /// Get total number of cells
    pub fn num_cells(&self) -> u32 {
        self.grid_cols * self.grid_rows
    }
}

impl BBox2D {
    /// Expand the bounds by a small percentage
    pub fn expand_bounds(&self, percent: f32) -> Self {
        let dx = ((self.xmax - self.xmin) * percent).max(1e-6);
        let dy = ((self.ymax - self.ymin) * percent).max(1e-6);
        Self::new(
            self.xmin - dx,
            self.xmax + dx,
            self.ymin - dy,
            self.ymax + dy,
        )
    }
}
