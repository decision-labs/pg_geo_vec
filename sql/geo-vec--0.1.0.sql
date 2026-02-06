-- pg_geo_vec extension
-- Composite index for vector similarity + spatial filtering

-- Create the access method handler
CREATE OR REPLACE FUNCTION geo_vec_amhandler(internal)
RETURNS index_am_handler
PARALLEL SAFE
IMMUTABLE
STRICT
COST 0.0001
LANGUAGE c
AS '@MODULE_PATHNAME@', 'geo_vec_amhandler';

-- Create the access method if not exists
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

-- Access method comment
COMMENT ON ACCESS METHOD geo_vec IS 'pg_geo_vec: composite vector + spatial index';

-- Distance type function
CREATE OR REPLACE FUNCTION geo_vec_distance_type(internal)
RETURNS internal
PARALLEL SAFE
IMMUTABLE
STRICT
COST 0.0001
LANGUAGE c
AS '@MODULE_PATHNAME@', 'geo_vec_distance_type';

-- Operator classes for pg_geo_vec index
-- These will be created when the index is defined

-- Hybrid API installer. Creates PostGIS-dependent helper functions when available.
CREATE OR REPLACE FUNCTION geo_vec_install_hybrid_api()
RETURNS boolean
LANGUAGE plpgsql
AS $$
BEGIN
    IF to_regtype('geometry') IS NULL THEN
        RETURN FALSE;
    END IF;

    EXECUTE $fn$
        CREATE OR REPLACE FUNCTION geo_vec_hybrid_bbox_l2(
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
        CREATE OR REPLACE FUNCTION geo_vec_hybrid_dwithin_l2(
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
        CREATE OR REPLACE FUNCTION geo_vec_hybrid_bbox_cosine(
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
        CREATE OR REPLACE FUNCTION geo_vec_hybrid_dwithin_cosine(
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
        CREATE OR REPLACE FUNCTION geo_vec_hybrid_bbox_ip(
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
        CREATE OR REPLACE FUNCTION geo_vec_hybrid_dwithin_ip(
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

SELECT geo_vec_install_hybrid_api();
