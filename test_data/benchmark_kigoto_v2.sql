-- Performance comparison at scale: 318K rows, 384-dim
-- geo_vec vs pgvector HNSW + PostGIS GiST
-- Fixed: drop competing indexes during each approach's test

\set ON_ERROR_STOP on
\timing on

\echo '=== Creating additional indexes ==='
CREATE INDEX IF NOT EXISTS kigoto_hnsw_idx ON kigoto
    USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 100);
CREATE INDEX IF NOT EXISTS kigoto_gist_idx ON kigoto USING gist (geom);
ANALYZE kigoto;

\echo ''
\echo '=== INDEX SIZES ==='
SELECT indexname, pg_size_pretty(pg_relation_size(indexname::regclass)) AS size
FROM pg_indexes WHERE tablename = 'kigoto' ORDER BY indexname;

\echo ''
\echo '=== Candidate counts ==='
SELECT 'wide' AS bbox, count(*) AS candidates FROM kigoto
WHERE geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326)
UNION ALL SELECT 'medium', count(*) FROM kigoto
WHERE geom && ST_MakeEnvelope(32.883, -2.505, 32.886, -2.503, 4326)
UNION ALL SELECT 'narrow', count(*) FROM kigoto
WHERE geom && ST_MakeEnvelope(32.884, -2.5045, 32.8855, -2.5035, 4326);

-- ============================================================================
-- Ground truth (sequential scan)
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
-- APPROACH 1: geo_vec ONLY (drop HNSW + GiST so planner must use geo_vec)
-- ============================================================================
\echo ''
\echo '========================================='
\echo '  APPROACH 1: geo_vec ONLY'
\echo '========================================='

DROP INDEX IF EXISTS kigoto_hnsw_idx;
DROP INDEX IF EXISTS kigoto_gist_idx;

SET enable_indexscan = on;
SET enable_bitmapscan = off;
SET enable_seqscan = off;
SET geo_vec.query_search_list_size = 200;

\echo '--- geo_vec: Wide bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE gv_wide AS
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "geo_vec recall (wide)" FROM gt_wide g JOIN gv_wide i ON g.id = i.id;
SELECT count(*) AS "outside_bbox_wide" FROM gv_wide i JOIN kigoto b ON b.id = i.id
WHERE NOT (b.geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326));

\echo ''
\echo '--- geo_vec: Medium bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.883, -2.505, 32.886, -2.503, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE gv_medium AS
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.883, -2.505, 32.886, -2.503, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "geo_vec recall (medium)" FROM gt_medium g JOIN gv_medium i ON g.id = i.id;

\echo ''
\echo '--- geo_vec: Narrow bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.884, -2.5045, 32.8855, -2.5035, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE gv_narrow AS
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.884, -2.5045, 32.8855, -2.5035, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "geo_vec recall (narrow)" FROM gt_narrow g JOIN gv_narrow i ON g.id = i.id;

\echo ''
\echo '--- geo_vec: Pure vector ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE gv_vector AS
SELECT id FROM kigoto
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "geo_vec recall (vector)" FROM gt_vector g JOIN gv_vector i ON g.id = i.id;

-- ============================================================================
-- APPROACH 2: HNSW + GiST (drop geo_vec, recreate HNSW + GiST)
-- ============================================================================
\echo ''
\echo '========================================='
\echo '  APPROACH 2: HNSW + GiST'
\echo '========================================='

DROP INDEX IF EXISTS kigoto_geo_vec_idx;

CREATE INDEX kigoto_hnsw_idx ON kigoto USING hnsw (embedding vector_cosine_ops) WITH (m = 16, ef_construction = 100);
CREATE INDEX kigoto_gist_idx ON kigoto USING gist (geom);
ANALYZE kigoto;

SET enable_indexscan = on;
SET enable_bitmapscan = on;
SET enable_seqscan = off;
SET hnsw.ef_search = 200;

\echo '--- HNSW+GiST: Wide bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE hw_wide AS
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "HNSW+GiST recall (wide)" FROM gt_wide g JOIN hw_wide i ON g.id = i.id;

\echo ''
\echo '--- HNSW+GiST: Medium bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.883, -2.505, 32.886, -2.503, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE hw_medium AS
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.883, -2.505, 32.886, -2.503, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "HNSW+GiST recall (medium)" FROM gt_medium g JOIN hw_medium i ON g.id = i.id;

\echo ''
\echo '--- HNSW+GiST: Narrow bbox ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.884, -2.5045, 32.8855, -2.5035, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE hw_narrow AS
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.884, -2.5045, 32.8855, -2.5035, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "HNSW+GiST recall (narrow)" FROM gt_narrow g JOIN hw_narrow i ON g.id = i.id;

\echo ''
\echo '--- HNSW: Pure vector ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;

CREATE TEMP TABLE hw_vector AS
SELECT id FROM kigoto
ORDER BY embedding <=> (SELECT embedding FROM query_vec) LIMIT 20;
SELECT count(*) AS "HNSW recall (vector)" FROM gt_vector g JOIN hw_vector i ON g.id = i.id;

-- ============================================================================
-- Seq scan baseline
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
