// Partition metadata for the geo-vec index
// This is stored in the index meta page

use crate::partition::BBox2D;
use rkyv::{Archive, Deserialize, Serialize};

/// Partition metadata stored in the index
/// Each partition has its own bounding box and graph
#[derive(Clone, Debug, PartialEq, Archive, Deserialize, Serialize)]
#[archive(check_bytes)]
pub struct PartitionMetadata {
    /// Partition ID (0-based)
    pub id: u32,
    /// Bounding box of all points in this partition
    pub bbox: BBox2D,
    /// Root page of this partition's graph
    pub root_page: u32,
    /// Number of nodes in this partition
    pub row_count: u64,
    /// First node's item pointer (for traversal)
    pub first_node: Option<(u32, u16)>, // (block_number, offset)
}

impl PartitionMetadata {
    pub fn new(id: u32, bbox: BBox2D) -> Self {
        Self {
            id,
            bbox,
            root_page: 0,
            row_count: 0,
            first_node: None,
        }
    }
}

/// Global bounding box - union of all partition bboxes
#[derive(Clone, Debug, PartialEq, Archive, Deserialize, Serialize)]
#[repr(C)]
pub struct GlobalBBox {
    pub xmin: f32,
    pub xmax: f32,
    pub ymin: f32,
    pub ymax: f32,
}

impl GlobalBBox {
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

    pub fn expand(&mut self, bbox: &BBox2D) {
        self.xmin = self.xmin.min(bbox.xmin);
        self.xmax = self.xmax.max(bbox.xmax);
        self.ymin = self.ymin.min(bbox.ymin);
        self.ymax = self.ymax.max(bbox.ymax);
    }

    pub fn to_bbox2d(&self) -> BBox2D {
        BBox2D::new(self.xmin, self.xmax, self.ymin, self.ymax)
    }
}
