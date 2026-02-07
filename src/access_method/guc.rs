use pgrx::{pg_sys::AsPgCStr, *};

pub static TSV_QUERY_SEARCH_LIST_SIZE: GucSetting<i32> = GucSetting::<i32>::new(100);
pub static TSV_RESORT_SIZE: GucSetting<i32> = GucSetting::<i32>::new(50);
pub static TSV_PARALLEL_FLUSH_INTERVAL: GucSetting<f64> = GucSetting::<f64>::new(0.05);
pub static TSV_PARALLEL_INITIAL_START_NODES_COUNT: GucSetting<i32> = GucSetting::<i32>::new(1024);
pub static TSV_MIN_VECTORS_FOR_PARALLEL_BUILD: GucSetting<i32> = GucSetting::<i32>::new(65536);
pub static TSV_FORCE_PARALLEL_WORKERS: GucSetting<i32> = GucSetting::<i32>::new(-1);
pub static TSV_SPATIAL_BRUTE_FORCE_THRESHOLD: GucSetting<i32> = GucSetting::<i32>::new(5000);
pub static TSV_SPATIAL_SEEDS_PER_CELL: GucSetting<i32> = GucSetting::<i32>::new(2);

pub fn init() {
    GucRegistry::define_int_guc(
        unsafe { std::ffi::CStr::from_ptr("geo_vec.query_search_list_size".as_pg_cstr()) },
        unsafe {
            std::ffi::CStr::from_ptr("The size of the search list used in queries".as_pg_cstr())
        },
        unsafe {
            std::ffi::CStr::from_ptr(
                "Higher value increases recall at the cost of speed.".as_pg_cstr(),
            )
        },
        &TSV_QUERY_SEARCH_LIST_SIZE,
        1,
        10000,
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_int_guc(
        unsafe { std::ffi::CStr::from_ptr("geo_vec.query_rescore".as_pg_cstr()) },
        unsafe {
            std::ffi::CStr::from_ptr(
                "The number of elements rescored (0 to disable rescoring)".as_pg_cstr(),
            )
        },
        unsafe {
            std::ffi::CStr::from_ptr("Rescoring takes the query_rescore number of elements that have the smallest approximate distance, rescores them with the exact distance, returning the closest ones with the exact distance.".as_pg_cstr())
        },
        &TSV_RESORT_SIZE,
        0,
        1000,
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_float_guc(
        unsafe { std::ffi::CStr::from_ptr("geo_vec.parallel_flush_interval".as_pg_cstr()) },
        unsafe {
            std::ffi::CStr::from_ptr("The fraction of total vectors processed before flushing neighbor cache in parallel builds".as_pg_cstr())
        },
        unsafe {
            std::ffi::CStr::from_ptr("Controls how often the neighbor cache is flushed during parallel index builds as a fraction of total vectors (0.0-1.0).".as_pg_cstr())
        },
        &TSV_PARALLEL_FLUSH_INTERVAL,
        0.0,
        1.0,
        GucContext::Suset,
        GucFlags::default(),
    );

    GucRegistry::define_int_guc(
        unsafe {
            std::ffi::CStr::from_ptr("geo_vec.parallel_initial_start_nodes_count".as_pg_cstr())
        },
        unsafe {
            std::ffi::CStr::from_ptr(
                "The number of initial start nodes to process before starting parallel workers"
                    .as_pg_cstr(),
            )
        },
        unsafe {
            std::ffi::CStr::from_ptr("Determines how many nodes the initializing worker processes before other workers begin. Affects parallel build coordination.".as_pg_cstr())
        },
        &TSV_PARALLEL_INITIAL_START_NODES_COUNT,
        1,
        10000,
        GucContext::Suset,
        GucFlags::default(),
    );

    GucRegistry::define_int_guc(
        unsafe { std::ffi::CStr::from_ptr("geo_vec.min_vectors_for_parallel_build".as_pg_cstr()) },
        unsafe {
            std::ffi::CStr::from_ptr(
                "Minimum number of vectors required to enable parallel building".as_pg_cstr(),
            )
        },
        unsafe {
            std::ffi::CStr::from_ptr("If the table has fewer vectors than this threshold, parallel building will be disabled and serial building will be used instead.".as_pg_cstr())
        },
        &TSV_MIN_VECTORS_FOR_PARALLEL_BUILD,
        1,
        i32::MAX,
        GucContext::Suset,
        GucFlags::default(),
    );

    GucRegistry::define_int_guc(
        unsafe { std::ffi::CStr::from_ptr("geo_vec.force_parallel_workers".as_pg_cstr()) },
        unsafe {
            std::ffi::CStr::from_ptr(
                "Force a specific number of parallel workers for index builds".as_pg_cstr(),
            )
        },
        unsafe {
            std::ffi::CStr::from_ptr("When set to a positive value, this overrides PostgreSQL's automatic worker count determination. Set to -1 to use automatic determination (default).".as_pg_cstr())
        },
        &TSV_FORCE_PARALLEL_WORKERS,
        -1,
        1024,
        GucContext::Suset,
        GucFlags::default(),
    );

    GucRegistry::define_int_guc(
        unsafe {
            std::ffi::CStr::from_ptr("geo_vec.spatial_brute_force_threshold".as_pg_cstr())
        },
        unsafe {
            std::ffi::CStr::from_ptr(
                "Candidate count threshold for brute-force vs hybrid spatial search".as_pg_cstr(),
            )
        },
        unsafe {
            std::ffi::CStr::from_ptr("When a spatial+vector query has fewer candidates than this threshold, brute-force scan is used (100% recall). Above this threshold, spatial-seeded graph search is used (faster but approximate).".as_pg_cstr())
        },
        &TSV_SPATIAL_BRUTE_FORCE_THRESHOLD,
        0,
        i32::MAX,
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_int_guc(
        unsafe { std::ffi::CStr::from_ptr("geo_vec.spatial_seeds_per_cell".as_pg_cstr()) },
        unsafe {
            std::ffi::CStr::from_ptr(
                "Number of seed nodes sampled per grid cell for hybrid spatial search".as_pg_cstr(),
            )
        },
        unsafe {
            std::ffi::CStr::from_ptr("Controls how many evenly-spaced seed nodes are sampled from each overlapping grid cell to initialize the graph search in hybrid spatial mode.".as_pg_cstr())
        },
        &TSV_SPATIAL_SEEDS_PER_CELL,
        1,
        100,
        GucContext::Userset,
        GucFlags::default(),
    );
}
