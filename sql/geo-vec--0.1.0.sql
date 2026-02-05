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
