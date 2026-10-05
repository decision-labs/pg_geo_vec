use std::collections::BinaryHeap;

use pgrx::{pg_sys::InvalidOffsetNumber, *};

use crate::{
    access_method::{
        graph::neighbor_store::GraphNeighborStore,
        labels::LabeledVector,
        meta_page::MetaPage,
        pg_vector::PgVector,
        sbq::storage::SbqSpeedupStorage,
        spatial_index::SpatialCellIndex,
        spatial_maintain,
        storage_common::get_index_vector_attribute,
    },
    partition::BBox2D,
    util::{
        buffer::PinnedBufferShare,
        ports::pgstat_count_index_scan,
        table_slot::TableSlot,
        HeapPointer, IndexPointer,
    },
};

use super::{
    distance::DistanceFn,
    graph::{Graph, ListSearchResult},
    labels::LabelSetView,
    plain::{
        node::PlainNode,
        storage::{PlainStorage, PlainStorageLsnPrivateData},
        PlainDistanceMeasure,
    },
    sbq::{
        node::SbqNode,
        quantize::SbqQuantizer,
        storage::SbqSpeedupStorageLsnPrivateData,
        SbqMeans, SbqSearchDistanceMeasure,
    },
    stats::{QuantizerStats},
    storage::{ArchivedData, Storage, StorageType},
};

use super::node::ReadableNode;

/// Pre-computed spatial scan results, sorted by distance (ascending).
struct SpatialScanState {
    results: Vec<(f32, HeapPointer, IndexPointer)>,
    position: usize,
}

impl SpatialScanState {
    fn next(&mut self) -> Option<(HeapPointer, IndexPointer)> {
        while self.position < self.results.len() {
            let (_, hp, ip) = self.results[self.position];
            self.position += 1;
            if hp.offset != InvalidOffsetNumber {
                return Some((hp, ip));
            }
        }
        None
    }
}

/* Be very careful not to transfer PgRelations in the state, as they can change between calls. That means we shouldn't be
using lifetimes here. Everything should be owned */
enum StorageState {
    SbqSpeedup(
        SbqQuantizer,
        TSVResponseIterator<SbqSearchDistanceMeasure, SbqSpeedupStorageLsnPrivateData>,
    ),
    Plain(TSVResponseIterator<PlainDistanceMeasure, PlainStorageLsnPrivateData>),
    SpatialScan(SpatialScanState),
}

/* no lifetime usage here. */
struct TSVScanState {
    storage: *mut StorageState,
    distance_fn: Option<DistanceFn>,
    meta_page: MetaPage,
    last_buffer: Option<PinnedBufferShare>,
}

impl TSVScanState {
    fn new(meta_page: &MetaPage) -> Self {
        Self {
            storage: std::ptr::null_mut(),
            distance_fn: None,
            meta_page: meta_page.clone(),
            last_buffer: None,
        }
    }

    fn initialize(
        &mut self,
        index: &PgRelation,
        heap: &PgRelation,
        query: LabeledVector,
        search_list_size: usize,
        query_bbox: Option<BBox2D>,
        snapshot: pg_sys::Snapshot,
    ) {
        let meta_page = MetaPage::fetch(index);
        let distance = meta_page.get_distance_function();

        // Three-way routing for spatial queries
        if let Some(ref qbbox) = query_bbox {
            if let Some(sci_start) = meta_page.get_spatial_cell_index_start() {
                let cell_index = spatial_maintain::load_spatial_cell_index(index, sci_start);
                let overflow = spatial_maintain::load_spatial_overflow(index, &meta_page);
                let seeds_per_cell =
                    super::guc::TSV_SPATIAL_SEEDS_PER_CELL.get().max(1) as usize;
                let (seeds, total_candidates) = cell_index.sample_seeds_in_bbox_with_overflow(
                    qbbox,
                    seeds_per_cell,
                    overflow.as_ref(),
                );
                let threshold =
                    super::guc::TSV_SPATIAL_BRUTE_FORCE_THRESHOLD.get().max(0) as usize;

                if total_candidates <= threshold {
                    // Small candidate set → brute-force (100% recall)
                    let store_type = Self::initialize_spatial_scan_from_index(
                        index,
                        heap,
                        &meta_page,
                        &query,
                        qbbox,
                        &cell_index,
                        overflow.as_ref(),
                        distance,
                        snapshot,
                    );
                    self.storage = PgMemoryContexts::CurrentMemoryContext
                        .leak_and_drop_on_delete(store_type);
                    self.distance_fn = Some(distance);
                    return;
                } else {
                    // Large candidate set → hybrid spatial-seeded graph search
                    debug1!(
                        "Spatial hybrid: {} total candidates > {} threshold, using {} seeds from {} cells (overflow={})",
                        total_candidates,
                        threshold,
                        seeds.len(),
                        cell_index.grid.bbox_to_cells(qbbox).len(),
                        overflow.as_ref().map(|o| o.len()).unwrap_or(0)
                    );
                    let store_type = Self::initialize_spatial_seeded_search(
                        index,
                        heap,
                        &meta_page,
                        query,
                        search_list_size,
                        query_bbox,
                        seeds,
                    );
                    self.storage = PgMemoryContexts::CurrentMemoryContext
                        .leak_and_drop_on_delete(store_type);
                    self.distance_fn = Some(distance);
                    return;
                }
            }
        }

        // Fall through to graph-based search (no bbox or no spatial cell index)
        let storage = meta_page.get_storage_type();

        let store_type = match storage {
            StorageType::Plain => {
                let stats = QuantizerStats::default();
                let bq = PlainStorage::load_for_search(index, heap, &meta_page);
                let it = TSVResponseIterator::new(
                    &bq,
                    index,
                    query,
                    search_list_size,
                    meta_page,
                    stats,
                    query_bbox,
                );
                StorageState::Plain(it)
            }
            StorageType::SbqCompression => {
                let mut stats = QuantizerStats::default();
                let quantizer = unsafe { SbqMeans::load(index, &meta_page, &mut stats) };
                let bq = SbqSpeedupStorage::load_for_search(index, heap, &quantizer, &meta_page);
                let it = TSVResponseIterator::new(
                    &bq,
                    index,
                    query,
                    search_list_size,
                    meta_page,
                    stats,
                    query_bbox,
                );
                StorageState::SbqSpeedup(quantizer, it)
            }
        };

        self.storage = PgMemoryContexts::CurrentMemoryContext.leak_and_drop_on_delete(store_type);
        self.distance_fn = Some(distance);
    }

    /// Brute-force spatial scan using a pre-loaded cell index (+ optional overflow).
    /// For SBQ storage, reads full vectors from the heap for exact distances.
    fn initialize_spatial_scan_from_index(
        index: &PgRelation,
        heap: &PgRelation,
        meta_page: &MetaPage,
        query: &LabeledVector,
        query_bbox: &BBox2D,
        cell_index: &SpatialCellIndex,
        overflow: Option<&super::spatial_index::SpatialOverflowIndex>,
        distance_fn: DistanceFn,
        snapshot: pg_sys::Snapshot,
    ) -> StorageState {
        let node_pointers = cell_index.nodes_in_bbox_with_overflow(query_bbox, overflow);
        let query_vec_full = query.vec().to_full_slice();

        let mut results: Vec<(f32, HeapPointer, IndexPointer)> =
            Vec::with_capacity(node_pointers.len());

        let storage_type = meta_page.get_storage_type();
        let has_labels = meta_page.has_labels();

        match storage_type {
            StorageType::Plain => {
                let query_vec = query.vec().to_index_slice();
                let mut read_stats = crate::access_method::stats::InsertStats::default();
                for ip in &node_pointers {
                    let rn = unsafe { PlainNode::read(index, *ip, &mut read_stats) };
                    let node = rn.get_archived_node();
                    if node.is_deleted() {
                        continue;
                    }
                    // Exact bbox filter: grid cells are coarse, so filter nodes
                    // whose actual bbox doesn't overlap the query bbox.
                    let node_bbox = BBox2D {
                        xmin: node.bbox.xmin,
                        xmax: node.bbox.xmax,
                        ymin: node.bbox.ymin,
                        ymax: node.bbox.ymax,
                    };
                    if !node_bbox.is_empty() && !node_bbox.overlaps(query_bbox) {
                        continue;
                    }
                    let heap_pointer = node.get_heap_item_pointer();
                    let node_vector = node.vector.as_slice();
                    let dist = distance_fn(node_vector, query_vec);
                    results.push((dist, heap_pointer, *ip));
                }
            }
            StorageType::SbqCompression => {
                // For SBQ, read full vectors from the heap for exact distance computation.
                // This gives 100% recall for the brute-force path.
                let mut read_stats = crate::access_method::stats::InsertStats::default();
                let heap_attr = get_index_vector_attribute(index);

                for ip in &node_pointers {
                    let rn = unsafe { SbqNode::read(index, *ip, has_labels, &mut read_stats) };
                    let node = rn.get_archived_node();
                    if node.is_deleted() {
                        continue;
                    }
                    // Exact bbox filter: grid cells are coarse, so filter nodes
                    // whose actual bbox doesn't overlap the query bbox.
                    let node_bbox = node.get_bbox();
                    if !node_bbox.is_empty() && !node_bbox.overlaps(query_bbox) {
                        continue;
                    }
                    let heap_pointer = node.get_heap_item_pointer();

                    let slot_opt = unsafe {
                        TableSlot::from_index_heap_pointer(
                            heap,
                            heap_pointer,
                            snapshot,
                            &mut read_stats.greedy_search_stats,
                        )
                    };
                    if let Some(slot) = slot_opt {
                        let datum = unsafe {
                            slot.get_attribute(heap_attr)
                                .expect("vector attribute should exist in the heap")
                        };
                        let vec = unsafe { PgVector::from_datum(datum, meta_page, false, true) };
                        let dist = distance_fn(vec.to_full_slice(), query_vec_full);
                        results.push((dist, heap_pointer, *ip));
                    }
                }
            }
        }

        results.sort_by(|a, b| a.0.total_cmp(&b.0));

        debug1!(
            "Spatial brute-force: {} candidates from {} cells, {} after filtering deleted",
            node_pointers.len(),
            cell_index.grid.bbox_to_cells(query_bbox).len(),
            results.len()
        );

        StorageState::SpatialScan(SpatialScanState {
            results,
            position: 0,
        })
    }

    /// Hybrid spatial-seeded graph search: use spatial seeds as entry points
    /// into the DiskANN graph, with spatial post-filtering during traversal.
    fn initialize_spatial_seeded_search(
        index: &PgRelation,
        heap: &PgRelation,
        meta_page: &MetaPage,
        query: LabeledVector,
        search_list_size: usize,
        query_bbox: Option<BBox2D>,
        seeds: Vec<IndexPointer>,
    ) -> StorageState {
        let storage_type = meta_page.get_storage_type();

        match storage_type {
            StorageType::Plain => {
                let stats = QuantizerStats::default();
                let bq = PlainStorage::load_for_search(index, heap, meta_page);
                let it = TSVResponseIterator::new_with_seeds(
                    &bq,
                    index,
                    query,
                    search_list_size,
                    stats,
                    query_bbox,
                    seeds,
                );
                StorageState::Plain(it)
            }
            StorageType::SbqCompression => {
                let mut stats = QuantizerStats::default();
                let quantizer = unsafe { SbqMeans::load(index, meta_page, &mut stats) };
                let bq =
                    SbqSpeedupStorage::load_for_search(index, heap, &quantizer, meta_page);
                let it = TSVResponseIterator::new_with_seeds(
                    &bq,
                    index,
                    query,
                    search_list_size,
                    stats,
                    query_bbox,
                    seeds,
                );
                StorageState::SbqSpeedup(quantizer, it)
            }
        }
    }
}

struct ResortData {
    heap_pointer: HeapPointer,
    index_pointer: IndexPointer,
    distance: f32,
}

impl PartialEq for ResortData {
    fn eq(&self, other: &Self) -> bool {
        self.heap_pointer == other.heap_pointer
    }
}

impl PartialOrd for ResortData {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Eq for ResortData {}

impl Ord for ResortData {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        //notice the reverse here. Other is the one that is being compared to self
        //this allows us to have a min heap
        other.distance.total_cmp(&self.distance)
    }
}

struct StreamingStats {
    count: i32,
    mean: f32,
    m2: f32,
    max_distance: f32,
}

impl StreamingStats {
    fn new(_resort_size: usize) -> Self {
        Self {
            count: 0,
            mean: 0.0,
            m2: 0.0,
            max_distance: 0.0,
        }
    }

    fn update_base_stats(&mut self, distance: f32) {
        if distance == 0.0 {
            return;
        }
        self.count += 1;
        let delta = distance - self.mean;
        self.mean += delta / self.count as f32;
        let delta2 = distance - self.mean;
        self.m2 += delta * delta2;
    }

    #[allow(dead_code)]
    fn variance(&self) -> f32 {
        if self.count < 2 {
            return 0.0;
        }
        self.m2 / (self.count - 1) as f32
    }

    fn update(&mut self, distance: f32, diff: f32) {
        //base stats only on first resort_size elements
        self.update_base_stats(diff);
        self.max_distance = self.max_distance.max(distance);
    }
}

struct TSVResponseIterator<QDM, PD> {
    lsr: ListSearchResult<QDM, PD>,
    search_list_size: usize,
    meta_page: MetaPage,
    quantizer_stats: QuantizerStats,
    resort_size: usize,
    resort_buffer: BinaryHeap<ResortData>,
    streaming_stats: StreamingStats,
    next_calls: i32,
    next_calls_with_resort: i32,
    full_distance_comparisons: i32,
    has_label_filter: bool,
    query_bbox: Option<BBox2D>,
}

impl<QDM, PD> TSVResponseIterator<QDM, PD> {
    fn new<S: Storage<QueryDistanceMeasure = QDM, LSNPrivateData = PD>>(
        storage: &S,
        index: &PgRelation,
        query: LabeledVector,
        search_list_size: usize,
        //FIXME?
        _meta_page: MetaPage,
        quantizer_stats: QuantizerStats,
        query_bbox: Option<BBox2D>,
    ) -> Self {
        let mut meta_page = MetaPage::fetch(index);
        let mut graph = Graph::new(GraphNeighborStore::Disk, &mut meta_page);

        let has_label_filter = query.labels().is_some_and(|labels| !labels.is_empty());

        let lsr = graph.greedy_search_streaming_init(
            query,
            search_list_size,
            storage,
        );
        let resort_size = super::guc::TSV_RESORT_SIZE.get() as usize;

        Self {
            search_list_size,
            lsr,
            meta_page,
            quantizer_stats,
            resort_size,
            resort_buffer: BinaryHeap::with_capacity(resort_size),
            streaming_stats: StreamingStats::new(resort_size),
            next_calls: 0,
            next_calls_with_resort: 0,
            full_distance_comparisons: 0,
            has_label_filter,
            query_bbox,
        }
    }

    /// Create a TSVResponseIterator using explicit seed nodes instead of MetaPage start nodes.
    /// Used by the hybrid spatial-seeded graph search path.
    fn new_with_seeds<S: Storage<QueryDistanceMeasure = QDM, LSNPrivateData = PD>>(
        storage: &S,
        index: &PgRelation,
        query: LabeledVector,
        search_list_size: usize,
        quantizer_stats: QuantizerStats,
        query_bbox: Option<BBox2D>,
        seeds: Vec<IndexPointer>,
    ) -> Self {
        let mut meta_page = MetaPage::fetch(index);
        let mut graph = Graph::new(GraphNeighborStore::Disk, &mut meta_page);

        let has_label_filter = query.labels().is_some_and(|labels| !labels.is_empty());

        let lsr = graph.greedy_search_streaming_init_with_seeds(
            seeds,
            query,
            search_list_size,
            storage,
        );
        let resort_size = super::guc::TSV_RESORT_SIZE.get() as usize;

        Self {
            search_list_size,
            lsr,
            meta_page,
            quantizer_stats,
            resort_size,
            resort_buffer: BinaryHeap::with_capacity(resort_size),
            streaming_stats: StreamingStats::new(resort_size),
            next_calls: 0,
            next_calls_with_resort: 0,
            full_distance_comparisons: 0,
            has_label_filter,
            query_bbox,
        }
    }
}

impl<QDM, PD> TSVResponseIterator<QDM, PD> {
    fn next<S: Storage<QueryDistanceMeasure = QDM, LSNPrivateData = PD>>(
        &mut self,
        storage: &S,
    ) -> Option<(HeapPointer, IndexPointer)> {
        self.next_calls += 1;
        let mut graph = Graph::new(GraphNeighborStore::Disk, &mut self.meta_page);

        /* Iterate until we find a non-deleted tuple that passes spatial filter */
        loop {
            graph.greedy_search_iterate(
                &mut self.lsr,
                self.search_list_size,
                !self.has_label_filter,
                None,
                storage,
            );

            let item = self.lsr.consume(storage);

            match item {
                Some((heap_pointer, index_pointer, node_bbox)) => {
                    if heap_pointer.offset == InvalidOffsetNumber {
                        /* deleted tuple */
                        continue;
                    }
                    // Spatial filter: skip nodes outside query bbox
                    if let Some(ref qbbox) = self.query_bbox {
                        if !node_bbox.is_empty() && !node_bbox.overlaps(qbbox) {
                            continue;
                        }
                    }
                    return Some((heap_pointer, index_pointer));
                }
                None => {
                    return None;
                }
            }
        }
    }

    fn next_with_resort<S: Storage<QueryDistanceMeasure = QDM, LSNPrivateData = PD>>(
        &mut self,
        scan: &PgBox<pg_sys::IndexScanDescData>,
        _index: &PgRelation,
        storage: &S,
    ) -> Option<(HeapPointer, IndexPointer)> {
        self.next_calls_with_resort += 1;
        if self.resort_buffer.capacity() == 0 {
            return self.next(storage);
        }

        while self.resort_buffer.len() < self.resort_size {
            match self.next(storage) {
                Some((heap_pointer, index_pointer)) => {
                    self.full_distance_comparisons += 1;
                    let distance = storage.get_full_distance_for_resort(
                        scan,
                        self.lsr.sdm.as_ref().unwrap(),
                        index_pointer,
                        heap_pointer,
                        &self.meta_page,
                        &mut self.lsr.stats,
                    );

                    match distance {
                        None => {
                            /* No entry found in heap */
                            continue;
                        }
                        Some(distance) => {
                            if self.resort_buffer.len() > 1 {
                                self.streaming_stats
                                    .update(distance, distance - self.streaming_stats.max_distance);
                            }

                            self.resort_buffer.push(ResortData {
                                heap_pointer,
                                index_pointer,
                                distance,
                            });
                        }
                    }
                }
                None => {
                    break;
                }
            }
        }

        self.resort_buffer
            .pop()
            .map(|rd| (rd.heap_pointer, rd.index_pointer))
    }
}

#[pg_guard]
pub extern "C-unwind" fn ambeginscan(
    index_relation: pg_sys::Relation,
    nkeys: ::std::os::raw::c_int,
    norderbys: ::std::os::raw::c_int,
) -> pg_sys::IndexScanDesc {
    let mut scandesc: PgBox<pg_sys::IndexScanDescData> = unsafe {
        PgBox::from_pg(pg_sys::RelationGetIndexScan(
            index_relation,
            nkeys,
            norderbys,
        ))
    };
    let indexrel = unsafe { PgRelation::from_pg(index_relation) };
    let meta_page = MetaPage::fetch(&indexrel);

    unsafe {
        pgstat_count_index_scan(index_relation, indexrel);
    }

    let state: TSVScanState = TSVScanState::new(&meta_page);
    scandesc.opaque =
        PgMemoryContexts::CurrentMemoryContext.leak_and_drop_on_delete(state) as void_mut_ptr;

    scandesc.into_pg()
}

#[pg_guard]
pub extern "C-unwind" fn amrescan(
    scan: pg_sys::IndexScanDesc,
    keys: pg_sys::ScanKey,
    nkeys: ::std::os::raw::c_int,
    orderbys: pg_sys::ScanKey,
    norderbys: ::std::os::raw::c_int,
) {
    assert_eq!(norderbys, 1, "Expected a single order-by key");
    // nkeys can be 0 (no filter), 1 (label OR geometry), or 2 (label AND geometry)
    assert!(nkeys <= 2, "Expected 0, 1, or 2 keys");

    let mut scan: PgBox<pg_sys::IndexScanDescData> = unsafe { PgBox::from_pg(scan) };
    let indexrel = unsafe { PgRelation::from_pg(scan.indexRelation) };
    let heaprel = unsafe { PgRelation::from_pg(scan.heapRelation) };

    if nkeys > 0 {
        scan.xs_recheck = true;
    }

    let orderby_keys = unsafe {
        std::slice::from_raw_parts(orderbys as *const pg_sys::ScanKeyData, norderbys as _)
    };
    let all_keys =
        unsafe { std::slice::from_raw_parts(keys as *const pg_sys::ScanKeyData, nkeys as _) };

    let search_list_size = super::guc::TSV_QUERY_SEARCH_LIST_SIZE.get() as usize;

    let state = unsafe { (scan.opaque as *mut TSVScanState).as_mut() }.expect("no scandesc state");

    // Separate geometry keys (strategy 6) from label keys (strategy 1)
    let mut label_keys: Vec<pg_sys::ScanKeyData> = Vec::new();
    let mut query_bbox: Option<BBox2D> = None;

    for key in all_keys {
        if key.sk_strategy == 6 {
            // Geometry && operator — extract bbox from the geometry datum
            crate::partition::postgis::ensure_postgis_loaded();
            query_bbox = unsafe { crate::partition::postgis::postgis_extract_bbox(key.sk_argument) };
        } else {
            label_keys.push(*key);
        }
    }

    let query = unsafe {
        LabeledVector::from_scan_key_data(&label_keys, orderby_keys, &state.meta_page)
    };

    state.initialize(&indexrel, &heaprel, query, search_list_size, query_bbox, scan.xs_snapshot);
}

#[pg_guard]
pub extern "C-unwind" fn amgettuple(
    scan: pg_sys::IndexScanDesc,
    _direction: pg_sys::ScanDirection::Type,
) -> bool {
    let scan: PgBox<pg_sys::IndexScanDescData> = unsafe { PgBox::from_pg(scan) };
    let state = unsafe { (scan.opaque as *mut TSVScanState).as_mut() }.expect("no scandesc state");

    let indexrel = unsafe { PgRelation::from_pg(scan.indexRelation) };
    let heaprel = unsafe { PgRelation::from_pg(scan.heapRelation) };

    let mut storage = unsafe { state.storage.as_mut() }.expect("no storage in state");
    match &mut storage {
        StorageState::SpatialScan(spatial) => {
            let next = spatial.next();
            get_tuple(state, next, scan)
        }
        StorageState::SbqSpeedup(quantizer, iter) => {
            let bq = SbqSpeedupStorage::load_for_search(
                &indexrel,
                &heaprel,
                quantizer,
                &state.meta_page,
            );
            let next = iter.next_with_resort(&scan, &indexrel, &bq);
            get_tuple(state, next, scan)
        }
        StorageState::Plain(iter) => {
            let storage = PlainStorage::load_for_search(&indexrel, &heaprel, &state.meta_page);
            let next = if state.meta_page.get_num_dimensions()
                == state.meta_page.get_num_dimensions_to_index()
            {
                /* no need to resort */
                iter.next(&storage)
            } else {
                iter.next_with_resort(&scan, &indexrel, &storage)
            };
            get_tuple(state, next, scan)
        }
    }
}

fn get_tuple(
    state: &mut TSVScanState,
    next: Option<(HeapPointer, IndexPointer)>,
    mut scan: PgBox<pg_sys::IndexScanDescData>,
) -> bool {
    scan.xs_recheckorderby = false;
    match next {
        Some((heap_pointer, index_pointer)) => {
            let tid_to_set = &mut scan.xs_heaptid;
            heap_pointer.to_item_pointer_data(tid_to_set);

            /*
             * An index scan must maintain a pin on the index page holding the
             * item last returned by amgettuple
             *
             * https://www.postgresql.org/docs/current/index-locking.html
             */
            let indexrel = unsafe { PgRelation::from_pg(scan.indexRelation) };
            state.last_buffer = Some(PinnedBufferShare::read(
                &indexrel,
                index_pointer.block_number,
            ));
            true
        }
        None => {
            state.last_buffer = None;
            false
        }
    }
}

#[pg_guard]
pub extern "C-unwind" fn amendscan(scan: pg_sys::IndexScanDesc) {
    let min_level = unsafe {
        let l = pg_sys::log_min_messages;
        let c = pg_sys::client_min_messages;
        std::cmp::min(l, c)
    };
    if min_level <= pg_sys::DEBUG1 as _ {
        let scan: PgBox<pg_sys::IndexScanDescData> = unsafe { PgBox::from_pg(scan) };
        let state =
            unsafe { (scan.opaque as *mut TSVScanState).as_mut() }.expect("no scandesc state");

        let mut storage = unsafe { state.storage.as_mut() }.expect("no storage in state");
        match &mut storage {
            StorageState::SpatialScan(spatial) => {
                debug1!(
                    "Spatial scan stats - returned {} of {} candidates",
                    spatial.position,
                    spatial.results.len()
                );
            }
            StorageState::SbqSpeedup(_bq, iter) => end_scan::<SbqSpeedupStorage>(iter),
            StorageState::Plain(iter) => end_scan::<PlainStorage>(iter),
        }
    }
}

fn end_scan<S: Storage>(
    iter: &mut TSVResponseIterator<S::QueryDistanceMeasure, S::LSNPrivateData>,
) {
    debug1!(
        "Query stats - reads_index={} reads_heap={} d_total={} d_quantized={} d_full={} next={} resort={} visits={} candidate={}",
        iter.lsr.stats.get_node_reads(),
        iter.lsr.stats.get_node_heap_reads(),
        iter.lsr.stats.get_total_distance_comparisons(),
        iter.lsr.stats.get_quantized_distance_comparisons(),
        iter.full_distance_comparisons,
        iter.next_calls,
        iter.next_calls_with_resort,
        iter.lsr.stats.get_visited_nodes(),
        iter.lsr.stats.get_candidate_nodes(),
    );

    debug_assert_eq!(iter.quantizer_stats.node_reads, 1);
    debug_assert_eq!(iter.quantizer_stats.node_writes, 0);
}
