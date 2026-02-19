-- Performance comparison: 318K rows, 384-dim
-- geo_vec vs HNSW+GiST vs DiskANN+GiST
-- Uses separate tables per approach — no index drop/rebuild needed
\set ON_ERROR_STOP on
\timing on

\echo '=== TABLE + INDEX SIZES ==='
SELECT tablename, indexname, pg_size_pretty(pg_relation_size(indexname::regclass)) AS size
FROM pg_indexes
WHERE tablename IN ('kigoto', 'kigoto_geovec', 'kigoto_hnsw', 'kigoto_diskann')
ORDER BY tablename, indexname;

\echo ''
\echo '=== Candidate counts ==='
SELECT 'wide' AS bbox, count(*) AS candidates FROM kigoto
WHERE geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326)
UNION ALL SELECT 'medium', count(*) FROM kigoto
WHERE geom && ST_MakeEnvelope(32.883, -2.505, 32.886, -2.503, 4326)
UNION ALL SELECT 'narrow', count(*) FROM kigoto
WHERE geom && ST_MakeEnvelope(32.884, -2.5045, 32.8855, -2.5035, 4326);

-- ============================================================================
-- Ground truth (seq scan on base kigoto table — no vector indexes)
-- ============================================================================
\echo ''
\echo '=== Ground truth (seq scan) ==='
SET enable_indexscan = off;
SET enable_bitmapscan = off;
SET enable_seqscan = on;

CREATE TEMP TABLE query_vec AS SELECT embedding FROM kigoto WHERE id = 1000;

CREATE TEMP TABLE gt_wide AS
SELECT id, embedding <=> (SELECT embedding FROM query_vec) AS dist FROM kigoto
WHERE geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE gt_medium AS
SELECT id, embedding <=> (SELECT embedding FROM query_vec) AS dist FROM kigoto
WHERE geom && ST_MakeEnvelope(32.883, -2.505, 32.886, -2.503, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE gt_narrow AS
SELECT id, embedding <=> (SELECT embedding FROM query_vec) AS dist FROM kigoto
WHERE geom && ST_MakeEnvelope(32.884, -2.5045, 32.8855, -2.5035, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE gt_vector AS
SELECT id, embedding <=> (SELECT embedding FROM query_vec) AS dist FROM kigoto
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

-- ============================================================================
-- APPROACH 1: geo_vec (queries kigoto_geovec — only has geo_vec index)
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
SELECT id FROM kigoto_geovec
WHERE geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE gv_wide AS
SELECT id FROM kigoto_geovec
WHERE geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "geo_vec recall (wide)" FROM gt_wide g JOIN gv_wide i ON g.id = i.id;
SELECT count(*) AS "outside_bbox_wide" FROM gv_wide i JOIN kigoto_geovec b ON b.id = i.id
WHERE NOT (b.geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326));

\echo ''
\echo '--- geo_vec: Medium bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto_geovec
WHERE geom && ST_MakeEnvelope(32.883, -2.505, 32.886, -2.503, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE gv_medium AS
SELECT id FROM kigoto_geovec
WHERE geom && ST_MakeEnvelope(32.883, -2.505, 32.886, -2.503, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "geo_vec recall (medium)" FROM gt_medium g JOIN gv_medium i ON g.id = i.id;

\echo ''
\echo '--- geo_vec: Narrow bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto_geovec
WHERE geom && ST_MakeEnvelope(32.884, -2.5045, 32.8855, -2.5035, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE gv_narrow AS
SELECT id FROM kigoto_geovec
WHERE geom && ST_MakeEnvelope(32.884, -2.5045, 32.8855, -2.5035, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "geo_vec recall (narrow)" FROM gt_narrow g JOIN gv_narrow i ON g.id = i.id;

\echo ''
\echo '--- geo_vec: Pure vector ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto_geovec
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE gv_vector AS
SELECT id FROM kigoto_geovec
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "geo_vec recall (vector)" FROM gt_vector g JOIN gv_vector i ON g.id = i.id;

-- ============================================================================
-- APPROACH 2: HNSW + GiST (queries kigoto_hnsw — only has HNSW + GiST)
-- ============================================================================
\echo ''
\echo '========================================='
\echo '  APPROACH 2: HNSW + GiST'
\echo '========================================='

SET enable_indexscan = on;
SET enable_bitmapscan = on;
SET enable_seqscan = off;
SET hnsw.ef_search = 200;

\echo '--- HNSW+GiST: Wide bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto_hnsw
WHERE geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE hw_wide AS
SELECT id FROM kigoto_hnsw
WHERE geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "HNSW+GiST recall (wide)" FROM gt_wide g JOIN hw_wide i ON g.id = i.id;

\echo ''
\echo '--- HNSW+GiST: Medium bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto_hnsw
WHERE geom && ST_MakeEnvelope(32.883, -2.505, 32.886, -2.503, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE hw_medium AS
SELECT id FROM kigoto_hnsw
WHERE geom && ST_MakeEnvelope(32.883, -2.505, 32.886, -2.503, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "HNSW+GiST recall (medium)" FROM gt_medium g JOIN hw_medium i ON g.id = i.id;

\echo ''
\echo '--- HNSW+GiST: Narrow bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto_hnsw
WHERE geom && ST_MakeEnvelope(32.884, -2.5045, 32.8855, -2.5035, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE hw_narrow AS
SELECT id FROM kigoto_hnsw
WHERE geom && ST_MakeEnvelope(32.884, -2.5045, 32.8855, -2.5035, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "HNSW+GiST recall (narrow)" FROM gt_narrow g JOIN hw_narrow i ON g.id = i.id;

\echo ''
\echo '--- HNSW: Pure vector ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto_hnsw
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE hw_vector AS
SELECT id FROM kigoto_hnsw
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "HNSW recall (vector)" FROM gt_vector g JOIN hw_vector i ON g.id = i.id;

-- ============================================================================
-- APPROACH 3: DiskANN + GiST (queries kigoto_diskann)
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
SELECT id FROM kigoto_diskann
WHERE geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE da_wide AS
SELECT id FROM kigoto_diskann
WHERE geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "DiskANN+GiST recall (wide)" FROM gt_wide g JOIN da_wide i ON g.id = i.id;

\echo ''
\echo '--- DiskANN+GiST: Medium bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto_diskann
WHERE geom && ST_MakeEnvelope(32.883, -2.505, 32.886, -2.503, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE da_medium AS
SELECT id FROM kigoto_diskann
WHERE geom && ST_MakeEnvelope(32.883, -2.505, 32.886, -2.503, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "DiskANN+GiST recall (medium)" FROM gt_medium g JOIN da_medium i ON g.id = i.id;

\echo ''
\echo '--- DiskANN+GiST: Narrow bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto_diskann
WHERE geom && ST_MakeEnvelope(32.884, -2.5045, 32.8855, -2.5035, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE da_narrow AS
SELECT id FROM kigoto_diskann
WHERE geom && ST_MakeEnvelope(32.884, -2.5045, 32.8855, -2.5035, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "DiskANN+GiST recall (narrow)" FROM gt_narrow g JOIN da_narrow i ON g.id = i.id;

\echo ''
\echo '--- DiskANN: Pure vector ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto_diskann
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE da_vector AS
SELECT id FROM kigoto_diskann
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "DiskANN recall (vector)" FROM gt_vector g JOIN da_vector i ON g.id = i.id;

-- ============================================================================
-- APPROACH 4: Sequential scan baseline (base kigoto table)
-- ============================================================================
\echo ''
\echo '========================================='
\echo '  APPROACH 3: Seq scan baseline'
\echo '========================================='
SET enable_indexscan = off;
SET enable_bitmapscan = off;
SET enable_seqscan = on;

\echo '--- SeqScan: Wide ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

\echo '--- SeqScan: Narrow ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.884, -2.5045, 32.8855, -2.5035, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

\echo '--- SeqScan: Pure vector ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

-- ============================================================================
-- Final summary
-- ============================================================================
\echo ''
\echo '========================================='
\echo '  RECALL SUMMARY (out of 20)'
\echo '========================================='
\echo 'geo_vec:'
SELECT 'wide' AS bbox, count(*) AS recall FROM gt_wide g JOIN gv_wide i ON g.id = i.id
UNION ALL SELECT 'medium', count(*) FROM gt_medium g JOIN gv_medium i ON g.id = i.id
UNION ALL SELECT 'narrow', count(*) FROM gt_narrow g JOIN gv_narrow i ON g.id = i.id
UNION ALL SELECT 'vector_only', count(*) FROM gt_vector g JOIN gv_vector i ON g.id = i.id;

\echo ''
\echo 'HNSW+GiST:'
SELECT 'wide' AS bbox, count(*) AS recall FROM gt_wide g JOIN hw_wide i ON g.id = i.id
UNION ALL SELECT 'medium', count(*) FROM gt_medium g JOIN hw_medium i ON g.id = i.id
UNION ALL SELECT 'narrow', count(*) FROM gt_narrow g JOIN hw_narrow i ON g.id = i.id
UNION ALL SELECT 'vector_only', count(*) FROM gt_vector g JOIN hw_vector i ON g.id = i.id;

\echo ''
\echo 'DiskANN+GiST:'
SELECT 'wide' AS bbox, count(*) AS recall FROM gt_wide g JOIN da_wide i ON g.id = i.id
UNION ALL SELECT 'medium', count(*) FROM gt_medium g JOIN da_medium i ON g.id = i.id
UNION ALL SELECT 'narrow', count(*) FROM gt_narrow g JOIN da_narrow i ON g.id = i.id
UNION ALL SELECT 'vector_only', count(*) FROM gt_vector g JOIN da_vector i ON g.id = i.id;
