-- Hybrid Spatial-Seeded Graph Search Tests
-- Tests three-way routing: brute-force (small bbox), hybrid (large bbox), pure graph (no bbox)
-- Run after both buildings and kigoto tables are populated and indexed.

\set ON_ERROR_STOP on
\timing on

-- ============================================================================
-- PART 1: Buildings (8,494 rows, 1024-dim) — small dataset, brute-force path
-- ============================================================================
\echo ''
\echo '========================================='
\echo '  PART 1: Buildings (small dataset)'
\echo '========================================='

SELECT count(*) AS total_buildings FROM buildings;

-- Verify the index exists
SELECT indexname FROM pg_indexes WHERE tablename = 'buildings' AND indexname = 'buildings_geo_vec_idx';

-- Set threshold high so all buildings bbox queries hit brute-force
SET geo_vec.spatial_brute_force_threshold = 50000;
SET geo_vec.query_search_list_size = 200;

-- Query vector
CREATE TEMP TABLE buildings_qvec AS SELECT embedding FROM buildings WHERE id = 1;

-- Ground truth (seq scan)
SET enable_indexscan = off;
SET enable_seqscan = on;

CREATE TEMP TABLE bld_gt_wide AS
SELECT id, embedding <=> (SELECT embedding FROM buildings_qvec) AS dist FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings_qvec) LIMIT 20;

CREATE TEMP TABLE bld_gt_narrow AS
SELECT id, embedding <=> (SELECT embedding FROM buildings_qvec) AS dist FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5920, 47.6530, -117.5910, 47.6540, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings_qvec) LIMIT 20;

CREATE TEMP TABLE bld_gt_vector AS
SELECT id, embedding <=> (SELECT embedding FROM buildings_qvec) AS dist FROM buildings
ORDER BY embedding <=> (SELECT embedding FROM buildings_qvec) LIMIT 20;

-- Index scan
SET enable_indexscan = on;
SET enable_seqscan = off;

\echo '--- Buildings: Wide bbox (brute-force path) ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings_qvec) LIMIT 20;

CREATE TEMP TABLE bld_idx_wide AS
SELECT id FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings_qvec) LIMIT 20;
SELECT count(*) AS "buildings recall (wide /20)" FROM bld_gt_wide g JOIN bld_idx_wide i ON g.id = i.id;

\echo '--- Buildings: Narrow bbox (brute-force path) ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5920, 47.6530, -117.5910, 47.6540, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings_qvec) LIMIT 20;

CREATE TEMP TABLE bld_idx_narrow AS
SELECT id FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5920, 47.6530, -117.5910, 47.6540, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings_qvec) LIMIT 20;
SELECT count(*) AS "buildings recall (narrow /20)" FROM bld_gt_narrow g JOIN bld_idx_narrow i ON g.id = i.id;

\echo '--- Buildings: Pure vector (graph path) ---'
CREATE TEMP TABLE bld_idx_vector AS
SELECT id FROM buildings
ORDER BY embedding <=> (SELECT embedding FROM buildings_qvec) LIMIT 20;
SELECT count(*) AS "buildings recall (vector /20)" FROM bld_gt_vector g JOIN bld_idx_vector i ON g.id = i.id;

-- Spatial precision check
SELECT count(*) AS "buildings outside_bbox_wide" FROM bld_idx_wide i JOIN buildings b ON b.id = i.id
WHERE NOT (b.geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326));

\echo ''
\echo 'Buildings: expect 20/20 recall (brute-force), 0 outside bbox'

-- ============================================================================
-- PART 2: Kigoto (318K rows, 384-dim) — large dataset, hybrid path
-- ============================================================================
\echo ''
\echo '========================================='
\echo '  PART 2: Kigoto (large dataset)'
\echo '========================================='

SELECT count(*) AS total_kigoto FROM kigoto;

-- Verify the index exists
SELECT indexname FROM pg_indexes WHERE tablename = 'kigoto' AND indexname = 'kigoto_geo_vec_idx';

-- Kigoto spatial extent: x=[32.8798, 32.8893], y=[-2.5073, -2.5007]
-- Bbox sizes designed to test all three routing paths:
--   tiny:   ~1K candidates  → brute-force (< 5000 threshold)
--   narrow: ~20K candidates → hybrid
--   medium: ~70K candidates → hybrid
--   wide:  ~180K candidates → hybrid
--   full:  ~318K candidates → hybrid (stress test, nearly all data)

\echo '=== Candidate counts ==='
SELECT 'tiny' AS bbox, count(*) AS candidates FROM kigoto
WHERE geom && ST_MakeEnvelope(32.8848, -2.5038, 32.8852, -2.5036, 4326)
UNION ALL SELECT 'narrow', count(*) FROM kigoto
WHERE geom && ST_MakeEnvelope(32.884, -2.5045, 32.8855, -2.5035, 4326)
UNION ALL SELECT 'medium', count(*) FROM kigoto
WHERE geom && ST_MakeEnvelope(32.883, -2.505, 32.886, -2.503, 4326)
UNION ALL SELECT 'wide', count(*) FROM kigoto
WHERE geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326)
UNION ALL SELECT 'full', count(*) FROM kigoto
WHERE geom && ST_MakeEnvelope(32.879, -2.508, 32.890, -2.500, 4326);

-- Query vector
CREATE TEMP TABLE kigoto_qvec AS SELECT embedding FROM kigoto WHERE id = 1000;

-- Ground truth (seq scan)
SET enable_indexscan = off;
SET enable_seqscan = on;

CREATE TEMP TABLE kg_gt_tiny AS
SELECT id, embedding <=> (SELECT embedding FROM kigoto_qvec) AS dist FROM kigoto
WHERE geom && ST_MakeEnvelope(32.8848, -2.5038, 32.8852, -2.5036, 4326)
ORDER BY embedding <=> (SELECT embedding FROM kigoto_qvec) LIMIT 20;

CREATE TEMP TABLE kg_gt_narrow AS
SELECT id, embedding <=> (SELECT embedding FROM kigoto_qvec) AS dist FROM kigoto
WHERE geom && ST_MakeEnvelope(32.884, -2.5045, 32.8855, -2.5035, 4326)
ORDER BY embedding <=> (SELECT embedding FROM kigoto_qvec) LIMIT 20;

CREATE TEMP TABLE kg_gt_medium AS
SELECT id, embedding <=> (SELECT embedding FROM kigoto_qvec) AS dist FROM kigoto
WHERE geom && ST_MakeEnvelope(32.883, -2.505, 32.886, -2.503, 4326)
ORDER BY embedding <=> (SELECT embedding FROM kigoto_qvec) LIMIT 20;

CREATE TEMP TABLE kg_gt_wide AS
SELECT id, embedding <=> (SELECT embedding FROM kigoto_qvec) AS dist FROM kigoto
WHERE geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326)
ORDER BY embedding <=> (SELECT embedding FROM kigoto_qvec) LIMIT 20;

CREATE TEMP TABLE kg_gt_full AS
SELECT id, embedding <=> (SELECT embedding FROM kigoto_qvec) AS dist FROM kigoto
WHERE geom && ST_MakeEnvelope(32.879, -2.508, 32.890, -2.500, 4326)
ORDER BY embedding <=> (SELECT embedding FROM kigoto_qvec) LIMIT 20;

CREATE TEMP TABLE kg_gt_vector AS
SELECT id, embedding <=> (SELECT embedding FROM kigoto_qvec) AS dist FROM kigoto
ORDER BY embedding <=> (SELECT embedding FROM kigoto_qvec) LIMIT 20;

-- ---- Three-way routing threshold = 5000 ----
SET geo_vec.spatial_brute_force_threshold = 5000;
SET geo_vec.spatial_seeds_per_cell = 2;
SET geo_vec.query_search_list_size = 200;

SET enable_indexscan = on;
SET enable_seqscan = off;

\echo ''
\echo '--- Kigoto: Tiny bbox (BRUTE-FORCE path, ~2K candidates) ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.8848, -2.5038, 32.8852, -2.5036, 4326)
ORDER BY embedding <=> (SELECT embedding FROM kigoto_qvec) LIMIT 20;

CREATE TEMP TABLE kg_idx_tiny AS
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.8848, -2.5038, 32.8852, -2.5036, 4326)
ORDER BY embedding <=> (SELECT embedding FROM kigoto_qvec) LIMIT 20;
SELECT count(*) AS "kigoto brute-force recall (tiny /20)" FROM kg_gt_tiny g JOIN kg_idx_tiny i ON g.id = i.id;

\echo ''
\echo '--- Kigoto: Narrow bbox (HYBRID path, ~20K candidates) ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.884, -2.5045, 32.8855, -2.5035, 4326)
ORDER BY embedding <=> (SELECT embedding FROM kigoto_qvec) LIMIT 20;

CREATE TEMP TABLE kg_idx_narrow AS
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.884, -2.5045, 32.8855, -2.5035, 4326)
ORDER BY embedding <=> (SELECT embedding FROM kigoto_qvec) LIMIT 20;
SELECT count(*) AS "kigoto hybrid recall (narrow /20)" FROM kg_gt_narrow g JOIN kg_idx_narrow i ON g.id = i.id;

\echo ''
\echo '--- Kigoto: Medium bbox (HYBRID path, ~70K candidates) ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.883, -2.505, 32.886, -2.503, 4326)
ORDER BY embedding <=> (SELECT embedding FROM kigoto_qvec) LIMIT 20;

CREATE TEMP TABLE kg_idx_medium AS
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.883, -2.505, 32.886, -2.503, 4326)
ORDER BY embedding <=> (SELECT embedding FROM kigoto_qvec) LIMIT 20;
SELECT count(*) AS "kigoto hybrid recall (medium /20)" FROM kg_gt_medium g JOIN kg_idx_medium i ON g.id = i.id;

\echo ''
\echo '--- Kigoto: Wide bbox (HYBRID path, ~180K candidates) ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326)
ORDER BY embedding <=> (SELECT embedding FROM kigoto_qvec) LIMIT 20;

CREATE TEMP TABLE kg_idx_wide AS
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326)
ORDER BY embedding <=> (SELECT embedding FROM kigoto_qvec) LIMIT 20;
SELECT count(*) AS "kigoto hybrid recall (wide /20)" FROM kg_gt_wide g JOIN kg_idx_wide i ON g.id = i.id;
SELECT count(*) AS "kigoto outside_bbox_wide" FROM kg_idx_wide i JOIN kigoto b ON b.id = i.id
WHERE NOT (b.geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326));

\echo ''
\echo '--- Kigoto: Full extent bbox (HYBRID path, ~318K candidates) ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.879, -2.508, 32.890, -2.500, 4326)
ORDER BY embedding <=> (SELECT embedding FROM kigoto_qvec) LIMIT 20;

CREATE TEMP TABLE kg_idx_full AS
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.879, -2.508, 32.890, -2.500, 4326)
ORDER BY embedding <=> (SELECT embedding FROM kigoto_qvec) LIMIT 20;
SELECT count(*) AS "kigoto hybrid recall (full /20)" FROM kg_gt_full g JOIN kg_idx_full i ON g.id = i.id;

\echo ''
\echo '--- Kigoto: Pure vector (graph path) ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto
ORDER BY embedding <=> (SELECT embedding FROM kigoto_qvec) LIMIT 20;

CREATE TEMP TABLE kg_idx_vector AS
SELECT id FROM kigoto
ORDER BY embedding <=> (SELECT embedding FROM kigoto_qvec) LIMIT 20;
SELECT count(*) AS "kigoto graph recall (vector /20)" FROM kg_gt_vector g JOIN kg_idx_vector i ON g.id = i.id;

-- ---- Comparison: force brute-force on wide bbox ----
\echo ''
\echo '--- Kigoto: Wide bbox forced BRUTE-FORCE (threshold=999999) ---'
SET geo_vec.spatial_brute_force_threshold = 999999;

EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326)
ORDER BY embedding <=> (SELECT embedding FROM kigoto_qvec) LIMIT 20;

CREATE TEMP TABLE kg_bf_wide AS
SELECT id FROM kigoto
WHERE geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326)
ORDER BY embedding <=> (SELECT embedding FROM kigoto_qvec) LIMIT 20;
SELECT count(*) AS "kigoto brute-force recall (wide /20)" FROM kg_gt_wide g JOIN kg_bf_wide i ON g.id = i.id;

-- ============================================================================
-- Summary
-- ============================================================================
\echo ''
\echo '========================================='
\echo '  RECALL SUMMARY (out of 20)'
\echo '========================================='

\echo 'Buildings (brute-force, all sizes):'
SELECT 'wide' AS bbox, count(*) AS recall FROM bld_gt_wide g JOIN bld_idx_wide i ON g.id = i.id
UNION ALL SELECT 'narrow', count(*) FROM bld_gt_narrow g JOIN bld_idx_narrow i ON g.id = i.id
UNION ALL SELECT 'vector', count(*) FROM bld_gt_vector g JOIN bld_idx_vector i ON g.id = i.id;

\echo ''
\echo 'Kigoto (three-way routing, threshold=5000):'
SELECT 'tiny (brute)' AS bbox, count(*) AS recall FROM kg_gt_tiny g JOIN kg_idx_tiny i ON g.id = i.id
UNION ALL SELECT 'narrow (hybrid)', count(*) FROM kg_gt_narrow g JOIN kg_idx_narrow i ON g.id = i.id
UNION ALL SELECT 'medium (hybrid)', count(*) FROM kg_gt_medium g JOIN kg_idx_medium i ON g.id = i.id
UNION ALL SELECT 'wide (hybrid)', count(*) FROM kg_gt_wide g JOIN kg_idx_wide i ON g.id = i.id
UNION ALL SELECT 'full (hybrid)', count(*) FROM kg_gt_full g JOIN kg_idx_full i ON g.id = i.id
UNION ALL SELECT 'vector (graph)', count(*) FROM kg_gt_vector g JOIN kg_idx_vector i ON g.id = i.id
UNION ALL SELECT 'wide (brute)', count(*) FROM kg_gt_wide g JOIN kg_bf_wide i ON g.id = i.id;
