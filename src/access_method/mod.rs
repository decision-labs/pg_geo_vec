use std::collections::HashSet;

use pgrx::*;
mod build;
mod cost_estimate;
mod debugging;
pub mod distance;
mod graph;
pub mod guc;
mod labels;
mod meta_page;
mod node;
pub mod options;
pub mod pg_vector;
pub mod plain;
mod sbq;
mod scan;
pub mod stats;
mod storage;
pub mod spatial_index;
mod spatial_maintain;
mod storage_common;
mod type_utils;
mod vacuum;

/// Access method support function numbers
pub const GEO_VEC_DISTANCE_TYPE_PROC: u16 = 1;

#[pg_extern(sql = "
    CREATE OR REPLACE FUNCTION geo_vec_amhandler(internal) RETURNS index_am_handler PARALLEL SAFE IMMUTABLE STRICT COST 0.0001 LANGUAGE c AS '@MODULE_PATHNAME@', '@FUNCTION_NAME@';

    DO $$
    DECLARE
        c int;
    BEGIN
        SELECT count(*)
        INTO c
        FROM pg_catalog.pg_am a
        WHERE a.amname = 'geo_vec';

        IF c = 0 THEN
            CREATE ACCESS METHOD geo_vec TYPE INDEX HANDLER geo_vec_amhandler;
        END IF;
    END;
    $$;
")]
fn amhandler(_fcinfo: pg_sys::FunctionCallInfo) -> PgBox<pg_sys::IndexAmRoutine> {
    let mut amroutine =
        unsafe { PgBox::<pg_sys::IndexAmRoutine>::alloc_node(pg_sys::NodeTag::T_IndexAmRoutine) };

    amroutine.amstrategies = 0;
    amroutine.amsupport = 1;

    amroutine.amcanorder = false;
    amroutine.amcanorderbyop = true;
    amroutine.amcanbackward = false; /* can change direction mid-scan */
    amroutine.amcanunique = false;
    amroutine.amcanmulticol = true;
    amroutine.amoptionalkey = true;
    amroutine.amsearcharray = false;
    amroutine.amsearchnulls = false;
    amroutine.amstorage = false;
    amroutine.amclusterable = false;
    amroutine.ampredlocks = false;
    amroutine.amcanparallel = false; //TODO
    amroutine.amcaninclude = false; //TODO
    amroutine.amoptsprocnum = 0;
    amroutine.amusemaintenanceworkmem = false; /* not used during VACUUM */
    //amroutine.amparallelvacuumoptions = pg_sys  VACUUM_OPTION_PARALLEL_BULKDEL; //TODO
    amroutine.amkeytype = pg_sys::InvalidOid;

    amroutine.amvalidate = Some(amvalidate);
    amroutine.ambuild = Some(build::ambuild);
    amroutine.ambuildempty = Some(build::ambuildempty);
    amroutine.aminsert = Some(build::aminsert);
    amroutine.ambulkdelete = Some(vacuum::ambulkdelete);
    amroutine.amvacuumcleanup = Some(vacuum::amvacuumcleanup);
    amroutine.amcostestimate = Some(cost_estimate::amcostestimate);
    amroutine.amoptions = Some(options::amoptions);
    amroutine.ambeginscan = Some(scan::ambeginscan);
    amroutine.amrescan = Some(scan::amrescan);
    amroutine.amgettuple = Some(scan::amgettuple);
    amroutine.amgetbitmap = None;
    amroutine.amendscan = Some(scan::amendscan);

    amroutine.ambuildphasename = Some(build::ambuildphasename);

    #[cfg(all(any(feature = "pg17", feature = "pg18"), feature = "build_parallel"))]
    {
        amroutine.amcanbuildparallel = true;
    }

    amroutine.into_pg_boxed()
}

// Background on system catalog state needed to understand the SQL for idempotent install/upgrade
// ----------------------------------------------------------------------------------------------
//
// When installing from scratch, we execute:
//
// CREATE OPERATOR CLASS vector_cosine_ops
// DEFAULT FOR TYPE vector USING geo_vec AS
//     OPERATOR 1 <=> (vector, vector) FOR ORDER BY float_ops,
//     FUNCTION 1 distance_type_cosine();
//
// This creates the following system catalog state:
//
// (1) A row in pg_opclass for vector_l2_ops and geo_vec:
//
//   oid  | opcmethod |      opcname      | opcnamespace | opcowner | opcfamily | opcintype | opcdefault | opckeytype
// -------+-----------+-------------------+--------------+----------+-----------+-----------+------------+------------
//  17722 |     17718 | vector_cosine_ops |         2200 |       10 |     17721 |     17389 | t          |          0
//
//     Note: opcmethod is the oid of the access method (geo_vec) already in pg_am.
//     Also: note that opcdefault is t, which means that this is the default operator class for the type.
//
// (2) A row in pg_amop for the <=> operator:
//   oid  | amopfamily | amoplefttype | amoprighttype | amopstrategy | amoppurpose | amopopr | amopmethod | amopsortfamily
// -------+------------+--------------+---------------+--------------+-------------+---------+------------+----------------
//  17723 |      17721 |        17389 |         17389 |            1 | o           |   17438 |      17718 |           1970
//
// (3) A row in pg_amproc for the distance_type_cosine function:
//
//   oid  | amprocfamily | amproclefttype | amprocrighttype | amprocnum |        amproc
// -------+--------------+----------------+-----------------+-----------+----------------------
//  17724 |        17721 |          17389 |           17389 |         1 | distance_type_cosine
//
// Version 0.4.0 contained the same SQL as above, but without the FUNCTION 1 part:
//
// CREATE OPERATOR CLASS vector_cosine_ops
// DEFAULT FOR TYPE vector USING geo_vec AS
//     OPERATOR 1 <=> (vector, vector) FOR ORDER BY float_ops;
//
// Thus, when upgrading from 0.4.0 to 0.5.0, we need to add the appropriate entry in `pg_amproc`.
//
// Similarly, here is the sample system catalog state created by:
//
// CREATE OPERATOR CLASS vector_l2_ops
// FOR TYPE vector USING geo_vec AS
//     OPERATOR 1 <-> (vector, vector) FOR ORDER BY float_ops,
//     FUNCTION 1 distance_type_l2();
//
// (1) A row in pg_opclass for vector_l2_ops and geo_vec:
//
//   oid  | opcmethod |    opcname    | opcnamespace | opcowner | opcfamily | opcintype | opcdefault | opckeytype
// -------+-----------+---------------+--------------+----------+-----------+-----------+------------+------------
//  17726 |     17718 | vector_l2_ops |         2200 |       10 |     17725 |     17389 | f          |          0
//
//     Note: opcmethod is the oid of the access method (geo_vec) already in pg_am.
//     Also: note that opcdefault is f, which means that this is not the default operator class for the type.
//
// (2) A row in pg_amop for the <-> operator:
//
//   oid  | amopfamily | amoplefttype | amoprighttype | amopstrategy | amoppurpose | amopopr | amopmethod | amopsortfamily
// -------+------------+--------------+---------------+--------------+-------------+---------+------------+----------------
//  17727 |      17725 |        17389 |         17389 |            1 | o           |   17436 |      17718 |           1970
//
// (3) A row in pg_amproc for the distance_type_l2 function:
//
//   oid  | amprocfamily | amproclefttype | amprocrighttype | amprocnum |      amproc
// -------+--------------+----------------+-----------------+-----------+------------------
//  17728 |        17725 |          17389 |           17389 |         1 | distance_type_l2
//
// However, the situation is easier for upgrade.  Version 0.4.0 did not contain support for the L2 distance, so we can
// just run the CREATE OPERATOR CLASS statement above to add the L2 distance support.

// This SQL is made idempotent so that we can use the same script for the installation and the upgrade.
extension_sql!(
    r#"
DO $$
DECLARE
  have_cos_ops int;
  have_l2_ops int;
  have_ip_ops int;
  have_label_ops int;
BEGIN
    -- Has cosine operator class been installed previously?
    SELECT count(*)
    INTO have_cos_ops
    FROM pg_catalog.pg_opclass c
    WHERE c.opcname = 'vector_cosine_ops'
    AND c.opcmethod = (SELECT oid FROM pg_catalog.pg_am am WHERE am.amname = 'geo_vec')
    AND c.opcnamespace = (SELECT oid FROM pg_catalog.pg_namespace where nspname='@extschema@');

    -- Has L2 operator class been installed previously?
    SELECT count(*)
    INTO have_l2_ops
    FROM pg_catalog.pg_opclass c
    WHERE c.opcname = 'vector_l2_ops'
    AND c.opcmethod = (SELECT oid FROM pg_catalog.pg_am am WHERE am.amname = 'geo_vec')
    AND c.opcnamespace = (SELECT oid FROM pg_catalog.pg_namespace where nspname='@extschema@');

    -- Has inner product operator class been installed previously?
    SELECT count(*)
    INTO have_ip_ops
    FROM pg_catalog.pg_opclass c
    WHERE c.opcname = 'vector_ip_ops'
    AND c.opcmethod = (SELECT oid FROM pg_catalog.pg_am am WHERE am.amname = 'geo_vec')
    AND c.opcnamespace = (SELECT oid FROM pg_catalog.pg_namespace where nspname='@extschema@');
    
    -- Has label-filtering support been installed previously?
    SELECT count(*)
    INTO have_label_ops
    FROM pg_catalog.pg_opclass c
    WHERE c.opcname = 'vector_smallint_label_ops'
    AND c.opcmethod = (SELECT oid FROM pg_catalog.pg_am am WHERE am.amname = 'geo_vec')
    AND c.opcnamespace = (SELECT oid FROM pg_catalog.pg_namespace where nspname='@extschema@');

    IF have_cos_ops = 0 THEN
        CREATE OPERATOR CLASS vector_cosine_ops DEFAULT
        FOR TYPE vector USING geo_vec AS
	        OPERATOR 1 <=> (vector, vector) FOR ORDER BY float_ops,
            FUNCTION 1 geo_vec_distance_type_cosine();
    ELSIF have_l2_ops = 0 THEN
        -- Upgrade from 0.4.0 to 0.5.0.  Update cosine opclass to include
        -- the geo_vec_distance_type_cosine function.
        INSERT INTO pg_amproc (oid, amprocfamily, amproclefttype, amprocrighttype, amprocnum, amproc)
        SELECT  (select (max(oid)::int + 1)::oid from pg_amproc), c.opcfamily, c.opcintype, c.opcintype, 1, '@extschema@.geo_vec_distance_type_l2'::regproc
        FROM pg_opclass c, pg_am a
        WHERE a.oid = c.opcmethod AND c.opcname = 'vector_cosine_ops' AND a.amname = 'geo_vec';
    END IF;

    IF have_l2_ops = 0 THEN
        CREATE OPERATOR CLASS vector_l2_ops
        FOR TYPE vector USING geo_vec AS
            OPERATOR 1 <-> (vector, vector) FOR ORDER BY float_ops,
            FUNCTION 1 geo_vec_distance_type_l2();
    END IF;

    IF have_ip_ops = 0 THEN
        CREATE OPERATOR CLASS vector_ip_ops
        FOR TYPE vector USING geo_vec AS
            OPERATOR 1 <#> (vector, vector) FOR ORDER BY float_ops,
            FUNCTION 1 geo_vec_distance_type_inner_product();
    END IF;
    
    -- First, check if the && operator exists for smallint[]
    IF NOT EXISTS (
        SELECT 1 FROM pg_operator 
        WHERE oprname = '&&' 
        AND oprleft = 'smallint[]'::regtype 
        AND oprright = 'smallint[]'::regtype
    ) THEN
        -- Create the && operator for smallint[]
        CREATE OPERATOR && (
            LEFTARG = smallint[],
            RIGHTARG = smallint[],
            PROCEDURE = geo_vec_smallint_array_overlap,
            COMMUTATOR = &&,
            RESTRICT = contsel,
            JOIN = contjoinsel
        );
        
        -- Register the operator with the system catalogs for proper selectivity estimation
        -- This is done by adding entries to pg_amop for the array_ops operator class
        EXECUTE format(
            'ALTER OPERATOR FAMILY array_ops USING btree ADD OPERATOR 3 && (smallint[], smallint[]) FOR SEARCH'
        );
    END IF;

    IF have_label_ops = 0 THEN
        CREATE OPERATOR CLASS vector_smallint_label_ops
        DEFAULT FOR TYPE smallint[] USING geo_vec AS
            OPERATOR 1 &&;
    END IF;

    -- geometry_geo_vec_ops: allows spatial filter on geometry column in geo_vec index
    IF to_regtype('geometry') IS NOT NULL THEN
        DECLARE have_geom_ops int;
        BEGIN
            SELECT count(*)
            INTO have_geom_ops
            FROM pg_catalog.pg_opclass c
            WHERE c.opcname = 'geometry_geo_vec_ops'
            AND c.opcmethod = (SELECT oid FROM pg_catalog.pg_am am WHERE am.amname = 'geo_vec')
            AND c.opcnamespace = (SELECT oid FROM pg_catalog.pg_namespace where nspname='@extschema@');

            IF have_geom_ops = 0 THEN
                CREATE OPERATOR CLASS geometry_geo_vec_ops
                DEFAULT FOR TYPE geometry USING geo_vec AS
                    OPERATOR 6 &&(geometry, geometry);
            END IF;
        END;
    END IF;
END;
$$;
"#,
    name = "geo_vec_ops_operator",
    requires = [
        amhandler,
        geo_vec_distance_type_cosine,
        geo_vec_distance_type_l2,
        geo_vec_distance_type_inner_product,
        geo_vec_smallint_array_overlap
    ]
);

// Hybrid API installer. This function is always creatable because geometry-specific signatures
// are emitted dynamically only when PostGIS is available.
extension_sql!(
    r#"
CREATE OR REPLACE FUNCTION @extschema@.geo_vec_install_hybrid_api()
RETURNS boolean
LANGUAGE plpgsql
AS $$
BEGIN
    IF to_regtype('geometry') IS NULL THEN
        RETURN FALSE;
    END IF;

    EXECUTE $fn$
        CREATE OR REPLACE FUNCTION @extschema@.geo_vec_hybrid_bbox_l2(
            p_table regclass,
            p_id_column name,
            p_geom_column name,
            p_embedding_column name,
            p_query_bbox geometry,
            p_query_embedding vector,
            p_k integer DEFAULT 20,
            p_candidate_limit integer DEFAULT 2000
        )
        RETURNS TABLE(row_id text, distance double precision)
        LANGUAGE plpgsql
        STABLE
        AS $inner$
        DECLARE
            query_sql text;
        BEGIN
            IF p_k <= 0 THEN
                RAISE EXCEPTION 'p_k must be > 0';
            END IF;
            IF p_candidate_limit <= 0 THEN
                RAISE EXCEPTION 'p_candidate_limit must be > 0';
            END IF;

            query_sql := format(
                'WITH spatial AS MATERIALIZED (
                    SELECT (%1$I)::text AS row_id, %2$I AS embedding
                    FROM %3$s
                    WHERE %4$I && $1
                    LIMIT $2
                )
                SELECT row_id, embedding <-> $3 AS distance
                FROM spatial
                ORDER BY embedding <-> $3
                LIMIT $4',
                p_id_column,
                p_embedding_column,
                p_table,
                p_geom_column
            );

            RETURN QUERY EXECUTE query_sql
                USING p_query_bbox, p_candidate_limit, p_query_embedding, p_k;
        END;
        $inner$;
    $fn$;

    EXECUTE $fn$
        CREATE OR REPLACE FUNCTION @extschema@.geo_vec_hybrid_dwithin_l2(
            p_table regclass,
            p_id_column name,
            p_geom_column name,
            p_embedding_column name,
            p_query_geom geometry,
            p_radius double precision,
            p_query_embedding vector,
            p_k integer DEFAULT 20,
            p_candidate_limit integer DEFAULT 2000
        )
        RETURNS TABLE(row_id text, distance double precision)
        LANGUAGE plpgsql
        STABLE
        AS $inner$
        DECLARE
            query_sql text;
        BEGIN
            IF p_k <= 0 THEN
                RAISE EXCEPTION 'p_k must be > 0';
            END IF;
            IF p_candidate_limit <= 0 THEN
                RAISE EXCEPTION 'p_candidate_limit must be > 0';
            END IF;
            IF p_radius < 0 THEN
                RAISE EXCEPTION 'p_radius must be >= 0';
            END IF;

            query_sql := format(
                'WITH spatial AS MATERIALIZED (
                    SELECT (%1$I)::text AS row_id, %2$I AS embedding
                    FROM %3$s
                    WHERE %4$I && ST_Expand($1, $2)
                      AND ST_DWithin(%4$I, $1, $2)
                    LIMIT $3
                )
                SELECT row_id, embedding <-> $4 AS distance
                FROM spatial
                ORDER BY embedding <-> $4
                LIMIT $5',
                p_id_column,
                p_embedding_column,
                p_table,
                p_geom_column
            );

            RETURN QUERY EXECUTE query_sql
                USING p_query_geom, p_radius, p_candidate_limit, p_query_embedding, p_k;
        END;
        $inner$;
    $fn$;

    EXECUTE $fn$
        CREATE OR REPLACE FUNCTION @extschema@.geo_vec_hybrid_bbox_cosine(
            p_table regclass,
            p_id_column name,
            p_geom_column name,
            p_embedding_column name,
            p_query_bbox geometry,
            p_query_embedding vector,
            p_k integer DEFAULT 20,
            p_candidate_limit integer DEFAULT 2000
        )
        RETURNS TABLE(row_id text, distance double precision)
        LANGUAGE plpgsql
        STABLE
        AS $inner$
        DECLARE
            query_sql text;
        BEGIN
            IF p_k <= 0 THEN
                RAISE EXCEPTION 'p_k must be > 0';
            END IF;
            IF p_candidate_limit <= 0 THEN
                RAISE EXCEPTION 'p_candidate_limit must be > 0';
            END IF;

            query_sql := format(
                'WITH spatial AS MATERIALIZED (
                    SELECT (%1$I)::text AS row_id, %2$I AS embedding
                    FROM %3$s
                    WHERE %4$I && $1
                    LIMIT $2
                )
                SELECT row_id, embedding <=> $3 AS distance
                FROM spatial
                ORDER BY embedding <=> $3
                LIMIT $4',
                p_id_column,
                p_embedding_column,
                p_table,
                p_geom_column
            );

            RETURN QUERY EXECUTE query_sql
                USING p_query_bbox, p_candidate_limit, p_query_embedding, p_k;
        END;
        $inner$;
    $fn$;

    EXECUTE $fn$
        CREATE OR REPLACE FUNCTION @extschema@.geo_vec_hybrid_dwithin_cosine(
            p_table regclass,
            p_id_column name,
            p_geom_column name,
            p_embedding_column name,
            p_query_geom geometry,
            p_radius double precision,
            p_query_embedding vector,
            p_k integer DEFAULT 20,
            p_candidate_limit integer DEFAULT 2000
        )
        RETURNS TABLE(row_id text, distance double precision)
        LANGUAGE plpgsql
        STABLE
        AS $inner$
        DECLARE
            query_sql text;
        BEGIN
            IF p_k <= 0 THEN
                RAISE EXCEPTION 'p_k must be > 0';
            END IF;
            IF p_candidate_limit <= 0 THEN
                RAISE EXCEPTION 'p_candidate_limit must be > 0';
            END IF;
            IF p_radius < 0 THEN
                RAISE EXCEPTION 'p_radius must be >= 0';
            END IF;

            query_sql := format(
                'WITH spatial AS MATERIALIZED (
                    SELECT (%1$I)::text AS row_id, %2$I AS embedding
                    FROM %3$s
                    WHERE %4$I && ST_Expand($1, $2)
                      AND ST_DWithin(%4$I, $1, $2)
                    LIMIT $3
                )
                SELECT row_id, embedding <=> $4 AS distance
                FROM spatial
                ORDER BY embedding <=> $4
                LIMIT $5',
                p_id_column,
                p_embedding_column,
                p_table,
                p_geom_column
            );

            RETURN QUERY EXECUTE query_sql
                USING p_query_geom, p_radius, p_candidate_limit, p_query_embedding, p_k;
        END;
        $inner$;
    $fn$;

    EXECUTE $fn$
        CREATE OR REPLACE FUNCTION @extschema@.geo_vec_hybrid_bbox_ip(
            p_table regclass,
            p_id_column name,
            p_geom_column name,
            p_embedding_column name,
            p_query_bbox geometry,
            p_query_embedding vector,
            p_k integer DEFAULT 20,
            p_candidate_limit integer DEFAULT 2000
        )
        RETURNS TABLE(row_id text, distance double precision)
        LANGUAGE plpgsql
        STABLE
        AS $inner$
        DECLARE
            query_sql text;
        BEGIN
            IF p_k <= 0 THEN
                RAISE EXCEPTION 'p_k must be > 0';
            END IF;
            IF p_candidate_limit <= 0 THEN
                RAISE EXCEPTION 'p_candidate_limit must be > 0';
            END IF;

            query_sql := format(
                'WITH spatial AS MATERIALIZED (
                    SELECT (%1$I)::text AS row_id, %2$I AS embedding
                    FROM %3$s
                    WHERE %4$I && $1
                    LIMIT $2
                )
                SELECT row_id, embedding <#> $3 AS distance
                FROM spatial
                ORDER BY embedding <#> $3
                LIMIT $4',
                p_id_column,
                p_embedding_column,
                p_table,
                p_geom_column
            );

            RETURN QUERY EXECUTE query_sql
                USING p_query_bbox, p_candidate_limit, p_query_embedding, p_k;
        END;
        $inner$;
    $fn$;

    EXECUTE $fn$
        CREATE OR REPLACE FUNCTION @extschema@.geo_vec_hybrid_dwithin_ip(
            p_table regclass,
            p_id_column name,
            p_geom_column name,
            p_embedding_column name,
            p_query_geom geometry,
            p_radius double precision,
            p_query_embedding vector,
            p_k integer DEFAULT 20,
            p_candidate_limit integer DEFAULT 2000
        )
        RETURNS TABLE(row_id text, distance double precision)
        LANGUAGE plpgsql
        STABLE
        AS $inner$
        DECLARE
            query_sql text;
        BEGIN
            IF p_k <= 0 THEN
                RAISE EXCEPTION 'p_k must be > 0';
            END IF;
            IF p_candidate_limit <= 0 THEN
                RAISE EXCEPTION 'p_candidate_limit must be > 0';
            END IF;
            IF p_radius < 0 THEN
                RAISE EXCEPTION 'p_radius must be >= 0';
            END IF;

            query_sql := format(
                'WITH spatial AS MATERIALIZED (
                    SELECT (%1$I)::text AS row_id, %2$I AS embedding
                    FROM %3$s
                    WHERE %4$I && ST_Expand($1, $2)
                      AND ST_DWithin(%4$I, $1, $2)
                    LIMIT $3
                )
                SELECT row_id, embedding <#> $4 AS distance
                FROM spatial
                ORDER BY embedding <#> $4
                LIMIT $5',
                p_id_column,
                p_embedding_column,
                p_table,
                p_geom_column
            );

            RETURN QUERY EXECUTE query_sql
                USING p_query_geom, p_radius, p_candidate_limit, p_query_embedding, p_k;
        END;
        $inner$;
    $fn$;

    RETURN TRUE;
END;
$$;

DO $$
BEGIN
    PERFORM @extschema@.geo_vec_install_hybrid_api();
END;
$$;
"#,
    name = "geo_vec_hybrid_api"
);

#[pg_guard]
pub extern "C-unwind" fn amvalidate(_opclassoid: pg_sys::Oid) -> bool {
    true
}

/// Implementation of the array overlap operator (&&) for smallint arrays
/// This function checks if two smallint arrays have at least one element in common
#[pg_extern(immutable, parallel_safe, create_or_replace)]
pub fn geo_vec_smallint_array_overlap(left: Array<i16>, right: Array<i16>) -> bool {
    // Early return for empty arrays
    if left.is_empty() || right.is_empty() {
        return false;
    }

    // If the arrays are small, use a simple quadratic search
    if left.len() <= 10 && right.len() <= 10 {
        for a in left.iter() {
            for b in right.iter() {
                if a.is_some() && b.is_some() && a.unwrap() == b.unwrap() {
                    return true;
                }
            }
        }
    } else {
        // Use a hash set to check if there are any elements in common
        let mut left_set = HashSet::new();
        for a in left.into_iter().flatten() {
            if !left_set.contains(&a) {
                left_set.insert(a);
            }
        }

        for b in right.into_iter().flatten() {
            if left_set.contains(&b) {
                return true;
            }
        }
    }

    false
}

#[cfg(any(test, feature = "pg_test"))]
#[pgrx::pg_schema]
mod tests {
    use super::*;

    #[pg_test]
    fn test_empty_overlap() -> spi::Result<()> {
        // Test overlap with arrays containing only NULL values
        let result = Spi::get_one::<bool>(
            "SELECT ARRAY[NULL, NULL, NULL]::smallint[] && ARRAY[NULL, NULL, NULL]::smallint[];",
        )?
        .expect("result was null");

        assert!(!result);
        Ok(())
    }

    #[pg_test]
    fn test_simple_overlap() -> spi::Result<()> {
        // Test simple overlap case with unsorted arrays
        let result = Spi::get_one::<bool>(
            "SELECT ARRAY[3, 1, 2]::smallint[] && ARRAY[6, 2, 4]::smallint[];",
        )?
        .expect("result was null");

        assert!(result);
        Ok(())
    }

    #[pg_test]
    fn test_no_overlap() -> spi::Result<()> {
        // Test no overlap with unsorted arrays
        let result = Spi::get_one::<bool>(
            "SELECT ARRAY[3, 1, 2]::smallint[] && ARRAY[8, 4, 6]::smallint[];",
        )?
        .expect("result was null");

        assert!(!result);
        Ok(())
    }

    #[pg_test]
    fn test_repeated_overlap() -> spi::Result<()> {
        // Test overlap with repeated elements and unsorted arrays
        let result = Spi::get_one::<bool>(
            "SELECT ARRAY[2, 1, 3, 2]::smallint[] && ARRAY[6, 4, 2]::smallint[];",
        )?
        .expect("result was null");

        assert!(result);
        Ok(())
    }

    #[pg_test]
    fn test_overlap_with_larger_arrays() -> spi::Result<()> {
        // Test overlap with larger unsorted arrays (triggering the hash set path)
        let result = Spi::get_one::<bool>(
            "SELECT ARRAY[19, 3, 2, 15, 7, 1, 14, 10, 11, 8, 13, 12, 9, 16, 17, 18, 2]::smallint[] && 
                    ARRAY[8, 6, 2, 4, 7]::smallint[];",
        )?
        .expect("result was null");

        assert!(result);
        Ok(())
    }

    #[pg_test]
    fn test_no_overlap_with_larger_arrays() -> spi::Result<()> {
        // Test no overlap with larger unsorted arrays (triggering the hash set path)
        let result = Spi::get_one::<bool>(
            "SELECT ARRAY[33, 2, 30, 5, 10, 20, 23, 24, 25, 26, 27, 28, 29, 1, 31, 32, 3]::smallint[] && 
                    ARRAY[9, 4, 8, 6]::smallint[];",
        )?
        .expect("result was null");

        assert!(!result);
        Ok(())
    }

    #[pg_test]
    fn test_hybrid_api_installer() -> spi::Result<()> {
        let installed = Spi::get_one::<bool>("SELECT geo_vec_install_hybrid_api();")?
            .expect("installer result was NULL");
        if !installed {
            return Ok(());
        }

        Spi::run("CREATE EXTENSION IF NOT EXISTS postgis;")?;
        Spi::run(
            "DROP TABLE IF EXISTS geo_vec_hybrid_api_test;
             CREATE TABLE geo_vec_hybrid_api_test (
                id bigint PRIMARY KEY,
                geom geometry(Point, 4326),
                embedding vector(2)
             );
             INSERT INTO geo_vec_hybrid_api_test VALUES
                (1, ST_SetSRID(ST_MakePoint(-122.401, 37.792), 4326), '[1.0,0.0]'),
                (2, ST_SetSRID(ST_MakePoint(-122.405, 37.790), 4326), '[0.9,0.1]'),
                (3, ST_SetSRID(ST_MakePoint(-122.500, 37.700), 4326), '[-1.0,0.0]');
             CREATE INDEX geo_vec_hybrid_api_test_geom_idx ON geo_vec_hybrid_api_test USING gist (geom);
             CREATE INDEX geo_vec_hybrid_api_test_vec_l2_idx
               ON geo_vec_hybrid_api_test USING geo_vec (embedding vector_l2_ops);
             CREATE INDEX geo_vec_hybrid_api_test_vec_cos_idx
               ON geo_vec_hybrid_api_test USING geo_vec (embedding vector_cosine_ops);
             CREATE INDEX geo_vec_hybrid_api_test_vec_ip_idx
               ON geo_vec_hybrid_api_test USING geo_vec (embedding vector_ip_ops);",
        )?;

        let top_l2_id = Spi::get_one::<String>(
            "SELECT row_id
             FROM geo_vec_hybrid_dwithin_l2(
                 'geo_vec_hybrid_api_test'::regclass,
                 'id',
                 'geom',
                 'embedding',
                 ST_SetSRID(ST_MakePoint(-122.401, 37.792), 4326),
                 0.02,
                 '[1.0,0.0]'::vector(2),
                 1,
                 100
             );",
        )?
        .expect("hybrid query returned no rows");

        let top_cos_id = Spi::get_one::<String>(
            "SELECT row_id
             FROM geo_vec_hybrid_dwithin_cosine(
                 'geo_vec_hybrid_api_test'::regclass,
                 'id',
                 'geom',
                 'embedding',
                 ST_SetSRID(ST_MakePoint(-122.401, 37.792), 4326),
                 0.02,
                 '[1.0,0.0]'::vector(2),
                 1,
                 100
             );",
        )?
        .expect("hybrid cosine query returned no rows");

        let top_ip_id = Spi::get_one::<String>(
            "SELECT row_id
             FROM geo_vec_hybrid_dwithin_ip(
                 'geo_vec_hybrid_api_test'::regclass,
                 'id',
                 'geom',
                 'embedding',
                 ST_SetSRID(ST_MakePoint(-122.401, 37.792), 4326),
                 0.02,
                 '[1.0,0.0]'::vector(2),
                 1,
                 100
             );",
        )?
        .expect("hybrid inner-product query returned no rows");

        assert_eq!(top_l2_id, "1");
        assert_eq!(top_cos_id, "1");
        assert_eq!(top_ip_id, "1");
        Spi::run("DROP TABLE geo_vec_hybrid_api_test;")?;
        Ok(())
    }
}
