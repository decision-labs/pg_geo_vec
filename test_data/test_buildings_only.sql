-- Buildings-only integration test for CI
-- Tests: build, brute-force spatial search, pure vector search, recall, spatial precision
\set ON_ERROR_STOP on
\timing on

SELECT count(*) AS total_buildings FROM buildings;

-- Verify the index exists and has spatial cell index
SELECT indexname FROM pg_indexes WHERE tablename = 'buildings' AND indexname = 'buildings_geo_vec_idx';

-- Set search params
SET geo_vec.spatial_brute_force_threshold = 50000;
SET geo_vec.query_search_list_size = 200;
SET geo_vec.query_rescore = 100;

-- Query vector
CREATE TEMP TABLE qvec AS SELECT embedding FROM buildings WHERE id = 1;

-- Ground truth (seq scan)
SET enable_indexscan = off;
SET enable_seqscan = on;

CREATE TEMP TABLE gt_wide AS
SELECT id FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> (SELECT embedding FROM qvec) LIMIT 20;

CREATE TEMP TABLE gt_narrow AS
SELECT id FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5920, 47.6530, -117.5910, 47.6540, 4326)
ORDER BY embedding <=> (SELECT embedding FROM qvec) LIMIT 20;

CREATE TEMP TABLE gt_vector AS
SELECT id FROM buildings
ORDER BY embedding <=> (SELECT embedding FROM qvec) LIMIT 20;

-- Switch to index scan
SET enable_indexscan = on;
SET enable_seqscan = off;

-- Wide bbox (brute-force path)
\echo '--- Wide bbox ---'
EXPLAIN (ANALYZE, COSTS OFF)
SELECT id FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> (SELECT embedding FROM qvec) LIMIT 20;

CREATE TEMP TABLE idx_wide AS
SELECT id FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> (SELECT embedding FROM qvec) LIMIT 20;

-- Narrow bbox (brute-force path)
\echo '--- Narrow bbox ---'
CREATE TEMP TABLE idx_narrow AS
SELECT id FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5920, 47.6530, -117.5910, 47.6540, 4326)
ORDER BY embedding <=> (SELECT embedding FROM qvec) LIMIT 20;

-- Pure vector (graph path)
\echo '--- Pure vector ---'
CREATE TEMP TABLE idx_vector AS
SELECT id FROM buildings
ORDER BY embedding <=> (SELECT embedding FROM qvec) LIMIT 20;

-- Recall
\echo '--- Recall ---'
SELECT 'wide' AS bbox, count(*) AS recall FROM gt_wide g JOIN idx_wide i ON g.id = i.id
UNION ALL
SELECT 'narrow', count(*) FROM gt_narrow g JOIN idx_narrow i ON g.id = i.id
UNION ALL
SELECT 'vector', count(*) FROM gt_vector g JOIN idx_vector i ON g.id = i.id
ORDER BY bbox;

-- Spatial precision: no results outside bbox
SELECT count(*) AS outside_bbox FROM idx_wide i JOIN buildings b ON b.id = i.id
WHERE NOT (b.geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326));

-- Recall assertions (fail CI if recall drops below threshold)
DO $$
DECLARE
  wide_recall int;
  vector_recall int;
  outside int;
BEGIN
  SELECT count(*) INTO wide_recall FROM gt_wide g JOIN idx_wide i ON g.id = i.id;
  SELECT count(*) INTO vector_recall FROM gt_vector g JOIN idx_vector i ON g.id = i.id;
  SELECT count(*) INTO outside FROM idx_wide i JOIN buildings b ON b.id = i.id
    WHERE NOT (b.geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326));

  IF wide_recall < 18 THEN
    RAISE EXCEPTION 'Wide bbox recall too low: %/20', wide_recall;
  END IF;
  IF vector_recall < 18 THEN
    RAISE EXCEPTION 'Vector recall too low: %/20', vector_recall;
  END IF;
  IF outside > 0 THEN
    RAISE EXCEPTION 'Found % results outside bbox!', outside;
  END IF;

  RAISE NOTICE 'PASS: wide=%/20 vector=%/20 outside_bbox=%', wide_recall, vector_recall, outside;
END $$;
