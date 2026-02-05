use std::collections::BinaryHeap;

use pgrx::{pg_sys::InvalidOffsetNumber, *};

use crate::{
    access_method::{
        graph::neighbor_store::GraphNeighborStore, labels::LabeledVector, meta_page::MetaPage,
        partition_metadata::GlobalBBox, sbq::storage::SbqSpeedupStorage,
    },
    partition::postgis::postgis_extract_bbox,
    util::{buffer::PinnedBufferShare, ports::pgstat_count_index_scan, HeapPointer, IndexPointer},
};

use super::{
    distance::DistanceFn,
    graph::{Graph, ListSearchResult},
    labels::LabelSetView,
    plain::{
        storage::{PlainStorage, PlainStorageLsnPrivateData},
        PlainDistanceMeasure,
    },
    sbq::{
        quantize::SbqQuantizer, storage::SbqSpeedupStorageLsnPrivateData, SbqMeans,
        SbqSearchDistanceMeasure,
    },
    stats::QuantizerStats,
    storage::{Storage, StorageType},
};

use crate::partition::BBox2D;

/* Be very careful not to transfer PgRelations in the state, as they can change between calls. That means we shouldn't be
using lifetimes here. Everything should be owned */
enum StorageState {
    SbqSpeedup(
        SbqQuantizer,
        TSVResponseIterator<SbqSearchDistanceMeasure, SbqSpeedupStorageLsnPrivateData>,
    ),
    Plain(TSVResponseIterator<PlainDistanceMeasure, PlainStorageLsnPrivateData>),
}

/* no lifetime usage here. */
struct TSVScanState {
    storage: *mut StorageState,
    distance_fn: Option<DistanceFn>,
    meta_page: MetaPage,
    last_buffer: Option<PinnedBufferShare>,
    /// Query bbox for spatial filtering (from PostGIS && operator)
    query_bbox: Option<BBox2D>,
    /// Global bbox from meta page
    global_bbox: GlobalBBox,
}

impl TSVScanState {
    fn new(meta_page: &MetaPage) -> Self {
        Self {
            storage: std::ptr::null_mut(),
            distance_fn: None,
            meta_page: meta_page.clone(),
            last_buffer: None,
            query_bbox: None,
            global_bbox: meta_page.get_global_bbox().clone(),
        }
    }

    fn initialize(
        &mut self,
        index: &PgRelation,
        heap: &PgRelation,
        query: LabeledVector,
        search_list_size: usize,
    ) {
        let meta_page = MetaPage::fetch(index);
        let storage = meta_page.get_storage_type();
        let distance = meta_page.get_distance_function();

        // Determine which partitions to search based on query bbox
        let partition_ids = self.get_partition_ids_for_query(&meta_page);

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
                    partition_ids.as_deref(),
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
                    partition_ids.as_deref(),
                );
                StorageState::SbqSpeedup(quantizer, it)
            }
        };

        self.storage = PgMemoryContexts::CurrentMemoryContext.leak_and_drop_on_delete(store_type);
        self.distance_fn = Some(distance);
    }

    /// Get partition IDs that overlap with the query bbox.
    /// Returns None if no spatial filter or no spatial partitioning.
    fn get_partition_ids_for_query(&self, meta_page: &MetaPage) -> Option<Vec<u32>> {
        // Check if we have a query bbox and spatial partitioning is enabled
        let query_bbox = self.query_bbox.as_ref()?;

        if !meta_page.has_spatial_partitioning() {
            return None;
        }

        // Get the grid partitioner
        let partitioner = meta_page.get_grid_partitioner()?;

        // Find overlapping partitions
        let partition_ids = partitioner.overlapping_cells(query_bbox);

        if partition_ids.is_empty() {
            return None;
        }

        pgrx::debug1!(
            "Query overlaps {} partitions: {:?}",
            partition_ids.len(),
            if partition_ids.len() <= 10 {
                format!("{:?}", partition_ids)
            } else {
                format!("{:?}...", &partition_ids[..10])
            }
        );

        Some(partition_ids)
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
        partition_ids: Option<&[u32]>,
    ) -> Self {
        let mut meta_page = MetaPage::fetch(index);
        let mut graph = Graph::new(GraphNeighborStore::Disk, &mut meta_page);

        let has_label_filter = query.labels().is_some_and(|labels| !labels.is_empty());

        // Use partition-aware search initialization when partition IDs are provided
        let lsr = graph.greedy_search_streaming_init_partitioned(
            query,
            search_list_size,
            partition_ids,
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

        /* Iterate until we find a non-deleted tuple */
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
                Some((heap_pointer, index_pointer)) => {
                    if heap_pointer.offset == InvalidOffsetNumber {
                        /* deleted tuple */
                        continue;
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

        /*error!(
            "Resort buffer size: {}, mean: {}, variance: {}, max_distance: {}: diff: {}",
            self.resort_buffer.len(),
            self.streaming_stats.mean(),
            self.streaming_stats.variance().sqrt(),
            self.streaming_stats.max_distance,
            self.streaming_stats.max_distance - self.resort_buffer.peek().unwrap().distance
        );*/

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
    assert!(nkeys == 0 || nkeys == 1, "Expected 0 or 1 keys");

    let mut scan: PgBox<pg_sys::IndexScanDescData> = unsafe { PgBox::from_pg(scan) };
    let indexrel = unsafe { PgRelation::from_pg(scan.indexRelation) };
    let heaprel = unsafe { PgRelation::from_pg(scan.heapRelation) };

    if nkeys > 0 {
        scan.xs_recheck = true;
    }

    let orderby_keys = unsafe {
        std::slice::from_raw_parts(orderbys as *const pg_sys::ScanKeyData, norderbys as _)
    };
    let keys =
        unsafe { std::slice::from_raw_parts(keys as *const pg_sys::ScanKeyData, nkeys as _) };

    let search_list_size = super::guc::TSV_QUERY_SEARCH_LIST_SIZE.get() as usize;

    let state = unsafe { (scan.opaque as *mut TSVScanState).as_mut() }.expect("no scandesc state");

    // Extract query bbox from spatial filter key if present
    if nkeys > 0 && !keys[0].sk_argument.is_null() {
        // The key contains a geometry from the spatial filter (e.g., && operator)
        let geom_datum = keys[0].sk_argument;
        // SAFETY: postgis_extract_bbox handles NULL geoms and validates the geometry type
        if let Some(bbox) = unsafe { postgis_extract_bbox(geom_datum) } {
            state.query_bbox = Some(bbox);
            pgrx::debug1!(
                "Spatial filter bbox: ({}, {}) - ({}, {})",
                bbox.xmin, bbox.ymin, bbox.xmax, bbox.ymax
            );
        }
    }

    let query = unsafe { LabeledVector::from_scan_key_data(keys, orderby_keys, &state.meta_page) };

    state.initialize(&indexrel, &heaprel, query, search_list_size);
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
