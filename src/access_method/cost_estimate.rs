//! Index cost estimation for the query planner (`amcostestimate`).
//!
//! Goal: give Postgres enough cardinality/cost signal to prefer `geo_vec` when a
//! spatial+vector query is selective, and not over-prefer it when the bbox covers
//! most of the table (see issue #4 / Kigoto wide≈57% case).
//!
//! Runtime already knows CSR candidate counts at scan start; this module approximates
//! that for planning via `clauselist_selectivity` on index quals plus a path model
//! aligned with scan routing (brute vs hybrid vs pure graph).

use pgrx::*;

use super::guc::{TSV_QUERY_SEARCH_LIST_SIZE, TSV_SPATIAL_BRUTE_FORCE_THRESHOLD};

/// Pure estimate of how many index entries a scan is expected to examine.
///
/// - `n`: indexed tuples
/// - `spatial_sel`: estimated fraction matching spatial (and other) index quals; use
///   `1.0` when there is no indexqual (pure vector ORDER BY)
/// - `limit_k`: expected LIMIT / top-K (from planner when known)
/// - `search_list`: `geo_vec.query_search_list_size`
/// - `brute_threshold`: `geo_vec.spatial_brute_force_threshold`
#[derive(Debug, Clone, Copy)]
pub struct ScanWorkEstimate {
    pub num_index_tuples: f64,
    pub estimated_candidates: f64,
    /// Which scan routing branch the estimate mirrors (used by unit tests / debugging).
    #[cfg_attr(not(test), allow(dead_code))]
    pub path: ScanPathKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanPathKind {
    /// No spatial indexqual — DiskANN-style graph probe.
    Graph,
    /// Spatial candidates ≤ brute threshold — CSR brute-force + exact distances.
    BruteForce,
    /// Spatial candidates above threshold — spatial-seeded hybrid graph.
    Hybrid,
}

pub fn estimate_scan_work(
    n: f64,
    spatial_sel: f64,
    limit_k: f64,
    search_list: f64,
    brute_threshold: f64,
    has_spatial_quals: bool,
) -> ScanWorkEstimate {
    let n = n.max(1.0);
    let search_list = search_list.clamp(1.0, 10_000.0);
    let limit_k = if limit_k.is_finite() && limit_k > 0.0 {
        limit_k.clamp(1.0, n)
    } else {
        search_list.min(n)
    };
    let spatial_sel = spatial_sel.clamp(0.0, 1.0);

    if !has_spatial_quals {
        // Pure vector: graph probe scales with search_list (and a small neighbor fanout),
        // not with n/100. Slight growth with log(n) mirrors HNSW-style estimators.
        let graph_work = (search_list * 4.0 + search_list * (n.ln().max(1.0) / 5.0)).min(n);
        return ScanWorkEstimate {
            num_index_tuples: graph_work.max(limit_k),
            estimated_candidates: n,
            path: ScanPathKind::Graph,
        };
    }

    let cand = (spatial_sel * n).max(1.0).min(n);
    if cand <= brute_threshold.max(0.0) {
        // Brute-force path examines ~all in-bbox candidates.
        ScanWorkEstimate {
            num_index_tuples: cand,
            estimated_candidates: cand,
            path: ScanPathKind::BruteForce,
        }
    } else {
        // Hybrid: seed sampling + graph probe. Cheaper than scanning all candidates,
        // but more work than pure graph as the bbox grows.
        let seed_work = (cand.sqrt() * 2.0).min(cand);
        let graph_work = search_list * 4.0;
        let hybrid = (seed_work + graph_work).min(cand).max(limit_k);
        ScanWorkEstimate {
            num_index_tuples: hybrid,
            estimated_candidates: cand,
            path: ScanPathKind::Hybrid,
        }
    }
}

/// Fraction of heap rows expected to be returned from the index scan.
/// For ANN `ORDER BY … LIMIT k`, that is ~k/n (optionally capped by spatial cand).
pub fn estimate_return_selectivity(
    n: f64,
    limit_k: f64,
    estimated_candidates: f64,
    has_spatial_quals: bool,
) -> f64 {
    let n = n.max(1.0);
    let k = if limit_k.is_finite() && limit_k > 0.0 {
        limit_k.clamp(1.0, n)
    } else {
        1.0
    };
    let returned = if has_spatial_quals {
        k.min(estimated_candidates)
    } else {
        k
    };
    (returned / n).clamp(f64::MIN_POSITIVE, 1.0)
}

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
        // Can't use index without distance ORDER BY.
        *index_startup_cost = f64::MAX;
        *index_total_cost = f64::MAX;
        *index_selectivity = 0.;
        *index_correlation = 0.;
        *index_pages = 0.;
        #[cfg(any(feature = "pg18"))]
        {
            // Following pgvector's PG18+ cost estimate change:
            // https://github.com/pgvector/pgvector/commit/1291b12090bbb03bd92b92e42a1567ae5b1c96ad
            (*path).path.disabled_nodes = 2;
        }
        return;
    }

    let path_ref = path.as_ref().expect("path argument is NULL");
    let root_ref = root.as_ref().expect("root argument is NULL");

    let total_index_tuples = (*path_ref.indexinfo).tuples.max(1.0);
    let has_spatial_quals = list_length(path_ref.indexclauses) > 0;

    let var_relid = if !path_ref.path.parent.is_null() {
        (*path_ref.path.parent).relid as std::ffi::c_int
    } else {
        0
    };

    let spatial_sel = if has_spatial_quals {
        estimate_indexqual_selectivity(root, path_ref.indexclauses, var_relid)
    } else {
        1.0
    };

    let limit_k = {
        let lt = root_ref.limit_tuples;
        if lt.is_finite() && lt > 0.0 {
            lt
        } else {
            // LIMIT not yet known at this path stage — fall back to search_list.
            TSV_QUERY_SEARCH_LIST_SIZE.get() as f64
        }
    };

    let search_list = TSV_QUERY_SEARCH_LIST_SIZE.get() as f64;
    let brute_threshold = TSV_SPATIAL_BRUTE_FORCE_THRESHOLD.get() as f64;

    let work = estimate_scan_work(
        total_index_tuples,
        spatial_sel,
        limit_k,
        search_list,
        brute_threshold,
        has_spatial_quals,
    );

    let mut generic_costs = pg_sys::GenericCosts {
        numIndexTuples: work.num_index_tuples,
        ..Default::default()
    };

    pg_sys::genericcostestimate(root, path, loop_count, &mut generic_costs);

    // ANN ordered scans return ≈ LIMIT k rows (capped by spatial candidates), not the
    // full spatial qual selectivity. Prefer that for path row estimates.
    let ann_sel = estimate_return_selectivity(
        total_index_tuples,
        limit_k,
        work.estimated_candidates,
        has_spatial_quals,
    );

    // Hybrid/brute startup: most work happens before the first tuple on ANN paths.
    // Scale startup toward total when we expect to examine many candidates.
    let startup_ratio = (work.num_index_tuples / total_index_tuples)
        .clamp(0.05, 1.0)
        .sqrt();
    let startup = generic_costs.indexTotalCost * startup_ratio;

    *index_startup_cost = startup;
    *index_total_cost = generic_costs.indexTotalCost;
    *index_selectivity = ann_sel;
    *index_correlation = generic_costs.indexCorrelation;
    *index_pages = generic_costs.numIndexPages;
}

/// Selectivity of `indexclauses` via `clauselist_selectivity` on their RestrictInfos.
unsafe fn estimate_indexqual_selectivity(
    root: *mut pg_sys::PlannerInfo,
    indexclauses: *mut pg_sys::List,
    var_relid: std::ffi::c_int,
) -> f64 {
    if list_length(indexclauses) == 0 {
        return 1.0;
    }

    let mut rinfos: *mut pg_sys::List = std::ptr::null_mut();
    for ic_ptr in list_ptr_values(indexclauses) {
        let ic = ic_ptr as *mut pg_sys::IndexClause;
        if ic.is_null() {
            continue;
        }
        let rinfo = (*ic).rinfo as *mut std::ffi::c_void;
        if !rinfo.is_null() {
            rinfos = pg_sys::lappend(rinfos, rinfo);
        }
    }

    if list_length(rinfos) == 0 {
        return 1.0;
    }

    let sel = pg_sys::clauselist_selectivity(
        root,
        rinfos,
        var_relid,
        pg_sys::JoinType::JOIN_INNER,
        std::ptr::null_mut(),
    );

    // RestrictInfo pointers are owned by the planner; only free the List spine.
    pg_sys::list_free(rinfos);

    if sel.is_finite() && sel > 0.0 {
        sel.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

unsafe fn list_length(list: *mut pg_sys::List) -> i32 {
    if list.is_null() {
        0
    } else {
        (*list).length
    }
}

unsafe fn list_ptr_values(list: *mut pg_sys::List) -> Vec<*mut std::ffi::c_void> {
    if list.is_null() || (*list).length <= 0 || (*list).elements.is_null() {
        return Vec::new();
    }
    let len = (*list).length as usize;
    let cells = std::slice::from_raw_parts((*list).elements, len);
    cells.iter().map(|c| c.ptr_value).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_estimate_graph_independent_of_n_scale() {
        let small = estimate_scan_work(10_000.0, 1.0, 20.0, 100.0, 5000.0, false);
        let large = estimate_scan_work(300_000.0, 1.0, 20.0, 100.0, 5000.0, false);
        assert_eq!(small.path, ScanPathKind::Graph);
        assert_eq!(large.path, ScanPathKind::Graph);
        // Must not use the old n/100 heuristic (would be 3000 on large).
        assert!(large.num_index_tuples < 3_000.0);
        assert!(large.num_index_tuples > 100.0);
    }

    #[test]
    fn work_estimate_selective_bbox_uses_brute() {
        // ~6% of 318K ≈ 19K still above default threshold 5K → hybrid
        let narrowish = estimate_scan_work(318_375.0, 0.06, 20.0, 100.0, 5000.0, true);
        assert_eq!(narrowish.path, ScanPathKind::Hybrid);

        // Truly selective: 2K candidates → brute
        let tiny = estimate_scan_work(318_375.0, 2000.0 / 318_375.0, 20.0, 100.0, 5000.0, true);
        assert_eq!(tiny.path, ScanPathKind::BruteForce);
        assert!((tiny.num_index_tuples - 2000.0).abs() < 1.0);
    }

    #[test]
    fn work_estimate_wide_bbox_examines_more_than_graph() {
        let graph = estimate_scan_work(318_375.0, 1.0, 20.0, 100.0, 5000.0, false);
        let wide = estimate_scan_work(318_375.0, 0.57, 20.0, 100.0, 5000.0, true);
        assert_eq!(wide.path, ScanPathKind::Hybrid);
        // Wide spatial should look more expensive to examine than pure graph.
        assert!(wide.num_index_tuples > graph.num_index_tuples);
    }

    #[test]
    fn return_selectivity_tracks_limit() {
        let sel = estimate_return_selectivity(100_000.0, 20.0, 100_000.0, false);
        assert!((sel - 20.0 / 100_000.0).abs() < 1e-12);
    }
}
