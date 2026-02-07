-- Label Filtering Integration Tests
-- Tests vector+label and vector+spatial+label index configurations
-- Run after buildings table is populated (load_buildings.py)
-- Requires: geo_vec extension loaded, buildings table with embedding + geom columns

\set ON_ERROR_STOP on
\timing on

-- ============================================================================
-- Setup: Add synthetic labels column
-- ============================================================================
\echo ''
\echo '========================================='
\echo '  Label Filtering Tests'
\echo '========================================='

SELECT count(*) AS total_buildings FROM buildings;

-- Add deterministic labels based on row ID
-- Each row gets 2 labels from a pool of 12 distinct values (0-4, 5-11)
ALTER TABLE buildings ADD COLUMN IF NOT EXISTS labels smallint[];
UPDATE buildings SET labels = ARRAY[
    (id % 5)::smallint,
    ((id * 3 + 1) % 7 + 5)::smallint
];

-- Verify label distribution
\echo '--- Label distribution ---'
SELECT 'rows with label 2' AS metric, count(*) AS cnt FROM buildings WHERE labels && ARRAY[2::smallint]
UNION ALL
SELECT 'rows with label 7', count(*) FROM buildings WHERE labels && ARRAY[7::smallint]
UNION ALL
SELECT 'total rows', count(*) FROM buildings;

-- ============================================================================
-- PART 1: Vector + Label (no geometry)
-- ============================================================================
\echo ''
\echo '========================================='
\echo '  PART 1: Vector + Label Index'
\echo '========================================='

-- Drop any existing geo_vec indexes on buildings to avoid conflicts
DROP INDEX IF EXISTS buildings_geo_vec_idx;
DROP INDEX IF EXISTS buildings_label_idx;
DROP INDEX IF EXISTS buildings_all_idx;

-- Create vector + label index (uses SBQ storage by default)
SET maintenance_work_mem = '512MB';
\echo '--- Creating vector + label index ---'
CREATE INDEX buildings_label_idx ON buildings
    USING geo_vec (embedding vector_cosine_ops, labels vector_smallint_label_ops);
ANALYZE buildings;

-- Verify index created
SELECT indexname, indexdef FROM pg_indexes
WHERE tablename = 'buildings' AND indexname = 'buildings_label_idx';

-- Query vector
CREATE TEMP TABLE lbl_qvec AS SELECT embedding FROM buildings WHERE id = 1;

-- Set search params
SET geo_vec.query_search_list_size = 200;
SET geo_vec.query_rescore = 100;

-- Ground truth: seq scan with label filter
SET enable_indexscan = off;
SET enable_seqscan = on;

CREATE TEMP TABLE lbl_gt AS
SELECT id FROM buildings
WHERE labels && ARRAY[2::smallint]
ORDER BY embedding <=> (SELECT embedding FROM lbl_qvec) LIMIT 20;

-- Index scan
SET enable_indexscan = on;
SET enable_seqscan = off;

\echo '--- Vector + Label: EXPLAIN ---'
EXPLAIN (COSTS OFF)
SELECT id FROM buildings
WHERE labels && ARRAY[2::smallint]
ORDER BY embedding <=> (SELECT embedding FROM lbl_qvec) LIMIT 20;

\echo '--- Vector + Label: query ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM buildings
WHERE labels && ARRAY[2::smallint]
ORDER BY embedding <=> (SELECT embedding FROM lbl_qvec) LIMIT 20;

CREATE TEMP TABLE lbl_idx AS
SELECT id FROM buildings
WHERE labels && ARRAY[2::smallint]
ORDER BY embedding <=> (SELECT embedding FROM lbl_qvec) LIMIT 20;

-- Recall
SELECT count(*) AS "label recall (/20)" FROM lbl_gt g JOIN lbl_idx i ON g.id = i.id;

-- Label precision: every returned row must have label 2
SELECT count(*) AS "label violations (should be 0)"
FROM lbl_idx i JOIN buildings b ON b.id = i.id
WHERE NOT (b.labels && ARRAY[2::smallint]);

-- ============================================================================
-- PART 2: Vector + Spatial + Label (all three columns)
-- ============================================================================
\echo ''
\echo '========================================='
\echo '  PART 2: Vector + Spatial + Label Index'
\echo '========================================='

-- Drop vector+label index, create the triple index
DROP INDEX buildings_label_idx;

\echo '--- Creating vector + spatial + label index ---'
CREATE INDEX buildings_all_idx ON buildings
    USING geo_vec (embedding vector_cosine_ops, geom geometry_geo_vec_ops, labels vector_smallint_label_ops);
ANALYZE buildings;

-- Verify index created
SELECT indexname, indexdef FROM pg_indexes
WHERE tablename = 'buildings' AND indexname = 'buildings_all_idx';

-- Ground truth: seq scan with bbox AND label filter
SET enable_indexscan = off;
SET enable_seqscan = on;

CREATE TEMP TABLE all_gt AS
SELECT id FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
  AND labels && ARRAY[2::smallint]
ORDER BY embedding <=> (SELECT embedding FROM lbl_qvec) LIMIT 20;

SELECT count(*) AS "ground truth candidates (bbox + label)" FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
  AND labels && ARRAY[2::smallint];

-- Index scan
SET enable_indexscan = on;
SET enable_seqscan = off;
SET geo_vec.spatial_brute_force_threshold = 50000;

\echo '--- Vector + Spatial + Label: EXPLAIN ---'
EXPLAIN (COSTS OFF)
SELECT id FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
  AND labels && ARRAY[2::smallint]
ORDER BY embedding <=> (SELECT embedding FROM lbl_qvec) LIMIT 20;

\echo '--- Vector + Spatial + Label: query ---'
EXPLAIN (ANALYZE, COSTS OFF, TIMING ON)
SELECT id FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
  AND labels && ARRAY[2::smallint]
ORDER BY embedding <=> (SELECT embedding FROM lbl_qvec) LIMIT 20;

CREATE TEMP TABLE all_idx AS
SELECT id FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
  AND labels && ARRAY[2::smallint]
ORDER BY embedding <=> (SELECT embedding FROM lbl_qvec) LIMIT 20;

-- Recall
SELECT count(*) AS "spatial+label recall (/20)" FROM all_gt g JOIN all_idx i ON g.id = i.id;

-- Label precision: every returned row must have label 2
SELECT count(*) AS "spatial+label label violations (should be 0)"
FROM all_idx i JOIN buildings b ON b.id = i.id
WHERE NOT (b.labels && ARRAY[2::smallint]);

-- Spatial precision: every returned row must be within bbox
SELECT count(*) AS "spatial+label bbox violations (should be 0)"
FROM all_idx i JOIN buildings b ON b.id = i.id
WHERE NOT (b.geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326));

-- ============================================================================
-- PART 3: Label-only query on triple index (no bbox)
-- ============================================================================
\echo ''
\echo '========================================='
\echo '  PART 3: Label-only query (no bbox)'
\echo '========================================='

SET enable_indexscan = off;
SET enable_seqscan = on;

CREATE TEMP TABLE lbl_only_gt AS
SELECT id FROM buildings
WHERE labels && ARRAY[2::smallint]
ORDER BY embedding <=> (SELECT embedding FROM lbl_qvec) LIMIT 20;

SET enable_indexscan = on;
SET enable_seqscan = off;

\echo '--- Label-only on triple index: query ---'
CREATE TEMP TABLE lbl_only_idx AS
SELECT id FROM buildings
WHERE labels && ARRAY[2::smallint]
ORDER BY embedding <=> (SELECT embedding FROM lbl_qvec) LIMIT 20;

SELECT count(*) AS "label-only recall on triple idx (/20)" FROM lbl_only_gt g JOIN lbl_only_idx i ON g.id = i.id;

SELECT count(*) AS "label-only violations (should be 0)"
FROM lbl_only_idx i JOIN buildings b ON b.id = i.id
WHERE NOT (b.labels && ARRAY[2::smallint]);

-- ============================================================================
-- Summary & Assertions
-- ============================================================================
\echo ''
\echo '========================================='
\echo '  RECALL SUMMARY'
\echo '========================================='

SELECT 'vector+label' AS test, count(*) AS recall FROM lbl_gt g JOIN lbl_idx i ON g.id = i.id
UNION ALL
SELECT 'vector+spatial+label', count(*) FROM all_gt g JOIN all_idx i ON g.id = i.id
UNION ALL
SELECT 'label-only (triple idx)', count(*) FROM lbl_only_gt g JOIN lbl_only_idx i ON g.id = i.id
ORDER BY test;

-- Assertions (fail CI if below thresholds)
DO $$
DECLARE
  label_recall int;
  all_recall int;
  label_only_recall int;
  label_violations int;
  all_label_violations int;
  all_bbox_violations int;
  label_only_violations int;
BEGIN
  -- Recall
  SELECT count(*) INTO label_recall FROM lbl_gt g JOIN lbl_idx i ON g.id = i.id;
  SELECT count(*) INTO all_recall FROM all_gt g JOIN all_idx i ON g.id = i.id;
  SELECT count(*) INTO label_only_recall FROM lbl_only_gt g JOIN lbl_only_idx i ON g.id = i.id;

  -- Label precision
  SELECT count(*) INTO label_violations
  FROM lbl_idx i JOIN buildings b ON b.id = i.id
  WHERE NOT (b.labels && ARRAY[2::smallint]);

  SELECT count(*) INTO all_label_violations
  FROM all_idx i JOIN buildings b ON b.id = i.id
  WHERE NOT (b.labels && ARRAY[2::smallint]);

  SELECT count(*) INTO all_bbox_violations
  FROM all_idx i JOIN buildings b ON b.id = i.id
  WHERE NOT (b.geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326));

  SELECT count(*) INTO label_only_violations
  FROM lbl_only_idx i JOIN buildings b ON b.id = i.id
  WHERE NOT (b.labels && ARRAY[2::smallint]);

  -- Recall assertions
  IF label_recall < 18 THEN
    RAISE EXCEPTION 'Vector+label recall too low: %/20', label_recall;
  END IF;
  IF all_recall < 16 THEN
    RAISE EXCEPTION 'Vector+spatial+label recall too low: %/20', all_recall;
  END IF;
  IF label_only_recall < 18 THEN
    RAISE EXCEPTION 'Label-only recall too low: %/20', label_only_recall;
  END IF;

  -- Precision assertions
  IF label_violations > 0 THEN
    RAISE EXCEPTION 'Vector+label: % results have wrong labels!', label_violations;
  END IF;
  IF all_label_violations > 0 THEN
    RAISE EXCEPTION 'Vector+spatial+label: % results have wrong labels!', all_label_violations;
  END IF;
  IF all_bbox_violations > 0 THEN
    RAISE EXCEPTION 'Vector+spatial+label: % results outside bbox!', all_bbox_violations;
  END IF;
  IF label_only_violations > 0 THEN
    RAISE EXCEPTION 'Label-only: % results have wrong labels!', label_only_violations;
  END IF;

  RAISE NOTICE 'PASS: vector+label=%/20 spatial+label=%/20 label-only=%/20 violations=0',
    label_recall, all_recall, label_only_recall;
END $$;

-- Cleanup: restore the original spatial-only index for other tests
DROP INDEX IF EXISTS buildings_all_idx;
CREATE INDEX buildings_geo_vec_idx ON buildings
    USING geo_vec (embedding vector_cosine_ops, geom geometry_geo_vec_ops);
