//! Incremental spatial CSR maintenance: overflow append on INSERT, compact on VACUUM.
//!
//! Base CSR is immutable after `CREATE INDEX`. New nodes are recorded in a
//! `SpatialOverflowIndex` ChainTape blob pointed to by MetaPage. Scans union
//! base + overflow. Compact rebuilds CSR from the live graph and clears overflow.

use pgrx::*;

use crate::access_method::meta_page::MetaPage;
use crate::access_method::spatial_index::{
    SpatialCellIndex, SpatialCellIndexBuilder, SpatialGridConfig, SpatialOverflowIndex,
};
use crate::access_method::stats::WriteStats;
use crate::partition::BBox2D;
use crate::util::chain::{ChainItemReader, ChainTapeWriter};
use crate::util::page::PageType;
use crate::util::ItemPointer;

pub fn load_spatial_cell_index(index: &PgRelation, sci_start: ItemPointer) -> SpatialCellIndex {
    let mut stats = WriteStats::default();
    let mut reader = ChainItemReader::new(index, PageType::SpatialCellIndex, &mut stats);
    let mut buf: Vec<u8> = Vec::new();
    for item in reader.read(sci_start) {
        buf.extend_from_slice(item.get_data_slice());
    }
    SpatialCellIndex::deserialize_from_bytes(&buf)
}

pub fn load_spatial_overflow(
    index: &PgRelation,
    meta: &MetaPage,
) -> Option<SpatialOverflowIndex> {
    let start = meta.get_spatial_overflow_start()?;
    let mut stats = WriteStats::default();
    let mut reader = ChainItemReader::new(index, PageType::SpatialOverflow, &mut stats);
    let mut buf: Vec<u8> = Vec::new();
    for item in reader.read(start) {
        buf.extend_from_slice(item.get_data_slice());
    }
    Some(SpatialOverflowIndex::deserialize_from_bytes(&buf))
}

fn store_spatial_overflow(
    index: &PgRelation,
    meta: &mut MetaPage,
    overflow: &SpatialOverflowIndex,
) {
    let mut stats = WriteStats::default();
    let mut tape = ChainTapeWriter::new(index, PageType::SpatialOverflow, &mut stats);
    let start = tape.write(&overflow.serialize_to_bytes());
    meta.set_spatial_overflow_start(start);
    meta.set_spatial_overflow_entries(overflow.len() as u32);
}

fn store_spatial_cell_index(
    index: &PgRelation,
    meta: &mut MetaPage,
    cell_index: &SpatialCellIndex,
) {
    let mut stats = WriteStats::default();
    let mut tape = ChainTapeWriter::new(index, PageType::SpatialCellIndex, &mut stats);
    let start = tape.write(&cell_index.serialize_to_bytes());
    meta.set_spatial_cell_index_start(start);
}

/// After a graph node is inserted, record it in the spatial overflow (or create a CSR).
pub unsafe fn maintain_spatial_on_insert(
    index: &PgRelation,
    meta: &mut MetaPage,
    index_pointer: ItemPointer,
    bbox: &BBox2D,
) {
    if !meta.has_geometry() || bbox.is_empty() {
        return;
    }

    if let Some(sci_start) = meta.get_spatial_cell_index_start() {
        let cell_index = load_spatial_cell_index(index, sci_start);
        let mut overflow = load_spatial_overflow(index, meta).unwrap_or_default();
        overflow.append_node(&cell_index.grid, index_pointer, bbox);
        store_spatial_overflow(index, meta, &overflow);
        meta.store(index, false);

        let threshold = super::guc::TSV_SPATIAL_OVERFLOW_COMPACT_THRESHOLD
            .get()
            .max(1) as u32;
        if meta.get_spatial_overflow_entries() >= threshold {
            notice!(
                "Spatial overflow reached {} entries (threshold {}); compacting CSR",
                meta.get_spatial_overflow_entries(),
                threshold
            );
            crate::access_method::build::compact_spatial_cell_index(index, meta);
        }
        return;
    }

    // No base CSR yet (e.g. index built on empty table): create a one-node CSR.
    let grid = SpatialGridConfig::from_extent(*bbox, 1);
    let mut builder = SpatialCellIndexBuilder::new(grid);
    builder.add(index_pointer, bbox);
    let cell_index = builder.build();
    store_spatial_cell_index(index, meta, &cell_index);
    meta.clear_spatial_overflow();
    meta.store(index, false);
    notice!(
        "Created spatial cell index on first insert: {} cells, {} entries",
        cell_index.grid.num_cells(),
        cell_index.node_pointers.len()
    );
}

/// Compact if overflow is non-empty (VACUUM path).
pub unsafe fn maybe_compact_spatial_on_vacuum(index: &PgRelation, meta: &mut MetaPage) {
    if meta.get_spatial_overflow_entries() == 0 && meta.get_spatial_overflow_start().is_none() {
        return;
    }
    notice!(
        "Compacting spatial CSR ({} overflow entries)",
        meta.get_spatial_overflow_entries()
    );
    crate::access_method::build::compact_spatial_cell_index(index, meta);
}

#[cfg(any(test, feature = "pg_test"))]
#[pgrx::pg_schema]
mod tests {
    use pgrx::prelude::*;

    /// After CREATE INDEX, a new INSERT must be visible to spatial+vector queries
    /// without REINDEX (via overflow postings).
    #[pg_test]
    fn insert_visible_via_spatial_overflow() -> spi::Result<()> {
        Spi::run("CREATE EXTENSION IF NOT EXISTS postgis;")?;
        Spi::run("CREATE EXTENSION IF NOT EXISTS vector;")?;

        Spi::run(
            "DROP TABLE IF EXISTS overflow_ins_test;
             CREATE TABLE overflow_ins_test (
                id int PRIMARY KEY,
                geom geometry(Point, 4326),
                embedding vector(2)
             );
             INSERT INTO overflow_ins_test VALUES
                (1, ST_SetSRID(ST_MakePoint(0.1, 0.1), 4326), '[1,0]'),
                (2, ST_SetSRID(ST_MakePoint(0.2, 0.2), 4326), '[0.9,0.1]'),
                (3, ST_SetSRID(ST_MakePoint(0.9, 0.9), 4326), '[-1,0]');
             CREATE INDEX overflow_ins_test_idx ON overflow_ins_test
                USING geo_vec (embedding vector_l2_ops, geom geometry_geo_vec_ops)
                WITH (storage_layout = plain);",
        )?;

        Spi::run(
            "INSERT INTO overflow_ins_test VALUES
                (4, ST_SetSRID(ST_MakePoint(0.15, 0.15), 4326), '[0.95,0.05]');",
        )?;

        let has_new = Spi::get_one::<i64>(
            "SELECT count(*) FROM (
                SELECT id FROM overflow_ins_test
                WHERE geom && ST_MakeEnvelope(0.0, 0.0, 0.3, 0.3, 4326)
                ORDER BY embedding <-> '[1,0]'::vector
                LIMIT 10
             ) t WHERE id = 4;",
        )?
        .unwrap_or(0);
        assert_eq!(
            has_new, 1,
            "inserted row id=4 must be visible via spatial overflow without REINDEX"
        );

        Spi::run("DROP TABLE overflow_ins_test;")?;
        Ok(())
    }
}
