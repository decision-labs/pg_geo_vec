use crate::access_method::meta_page::MetaPage;
use pgrx::*;

/// cost estimate function loosely based on how ivfflat does things
#[pg_guard(immutable, parallel_safe)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C-unwind" fn amcostestimate(
    root: *mut pg_sys::PlannerInfo,
    path: *mut pg_sys::IndexPath,
    loop_count: f64,
    index_startup_cost: *mut pg_sys::Cost,
    index_total_cost: *mut pg_sys::Cost,
    index_selectivity: *mut pg_sys::Selectivity,
    index_correlation: *mut f64,
    index_pages: *mut f64,
) {
    if (*path).indexorderbys.is_null() {
        // Can't use index without order-bys
        *index_startup_cost = f64::MAX;
        *index_total_cost = f64::MAX;
        *index_selectivity = 0.;
        *index_correlation = 0.;
        *index_pages = 0.;
        #[cfg(any(feature = "pg18"))]
        {
            // Following in the footsteps of pgvector's PG18+ cost estimate change
            // https://github.com/pgvector/pgvector/commit/1291b12090bbb03bd92b92e42a1567ae5b1c96ad
            (*path).path.disabled_nodes = 2;
        }
        return;
    }
    let path_ref = path.as_ref().expect("path argument is NULL");

    let total_index_tuples = (*path_ref.indexinfo).tuples;
    let mut spatial_prune_ratio = 1.0_f64;

    let index_oid = (*path_ref.indexinfo).indexoid;
    let index_rel_ptr = pg_sys::RelationIdGetRelation(index_oid);
    if !index_rel_ptr.is_null() {
        let index_relation = PgRelation::from_pg(index_rel_ptr);
        let meta_page = MetaPage::fetch(&index_relation);
        if meta_page.has_spatial_partitioning() && !(*path).indexclauses.is_null() {
            let partitions = meta_page.get_num_partitions().max(1) as f64;
            // Heuristic: spatial clause + partitioning tends to probe a subset of partitions.
            // Keep a floor to avoid unrealistically tiny costs.
            spatial_prune_ratio = (1.0 / partitions).clamp(0.02, 1.0);
        }
    }

    let mut generic_costs = pg_sys::GenericCosts {
        numIndexTuples: (total_index_tuples / 100.) * spatial_prune_ratio, // TODO tune with empirical stats
        ..Default::default()
    };

    pg_sys::genericcostestimate(root, path, loop_count, &mut generic_costs);

    //TODO probably have to adjust costs more here

    *index_startup_cost = generic_costs.indexTotalCost * spatial_prune_ratio;
    *index_total_cost = generic_costs.indexTotalCost;
    *index_selectivity = (generic_costs.indexSelectivity * spatial_prune_ratio).clamp(0.0, 1.0);
    *index_correlation = generic_costs.indexCorrelation;
    *index_pages = generic_costs.numIndexPages;
    //pg_sys::cpu_index_tuple_cost;
}
