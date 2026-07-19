-- Planner index-choice smoke for issue #4 / amcostestimate.
-- Puts geo_vec (rich) + HNSW + GiST on ONE table and reports which path
-- EXPLAIN picks for tiny / narrow / medium / wide bboxes.
--
-- Expected signal after spatial+LIMIT-aware costs (not a hard guarantee):
--   selective bboxes → more likely Index Scan using …geo_vec…
--   wide / weak filter → may stay on HNSW+GiST (or bitmap)

\set ON_ERROR_STOP on
\timing off

\echo '========================================='
\echo '  Planner choice: geo_vec vs HNSW+GiST'
\echo '========================================='

-- One shared table with competing indexes (created once; cheap to re-run ANALYZE).
SELECT CASE
    WHEN to_regclass('public.kigoto_planner') IS NOT NULL THEN 'true'
    ELSE 'false'
END AS planner_table_exists \gset

\if :planner_table_exists
\echo '>>> Reusing kigoto_planner'
\else
\echo '>>> Creating kigoto_planner with competing indexes (one-time)...'
CREATE TABLE kigoto_planner AS TABLE kigoto;
ALTER TABLE kigoto_planner ADD PRIMARY KEY (id);

SET maintenance_work_mem = '1GB';

CREATE INDEX kigoto_planner_geovec_idx ON kigoto_planner
    USING geo_vec (embedding vector_cosine_ops, geom geometry_geo_vec_ops)
    WITH (storage_layout = plain);

CREATE INDEX kigoto_planner_hnsw_idx ON kigoto_planner
    USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 100);

CREATE INDEX kigoto_planner_gist_idx ON kigoto_planner
    USING gist (geom);
\endif

ANALYZE kigoto_planner;

\echo ''
\echo '=== Indexes on kigoto_planner ==='
SELECT indexname,
       pg_size_pretty(pg_relation_size(indexname::regclass)) AS size
FROM pg_indexes
WHERE tablename = 'kigoto_planner'
ORDER BY indexname;

CREATE TEMP TABLE query_vec AS
SELECT embedding FROM kigoto_planner WHERE id = 1000;

-- Candidate counts (same envelopes as hybrid / kigoto benches)
\echo ''
\echo '=== Candidate counts ==='
SELECT 'tiny' AS bbox, count(*) AS candidates,
       round(100.0 * count(*) / (SELECT count(*) FROM kigoto_planner), 2) AS pct
FROM kigoto_planner
WHERE geom && ST_MakeEnvelope(32.8848, -2.5038, 32.8852, -2.5036, 4326)
UNION ALL SELECT 'narrow', count(*),
       round(100.0 * count(*) / (SELECT count(*) FROM kigoto_planner), 2)
FROM kigoto_planner
WHERE geom && ST_MakeEnvelope(32.884, -2.5045, 32.8855, -2.5035, 4326)
UNION ALL SELECT 'medium', count(*),
       round(100.0 * count(*) / (SELECT count(*) FROM kigoto_planner), 2)
FROM kigoto_planner
WHERE geom && ST_MakeEnvelope(32.883, -2.505, 32.886, -2.503, 4326)
UNION ALL SELECT 'wide', count(*),
       round(100.0 * count(*) / (SELECT count(*) FROM kigoto_planner), 2)
FROM kigoto_planner
WHERE geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326)
ORDER BY 1;

-- Natural planner choice (all scan types enabled)
SET enable_indexscan = on;
SET enable_bitmapscan = on;
SET enable_seqscan = on;
SET geo_vec.query_search_list_size = 100;
SET geo_vec.spatial_brute_force_threshold = 5000;

CREATE TEMP TABLE planner_plans (
    bbox text PRIMARY KEY,
    plan jsonb NOT NULL
);

DO $$
DECLARE
    q text;
    j jsonb;
    cases text[][] := ARRAY[
        ARRAY['tiny',   '32.8848, -2.5038, 32.8852, -2.5036'],
        ARRAY['narrow', '32.884, -2.5045, 32.8855, -2.5035'],
        ARRAY['medium', '32.883, -2.505, 32.886, -2.503'],
        ARRAY['wide',   '32.881, -2.506, 32.887, -2.502']
    ];
    i int;
BEGIN
    FOR i IN 1 .. array_length(cases, 1) LOOP
        q := format($sql$
            EXPLAIN (FORMAT JSON, COSTS true)
            SELECT id
            FROM kigoto_planner
            WHERE geom && ST_MakeEnvelope(%s, 4326)
            ORDER BY embedding <=> (SELECT embedding FROM query_vec)
            LIMIT 20
        $sql$, cases[i][2]);
        EXECUTE q INTO j;
        INSERT INTO planner_plans (bbox, plan) VALUES (cases[i][1], j)
        ON CONFLICT (bbox) DO UPDATE SET plan = EXCLUDED.plan;
    END LOOP;
END $$;

\echo ''
\echo '=== Chosen plan summary (natural) ==='
SELECT
    p.bbox,
    COALESCE(
        (
            SELECT string_agg(DISTINCT trim(both '"' from idx::text), ', ' ORDER BY trim(both '"' from idx::text))
            FROM jsonb_path_query(p.plan, '$.**."Index Name"') AS t(idx)
        ),
        '(no Index Name — see node types)'
    ) AS indexes,
    (
        SELECT string_agg(DISTINCT trim(both '"' from ntype::text), ' / ' ORDER BY trim(both '"' from ntype::text))
        FROM jsonb_path_query(p.plan, '$.**."Node Type"') AS t(ntype)
    ) AS node_types,
    CASE
        WHEN p.plan::text ILIKE '%geovec%' OR p.plan::text ILIKE '%geo_vec%'
            THEN 'geo_vec'
        WHEN p.plan::text ILIKE '%hnsw%' AND p.plan::text ILIKE '%gist%'
            THEN 'hnsw+gist'
        WHEN p.plan::text ILIKE '%hnsw%'
            THEN 'hnsw'
        WHEN p.plan::text ILIKE '%gist%'
            THEN 'gist'
        WHEN p.plan::text ILIKE '%Seq Scan%'
            THEN 'seqscan'
        ELSE 'other'
    END AS chosen,
    (p.plan #>> '{0,Plan,"Total Cost"}')::numeric AS total_cost,
    (p.plan #>> '{0,Plan,"Plan Rows"}')::numeric AS plan_rows
FROM planner_plans p
ORDER BY array_position(ARRAY['tiny','narrow','medium','wide'], p.bbox);

\echo ''
\echo '=== Full EXPLAIN (COSTS) per bbox ==='
\echo '--- tiny ---'
EXPLAIN (COSTS true)
SELECT id FROM kigoto_planner
WHERE geom && ST_MakeEnvelope(32.8848, -2.5038, 32.8852, -2.5036, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec)
LIMIT 20;

\echo '--- narrow ---'
EXPLAIN (COSTS true)
SELECT id FROM kigoto_planner
WHERE geom && ST_MakeEnvelope(32.884, -2.5045, 32.8855, -2.5035, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec)
LIMIT 20;

\echo '--- medium ---'
EXPLAIN (COSTS true)
SELECT id FROM kigoto_planner
WHERE geom && ST_MakeEnvelope(32.883, -2.505, 32.886, -2.503, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec)
LIMIT 20;

\echo '--- wide ---'
EXPLAIN (COSTS true)
SELECT id FROM kigoto_planner
WHERE geom && ST_MakeEnvelope(32.881, -2.506, 32.887, -2.502, 4326)
ORDER BY embedding <=> (SELECT embedding FROM query_vec)
LIMIT 20;

-- Indexscan-only: both indexes still compete; bitmaps disabled.
\echo ''
\echo '=== Forced indexscan-only (both indexes still eligible) ==='
SET enable_bitmapscan = off;
SET enable_seqscan = off;

DO $$
DECLARE
    q text;
    j jsonb;
    cases text[][] := ARRAY[
        ARRAY['tiny',   '32.8848, -2.5038, 32.8852, -2.5036'],
        ARRAY['narrow', '32.884, -2.5045, 32.8855, -2.5035'],
        ARRAY['medium', '32.883, -2.505, 32.886, -2.503'],
        ARRAY['wide',   '32.881, -2.506, 32.887, -2.502']
    ];
    i int;
BEGIN
    DELETE FROM planner_plans;
    FOR i IN 1 .. array_length(cases, 1) LOOP
        q := format($sql$
            EXPLAIN (FORMAT JSON, COSTS true)
            SELECT id
            FROM kigoto_planner
            WHERE geom && ST_MakeEnvelope(%s, 4326)
            ORDER BY embedding <=> (SELECT embedding FROM query_vec)
            LIMIT 20
        $sql$, cases[i][2]);
        EXECUTE q INTO j;
        INSERT INTO planner_plans (bbox, plan) VALUES (cases[i][1], j);
    END LOOP;
END $$;

SELECT
    p.bbox,
    CASE
        WHEN p.plan::text ILIKE '%geovec%' OR p.plan::text ILIKE '%geo_vec%'
            THEN 'geo_vec'
        WHEN p.plan::text ILIKE '%hnsw%'
            THEN 'hnsw'
        WHEN p.plan::text ILIKE '%gist%'
            THEN 'gist'
        ELSE 'other'
    END AS chosen,
    (p.plan #>> '{0,Plan,"Total Cost"}')::numeric AS total_cost
FROM planner_plans p
ORDER BY array_position(ARRAY['tiny','narrow','medium','wide'], p.bbox);

\echo ''
\echo 'Done. Compare chosen column across bboxes before/after amcostestimate changes.'
\echo 'After cost_estimate edits: make bench-planner-choice with --rebuild.'
