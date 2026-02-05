use pg_geo_vec_derive::{Readable, Writeable};
use rkyv::{Archive, Deserialize, Serialize};

use crate::access_method::labels::{Label, LabelSet};
use crate::access_method::node::{ReadableNode, WriteableNode};
use crate::util::{ItemPointer, ReadableBuffer, WritableBuffer};
use pgrx::PgRelation;
use std::collections::BTreeMap;

use crate::access_method::labels::LabelSetView;

/// Partition start nodes for the graph.
/// Each spatial partition has its own entry point into the graph.
/// This enables partition-aware search that starts from nodes in relevant partitions.
#[derive(Clone, Debug, PartialEq, Eq, Archive, Deserialize, Serialize, Readable, Writeable)]
#[archive(check_bytes)]
pub struct PartitionStartNodes {
    /// Map from partition ID to the start node for that partition
    partition_nodes: BTreeMap<u32, ItemPointer>,
    /// Default starting node for the graph (used when no partition filter is specified)
    default_node: Option<ItemPointer>,
}

impl PartitionStartNodes {
    /// Create a new empty PartitionStartNodes
    pub fn new() -> Self {
        Self {
            partition_nodes: BTreeMap::new(),
            default_node: None,
        }
    }

    /// Set the start node for a partition. Returns the previous node if one existed.
    pub fn set_partition_start(&mut self, partition_id: u32, node: ItemPointer) -> Option<ItemPointer> {
        // Also update default node if this is the first partition
        if self.default_node.is_none() {
            self.default_node = Some(node);
        }
        self.partition_nodes.insert(partition_id, node)
    }

    /// Get start nodes for a set of partitions.
    /// Returns the start nodes for all specified partitions that have start nodes.
    /// If no partitions are specified or no partition nodes are found, returns the default node.
    pub fn get_for_partitions(&self, partition_ids: Option<&[u32]>) -> Vec<ItemPointer> {
        match partition_ids {
            Some(ids) if !ids.is_empty() => {
                let nodes: Vec<ItemPointer> = ids
                    .iter()
                    .filter_map(|id| self.partition_nodes.get(id).copied())
                    .collect();

                // If no partition nodes found, fall back to default
                if nodes.is_empty() {
                    self.default_node.iter().copied().collect()
                } else {
                    nodes
                }
            }
            _ => {
                // No partition filter - return default node if available
                self.default_node.iter().copied().collect()
            }
        }
    }

    /// Check if a partition has a start node
    pub fn has_partition(&self, partition_id: u32) -> bool {
        self.partition_nodes.contains_key(&partition_id)
    }

    /// Get the default start node
    pub fn default_node(&self) -> Option<ItemPointer> {
        self.default_node
    }

    /// Set the default start node
    pub fn set_default_node(&mut self, node: ItemPointer) {
        self.default_node = Some(node);
    }

    /// Get the start node for a specific partition
    pub fn get_partition_node(&self, partition_id: u32) -> Option<ItemPointer> {
        self.partition_nodes.get(&partition_id).copied()
    }

    /// Get all partition IDs that have start nodes
    pub fn partition_ids(&self) -> Vec<u32> {
        self.partition_nodes.keys().copied().collect()
    }

    /// Get the number of partitions with start nodes
    pub fn num_partitions(&self) -> usize {
        self.partition_nodes.len()
    }

    /// Get all start nodes (for debugging/iteration)
    pub fn get_all_nodes(&self) -> Vec<ItemPointer> {
        let mut nodes: Vec<ItemPointer> = self.partition_nodes.values().copied().collect();
        if let Some(default) = self.default_node {
            if !nodes.contains(&default) {
                nodes.push(default);
            }
        }
        nodes
    }

    /// Check if empty (no partitions and no default)
    pub fn is_empty(&self) -> bool {
        self.partition_nodes.is_empty() && self.default_node.is_none()
    }
}

impl Default for PartitionStartNodes {
    fn default() -> Self {
        Self::new()
    }
}

/// Start nodes for the graph.  For unlabeled vectorsets, this is a single node.  For
/// labeled vectorsets, this is a map of labels to nodes.
#[derive(Clone, Debug, PartialEq, Eq, Archive, Deserialize, Serialize, Readable, Writeable)]
#[archive(check_bytes)]
pub struct StartNodes {
    /// Default starting node for the graph.
    default_node: ItemPointer,
    /// Labeled starting nodes for the graph
    labeled_nodes: BTreeMap<Label, ItemPointer>,
}

impl StartNodes {
    pub fn new(default_node: ItemPointer) -> Self {
        Self {
            default_node,
            labeled_nodes: BTreeMap::new(),
        }
    }

    pub fn upsert(&mut self, label: Label, node: ItemPointer) -> Option<ItemPointer> {
        self.labeled_nodes.insert(label, node)
    }

    pub fn default_node(&self) -> ItemPointer {
        self.default_node
    }

    pub fn get_for_node(&self, labels: Option<&LabelSet>) -> Vec<ItemPointer> {
        if let Some(labels) = labels {
            labels
                .iter()
                .filter_map(|label| self.labeled_nodes.get(label).copied())
                .collect()
        } else {
            vec![self.default_node]
        }
    }

    pub fn contains(&self, label: Label) -> bool {
        self.labeled_nodes.contains_key(&label)
    }

    pub fn contains_all(&self, labels: Option<&LabelSet>) -> bool {
        match labels {
            Some(labels) => labels
                .iter()
                .all(|label| self.labeled_nodes.contains_key(label)),
            None => true,
        }
    }

    pub fn node_for_label(&self, label: Label) -> Option<ItemPointer> {
        self.labeled_nodes.get(&label).copied()
    }

    pub fn node_for_labels(&self, labels: &LabelSet) -> Vec<ItemPointer> {
        if labels.is_empty() {
            vec![self.default_node]
        } else {
            labels
                .iter()
                .filter_map(|label| self.labeled_nodes.get(label).copied())
                .collect()
        }
    }

    pub fn get_all_labeled_nodes(&self) -> Vec<(Option<Label>, ItemPointer)> {
        let mut nodes = vec![(None, self.default_node)];
        nodes.extend(
            self.labeled_nodes
                .iter()
                .map(|(label, node)| (Some(*label), *node)),
        );
        nodes
    }

    pub fn get_all_nodes(&self) -> Vec<ItemPointer> {
        let mut nodes = vec![self.default_node];
        nodes.extend(self.labeled_nodes.values().copied());
        nodes
    }
}
