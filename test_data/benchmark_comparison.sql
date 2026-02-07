-- Performance comparison: geo_vec spatial cell index vs pgvector HNSW + PostGIS GiST
-- Run after load_buildings.py populates the buildings table and geo_vec index exists

\set ON_ERROR_STOP on
\timing on

-- ============================================================================
-- Setup: Create pgvector + PostGIS indexes
-- ============================================================================

-- pgvector HNSW index
CREATE INDEX IF NOT EXISTS buildings_hnsw_idx ON buildings
    USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 100);

-- PostGIS GiST index
CREATE INDEX IF NOT EXISTS buildings_gist_idx ON buildings
    USING gist (geom);

ANALYZE buildings;

-- Show index sizes
SELECT indexname,
       pg_size_pretty(pg_relation_size(indexname::regclass)) AS size
FROM pg_indexes
WHERE tablename = 'buildings'
ORDER BY indexname;

-- ============================================================================
-- Ground truth via sequential scan
-- ============================================================================

-- Use first row's embedding as query vector
\echo ''
\echo '=== Computing ground truth (sequential scan) ==='

SET enable_indexscan = off;
SET enable_bitmapscan = off;
SET enable_seqscan = on;

-- Wide bbox ground truth
CREATE TEMP TABLE gt_wide AS
SELECT id, embedding <=> (SELECT embedding FROM buildings LIMIT 1) AS dist
FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

-- Medium bbox ground truth
CREATE TEMP TABLE gt_medium AS
SELECT id, embedding <=> (SELECT embedding FROM buildings LIMIT 1) AS dist
FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5942, 47.6524, -117.5902, 47.6536, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

-- Narrow bbox ground truth
CREATE TEMP TABLE gt_narrow AS
SELECT id, embedding <=> (SELECT embedding FROM buildings LIMIT 1) AS dist
FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5920, 47.6530, -117.5910, 47.6540, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

-- Pure vector ground truth (no bbox)
CREATE TEMP TABLE gt_vector AS
SELECT id, embedding <=> (SELECT embedding FROM buildings LIMIT 1) AS dist
FROM buildings
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

-- ============================================================================
-- Approach 1: geo_vec (spatial cell index)
-- ============================================================================
\echo ''
\echo '========================================='
\echo '  APPROACH 1: geo_vec (spatial cell index)'
\echo '========================================='

SET enable_indexscan = on;
SET enable_bitmapscan = off;
SET enable_seqscan = off;
SET geo_vec.query_search_list_size = 200;

-- Wide bbox
\echo ''
\echo '--- geo_vec: Wide bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id
FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

CREATE TEMP TABLE geovec_wide AS
SELECT id FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

SELECT count(*) AS "geo_vec recall (wide)" FROM gt_wide g JOIN geovec_wide i ON g.id = i.id;

-- Medium bbox
\echo ''
\echo '--- geo_vec: Medium bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id
FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5942, 47.6524, -117.5902, 47.6536, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

CREATE TEMP TABLE geovec_medium AS
SELECT id FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5942, 47.6524, -117.5902, 47.6536, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

SELECT count(*) AS "geo_vec recall (medium)" FROM gt_medium g JOIN geovec_medium i ON g.id = i.id;

-- Narrow bbox
\echo ''
\echo '--- geo_vec: Narrow bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id
FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5920, 47.6530, -117.5910, 47.6540, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

CREATE TEMP TABLE geovec_narrow AS
SELECT id FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5920, 47.6530, -117.5910, 47.6540, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

SELECT count(*) AS "geo_vec recall (narrow)" FROM gt_narrow g JOIN geovec_narrow i ON g.id = i.id;

-- Pure vector (no bbox)
\echo ''
\echo '--- geo_vec: Pure vector (no bbox) ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id
FROM buildings
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

CREATE TEMP TABLE geovec_vector AS
SELECT id FROM buildings
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

SELECT count(*) AS "geo_vec recall (vector only)" FROM gt_vector g JOIN geovec_vector i ON g.id = i.id;

-- ============================================================================
-- Approach 2: pgvector HNSW + PostGIS GiST (two separate indexes)
-- ============================================================================
\echo ''
\echo '========================================='
\echo '  APPROACH 2: pgvector HNSW + PostGIS GiST'
\echo '========================================='

-- Drop geo_vec index temporarily so planner picks HNSW
DROP INDEX buildings_geo_vec_idx;

SET enable_indexscan = on;
SET enable_bitmapscan = on;
SET enable_seqscan = off;
SET hnsw.ef_search = 200;

-- Wide bbox
\echo ''
\echo '--- HNSW+GiST: Wide bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id
FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

CREATE TEMP TABLE hnsw_wide AS
SELECT id FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

SELECT count(*) AS "HNSW+GiST recall (wide)" FROM gt_wide g JOIN hnsw_wide i ON g.id = i.id;

-- Medium bbox
\echo ''
\echo '--- HNSW+GiST: Medium bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id
FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5942, 47.6524, -117.5902, 47.6536, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

CREATE TEMP TABLE hnsw_medium AS
SELECT id FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5942, 47.6524, -117.5902, 47.6536, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

SELECT count(*) AS "HNSW+GiST recall (medium)" FROM gt_medium g JOIN hnsw_medium i ON g.id = i.id;

-- Narrow bbox
\echo ''
\echo '--- HNSW+GiST: Narrow bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id
FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5920, 47.6530, -117.5910, 47.6540, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

CREATE TEMP TABLE hnsw_narrow AS
SELECT id FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5920, 47.6530, -117.5910, 47.6540, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

SELECT count(*) AS "HNSW+GiST recall (narrow)" FROM gt_narrow g JOIN hnsw_narrow i ON g.id = i.id;

-- Pure vector (no bbox)
\echo ''
\echo '--- HNSW: Pure vector (no bbox) ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id
FROM buildings
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

CREATE TEMP TABLE hnsw_vector AS
SELECT id FROM buildings
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

SELECT count(*) AS "HNSW recall (vector only)" FROM gt_vector g JOIN hnsw_vector i ON g.id = i.id;

-- ============================================================================
-- Approach 3: Sequential scan (baseline)
-- ============================================================================
\echo ''
\echo '========================================='
\echo '  APPROACH 3: Sequential scan (baseline)'
\echo '========================================='

SET enable_indexscan = off;
SET enable_bitmapscan = off;
SET enable_seqscan = on;

\echo ''
\echo '--- SeqScan: Wide bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id
FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

\echo ''
\echo '--- SeqScan: Narrow bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id
FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5920, 47.6530, -117.5910, 47.6540, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

\echo ''
\echo '--- SeqScan: Pure vector (no bbox) ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id
FROM buildings
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

-- Recreate geo_vec index for future use
\echo ''
\echo '>>> Recreating geo_vec index...'
SET enable_indexscan = on;
SET enable_seqscan = on;
SET enable_bitmapscan = on;
SET maintenance_work_mem = '512MB';
CREATE INDEX buildings_geo_vec_idx ON buildings
    USING geo_vec (embedding vector_cosine_ops, geom geometry_geo_vec_ops)
    WITH (storage_layout = plain);

-- ============================================================================
-- Summary table
-- ============================================================================
\echo ''
\echo '========================================='
\echo '  INDEX SIZES'
\echo '========================================='

SELECT indexname,
       pg_size_pretty(pg_relation_size(indexname::regclass)) AS size
FROM pg_indexes
WHERE tablename = 'buildings'
ORDER BY indexname;

\echo ''
\echo '========================================='
\echo '  RECALL SUMMARY (out of 20)'
\echo '========================================='
\echo 'geo_vec results:'
SELECT 'wide' AS bbox,  count(*) AS recall FROM gt_wide  g JOIN geovec_wide  i ON g.id = i.id
UNION ALL
SELECT 'medium',        count(*)           FROM gt_medium g JOIN geovec_medium i ON g.id = i.id
UNION ALL
SELECT 'narrow',        count(*)           FROM gt_narrow g JOIN geovec_narrow i ON g.id = i.id
UNION ALL
SELECT 'vector_only',   count(*)           FROM gt_vector g JOIN geovec_vector i ON g.id = i.id;

\echo ''
\echo 'HNSW+GiST results:'
SELECT 'wide' AS bbox,  count(*) AS recall FROM gt_wide  g JOIN hnsw_wide  i ON g.id = i.id
UNION ALL
SELECT 'medium',        count(*)           FROM gt_medium g JOIN hnsw_medium i ON g.id = i.id
UNION ALL
SELECT 'narrow',        count(*)           FROM gt_narrow g JOIN hnsw_narrow i ON g.id = i.id
UNION ALL
SELECT 'vector_only',   count(*)           FROM gt_vector g JOIN hnsw_vector i ON g.id = i.id;
