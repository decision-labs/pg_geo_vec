-- Performance comparison: geo_vec vs HNSW+GiST vs DiskANN+GiST
-- Uses separate tables per approach — no index drop/rebuild needed
\set ON_ERROR_STOP on
\timing on

\echo '=== TABLE + INDEX SIZES ==='
SELECT tablename, indexname, pg_size_pretty(pg_relation_size(indexname::regclass)) AS size
FROM pg_indexes
WHERE tablename IN ('buildings', 'buildings_geovec', 'buildings_hnsw', 'buildings_diskann')
ORDER BY tablename, indexname;

-- ============================================================================
-- Ground truth (seq scan on base buildings table — no vector indexes)
-- ============================================================================
\echo ''
\echo '=== Computing ground truth (sequential scan) ==='
SET enable_indexscan = off;
SET enable_bitmapscan = off;
SET enable_seqscan = on;

CREATE TEMP TABLE query_vec AS SELECT embedding FROM buildings ORDER BY id LIMIT 1;

CREATE TEMP TABLE gt_wide AS
SELECT id, embedding <=> (SELECT embedding FROM query_vec) AS dist FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE gt_medium AS
SELECT id, embedding <=> (SELECT embedding FROM query_vec) AS dist FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5942, 47.6524, -117.5902, 47.6536, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE gt_narrow AS
SELECT id, embedding <=> (SELECT embedding FROM query_vec) AS dist FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5920, 47.6530, -117.5910, 47.6540, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE gt_vector AS
SELECT id, embedding <=> (SELECT embedding FROM query_vec) AS dist FROM buildings
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

-- ============================================================================
-- APPROACH 1: geo_vec (queries buildings_geovec — only has geo_vec index)
-- ============================================================================
\echo ''
\echo '========================================='
\echo '  APPROACH 1: geo_vec'
\echo '========================================='

SET enable_indexscan = on;
SET enable_bitmapscan = off;
SET enable_seqscan = off;
SET geo_vec.query_search_list_size = 200;

\echo '--- geo_vec: Wide bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM buildings_geovec
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE geovec_wide AS
SELECT id FROM buildings_geovec
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "geo_vec recall (wide)" FROM gt_wide g JOIN geovec_wide i ON g.id = i.id;

\echo ''
\echo '--- geo_vec: Medium bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM buildings_geovec
WHERE geom && ST_MakeEnvelope(-117.5942, 47.6524, -117.5902, 47.6536, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE geovec_medium AS
SELECT id FROM buildings_geovec
WHERE geom && ST_MakeEnvelope(-117.5942, 47.6524, -117.5902, 47.6536, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "geo_vec recall (medium)" FROM gt_medium g JOIN geovec_medium i ON g.id = i.id;

\echo ''
\echo '--- geo_vec: Narrow bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM buildings_geovec
WHERE geom && ST_MakeEnvelope(-117.5920, 47.6530, -117.5910, 47.6540, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE geovec_narrow AS
SELECT id FROM buildings_geovec
WHERE geom && ST_MakeEnvelope(-117.5920, 47.6530, -117.5910, 47.6540, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "geo_vec recall (narrow)" FROM gt_narrow g JOIN geovec_narrow i ON g.id = i.id;

\echo ''
\echo '--- geo_vec: Pure vector ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM buildings_geovec
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE geovec_vector AS
SELECT id FROM buildings_geovec
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "geo_vec recall (vector only)" FROM gt_vector g JOIN geovec_vector i ON g.id = i.id;

-- ============================================================================
-- APPROACH 2: HNSW + GiST (queries buildings_hnsw — only has HNSW + GiST)
-- ============================================================================
\echo ''
\echo '========================================='
\echo '  APPROACH 2: pgvector HNSW + PostGIS GiST'
\echo '========================================='

SET enable_indexscan = on;
SET enable_bitmapscan = on;
SET enable_seqscan = off;
SET hnsw.ef_search = 200;

\echo '--- HNSW+GiST: Wide bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM buildings_hnsw
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE hnsw_wide AS
SELECT id FROM buildings_hnsw
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "HNSW+GiST recall (wide)" FROM gt_wide g JOIN hnsw_wide i ON g.id = i.id;

\echo ''
\echo '--- HNSW+GiST: Medium bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM buildings_hnsw
WHERE geom && ST_MakeEnvelope(-117.5942, 47.6524, -117.5902, 47.6536, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE hnsw_medium AS
SELECT id FROM buildings_hnsw
WHERE geom && ST_MakeEnvelope(-117.5942, 47.6524, -117.5902, 47.6536, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "HNSW+GiST recall (medium)" FROM gt_medium g JOIN hnsw_medium i ON g.id = i.id;

\echo ''
\echo '--- HNSW+GiST: Narrow bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM buildings_hnsw
WHERE geom && ST_MakeEnvelope(-117.5920, 47.6530, -117.5910, 47.6540, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE hnsw_narrow AS
SELECT id FROM buildings_hnsw
WHERE geom && ST_MakeEnvelope(-117.5920, 47.6530, -117.5910, 47.6540, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "HNSW+GiST recall (narrow)" FROM gt_narrow g JOIN hnsw_narrow i ON g.id = i.id;

\echo ''
\echo '--- HNSW: Pure vector ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM buildings_hnsw
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE hnsw_vector AS
SELECT id FROM buildings_hnsw
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "HNSW recall (vector only)" FROM gt_vector g JOIN hnsw_vector i ON g.id = i.id;

-- ============================================================================
-- APPROACH 3: DiskANN + GiST (queries buildings_diskann)
-- ============================================================================
\echo ''
\echo '========================================='
\echo '  APPROACH 3: DiskANN + GiST'
\echo '========================================='

SET enable_indexscan = on;
SET enable_bitmapscan = on;
SET enable_seqscan = off;
SET diskann.query_search_list_size = 200;

\echo '--- DiskANN+GiST: Wide bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM buildings_diskann
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE da_wide AS
SELECT id FROM buildings_diskann
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "DiskANN+GiST recall (wide)" FROM gt_wide g JOIN da_wide i ON g.id = i.id;

\echo ''
\echo '--- DiskANN+GiST: Medium bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM buildings_diskann
WHERE geom && ST_MakeEnvelope(-117.5942, 47.6524, -117.5902, 47.6536, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE da_medium AS
SELECT id FROM buildings_diskann
WHERE geom && ST_MakeEnvelope(-117.5942, 47.6524, -117.5902, 47.6536, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "DiskANN+GiST recall (medium)" FROM gt_medium g JOIN da_medium i ON g.id = i.id;

\echo ''
\echo '--- DiskANN+GiST: Narrow bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM buildings_diskann
WHERE geom && ST_MakeEnvelope(-117.5920, 47.6530, -117.5910, 47.6540, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE da_narrow AS
SELECT id FROM buildings_diskann
WHERE geom && ST_MakeEnvelope(-117.5920, 47.6530, -117.5910, 47.6540, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "DiskANN+GiST recall (narrow)" FROM gt_narrow g JOIN da_narrow i ON g.id = i.id;

\echo ''
\echo '--- DiskANN: Pure vector ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM buildings_diskann
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE da_vector AS
SELECT id FROM buildings_diskann
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "DiskANN recall (vector only)" FROM gt_vector g JOIN da_vector i ON g.id = i.id;

-- ============================================================================
-- APPROACH 4: Sequential scan baseline (base buildings table)
-- ============================================================================
\echo ''
\echo '========================================='
\echo '  APPROACH 3: Sequential scan (baseline)'
\echo '========================================='

SET enable_indexscan = off;
SET enable_bitmapscan = off;
SET enable_seqscan = on;

\echo '--- SeqScan: Wide bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

\echo '--- SeqScan: Narrow bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5920, 47.6530, -117.5910, 47.6540, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

\echo '--- SeqScan: Pure vector ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM buildings
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

-- ============================================================================
-- Final summary
-- ============================================================================
\echo ''
\echo '========================================='
\echo '  RECALL SUMMARY (out of 20)'
\echo '========================================='
\echo 'geo_vec:'
SELECT 'wide' AS bbox, count(*) AS recall FROM gt_wide g JOIN geovec_wide i ON g.id = i.id
UNION ALL SELECT 'medium', count(*) FROM gt_medium g JOIN geovec_medium i ON g.id = i.id
UNION ALL SELECT 'narrow', count(*) FROM gt_narrow g JOIN geovec_narrow i ON g.id = i.id
UNION ALL SELECT 'vector_only', count(*) FROM gt_vector g JOIN geovec_vector i ON g.id = i.id;

\echo ''
\echo 'HNSW+GiST:'
SELECT 'wide' AS bbox, count(*) AS recall FROM gt_wide g JOIN hnsw_wide i ON g.id = i.id
UNION ALL SELECT 'medium', count(*) FROM gt_medium g JOIN hnsw_medium i ON g.id = i.id
UNION ALL SELECT 'narrow', count(*) FROM gt_narrow g JOIN hnsw_narrow i ON g.id = i.id
UNION ALL SELECT 'vector_only', count(*) FROM gt_vector g JOIN hnsw_vector i ON g.id = i.id;

\echo ''
\echo 'DiskANN+GiST:'
SELECT 'wide' AS bbox, count(*) AS recall FROM gt_wide g JOIN da_wide i ON g.id = i.id
UNION ALL SELECT 'medium', count(*) FROM gt_medium g JOIN da_medium i ON g.id = i.id
UNION ALL SELECT 'narrow', count(*) FROM gt_narrow g JOIN da_narrow i ON g.id = i.id
UNION ALL SELECT 'vector_only', count(*) FROM gt_vector g JOIN da_vector i ON g.id = i.id;
