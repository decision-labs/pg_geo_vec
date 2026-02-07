-- Spatial Cell Index Integration Tests
-- Run after load_buildings.py populates the buildings table

\set ON_ERROR_STOP on
\timing on

-- ============================================================================
-- Setup: Create geo_vec index
-- ============================================================================

-- First verify data loaded correctly
SELECT count(*) AS total_buildings FROM buildings;
SELECT ST_XMin(e) AS xmin, ST_XMax(e) AS xmax, ST_YMin(e) AS ymin, ST_YMax(e) AS ymax
FROM (SELECT ST_Extent(geom) AS e FROM buildings) t;

-- Create the geo_vec index with geometry column
-- Using cosine distance, plain storage for simplicity
SET maintenance_work_mem = '512MB';
CREATE INDEX buildings_geo_vec_idx ON buildings
    USING geo_vec (embedding vector_cosine_ops, geom geometry_geo_vec_ops)
    WITH (storage_layout = plain);

-- Verify index was created
SELECT indexname, indexdef FROM pg_indexes WHERE tablename = 'buildings' AND indexname = 'buildings_geo_vec_idx';

-- ============================================================================
-- Test 1: Wide bbox (~20% of data)
-- ============================================================================
\echo '=== Test 1: Wide bbox (~20% of data) ==='

-- Pick a query vector (use first building's embedding)
-- Ground truth: sequential scan
SET enable_indexscan = off;
SET enable_seqscan = on;

CREATE TEMP TABLE gt_wide AS
SELECT id, embedding <=> (SELECT embedding FROM buildings LIMIT 1) AS dist
FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

SELECT count(*) AS "ground_truth_candidates_wide" FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326);

-- Index scan
SET enable_indexscan = on;
SET enable_seqscan = off;
SET geo_vec.query_search_list_size = 200;

CREATE TEMP TABLE idx_wide AS
SELECT id, embedding <=> (SELECT embedding FROM buildings LIMIT 1) AS dist
FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

-- Check EXPLAIN to verify geo_vec is used
EXPLAIN (COSTS OFF) SELECT id
FROM buildings
WHERE geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

-- Recall: how many of the top-20 ground truth are in the index result?
SELECT count(*) AS recall_wide_20
FROM gt_wide g
JOIN idx_wide i ON g.id = i.id;

-- All returned results should be within the bbox
SELECT count(*) AS results_outside_bbox_wide
FROM idx_wide i
JOIN buildings b ON b.id = i.id
WHERE NOT (b.geom && ST_MakeEnvelope(-117.598, 47.651, -117.586, 47.655, 4326));

-- ============================================================================
-- Test 2: Medium bbox (~10% of data)
-- ============================================================================
\echo '=== Test 2: Medium bbox (~10% of data) ==='

SET enable_indexscan = off;
SET enable_seqscan = on;

CREATE TEMP TABLE gt_medium AS
SELECT id, embedding <=> (SELECT embedding FROM buildings LIMIT 1) AS dist
FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5942, 47.6524, -117.5902, 47.6536, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

SELECT count(*) AS "ground_truth_candidates_medium" FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5942, 47.6524, -117.5902, 47.6536, 4326);

SET enable_indexscan = on;
SET enable_seqscan = off;

CREATE TEMP TABLE idx_medium AS
SELECT id, embedding <=> (SELECT embedding FROM buildings LIMIT 1) AS dist
FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5942, 47.6524, -117.5902, 47.6536, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

SELECT count(*) AS recall_medium_20
FROM gt_medium g
JOIN idx_medium i ON g.id = i.id;

SELECT count(*) AS results_outside_bbox_medium
FROM idx_medium i
JOIN buildings b ON b.id = i.id
WHERE NOT (b.geom && ST_MakeEnvelope(-117.5942, 47.6524, -117.5902, 47.6536, 4326));

-- ============================================================================
-- Test 3: Narrow bbox (~2% of data)
-- ============================================================================
\echo '=== Test 3: Narrow bbox (~2% of data) ==='

SET enable_indexscan = off;
SET enable_seqscan = on;

CREATE TEMP TABLE gt_narrow AS
SELECT id, embedding <=> (SELECT embedding FROM buildings LIMIT 1) AS dist
FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5920, 47.6530, -117.5910, 47.6540, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

SELECT count(*) AS "ground_truth_candidates_narrow" FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5920, 47.6530, -117.5910, 47.6540, 4326);

SET enable_indexscan = on;
SET enable_seqscan = off;

CREATE TEMP TABLE idx_narrow AS
SELECT id, embedding <=> (SELECT embedding FROM buildings LIMIT 1) AS dist
FROM buildings
WHERE geom && ST_MakeEnvelope(-117.5920, 47.6530, -117.5910, 47.6540, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

SELECT count(*) AS recall_narrow_20
FROM gt_narrow g
JOIN idx_narrow i ON g.id = i.id;

SELECT count(*) AS results_outside_bbox_narrow
FROM idx_narrow i
JOIN buildings b ON b.id = i.id
WHERE NOT (b.geom && ST_MakeEnvelope(-117.5920, 47.6530, -117.5910, 47.6540, 4326));

-- ============================================================================
-- Test 4: Empty bbox (outside extent) => should return 0 rows
-- ============================================================================
\echo '=== Test 4: Empty bbox (outside extent) ==='

SET enable_indexscan = on;
SET enable_seqscan = off;

SELECT count(*) AS "empty_bbox_result" FROM (
    SELECT id
    FROM buildings
    WHERE geom && ST_MakeEnvelope(-118.0, 48.0, -117.9, 48.1, 4326)
    ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
    LIMIT 20
) t;

-- ============================================================================
-- Test 5: Full extent bbox => should return top-20 from all buildings
-- ============================================================================
\echo '=== Test 5: Full extent bbox ==='

SET enable_indexscan = off;
SET enable_seqscan = on;

CREATE TEMP TABLE gt_full AS
SELECT id, embedding <=> (SELECT embedding FROM buildings LIMIT 1) AS dist
FROM buildings
WHERE geom && ST_MakeEnvelope(-117.61, 47.64, -117.58, 47.66, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

SET enable_indexscan = on;
SET enable_seqscan = off;

CREATE TEMP TABLE idx_full AS
SELECT id, embedding <=> (SELECT embedding FROM buildings LIMIT 1) AS dist
FROM buildings
WHERE geom && ST_MakeEnvelope(-117.61, 47.64, -117.58, 47.66, 4326)
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 20;

SELECT count(*) AS recall_full_20
FROM gt_full g
JOIN idx_full i ON g.id = i.id;

-- ============================================================================
-- Test 6: Pure vector query (no bbox) => should use graph search
-- ============================================================================
\echo '=== Test 6: Pure vector query (no bbox, uses graph search) ==='

SET enable_indexscan = on;
SET enable_seqscan = off;

EXPLAIN (COSTS OFF) SELECT id
FROM buildings
ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
LIMIT 10;

SELECT count(*) FROM (
    SELECT id
    FROM buildings
    ORDER BY embedding <=> (SELECT embedding FROM buildings LIMIT 1)
    LIMIT 10
) t;

-- ============================================================================
-- Summary
-- ============================================================================
\echo '=== SUMMARY ==='
\echo 'Recall should be 20/20 for all bbox tests (100%).'
\echo 'results_outside_bbox should be 0 for all tests.'
\echo 'empty_bbox_result should be 0.'
\echo 'Pure vector query should return 10 rows using Index Scan.'
