-- One-time setup: create separate tables per index approach
-- Run by run_benchmark.sh on first init only
\set ON_ERROR_STOP on
\timing on

-- buildings base table already exists (loaded by load_buildings.py)
-- It has no vector/spatial indexes — used for ground truth seq scans

\echo '>>> Creating buildings_geovec table (copy for geo_vec index)...'
CREATE TABLE buildings_geovec AS TABLE buildings;
ALTER TABLE buildings_geovec ADD PRIMARY KEY (id);

\echo '>>> Creating buildings_hnsw table (copy for HNSW + GiST indexes)...'
CREATE TABLE buildings_hnsw AS TABLE buildings;
ALTER TABLE buildings_hnsw ADD PRIMARY KEY (id);

\echo '>>> Building geo_vec index on buildings_geovec...'
SET maintenance_work_mem = '512MB';
CREATE INDEX buildings_geovec_idx ON buildings_geovec
    USING geo_vec (embedding vector_cosine_ops, geom geometry_geo_vec_ops)
    WITH (storage_layout = plain);

\echo '>>> Building HNSW index on buildings_hnsw...'
CREATE INDEX buildings_hnsw_idx ON buildings_hnsw
    USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 100);

\echo '>>> Building GiST index on buildings_hnsw...'
CREATE INDEX buildings_hnsw_gist_idx ON buildings_hnsw USING gist (geom);

\echo '>>> Creating buildings_diskann table (copy for DiskANN + GiST indexes)...'
CREATE TABLE buildings_diskann AS TABLE buildings;
ALTER TABLE buildings_diskann ADD PRIMARY KEY (id);

\echo '>>> Building DiskANN index on buildings_diskann...'
CREATE INDEX buildings_diskann_idx ON buildings_diskann
    USING diskann (embedding vector_cosine_ops);

\echo '>>> Building GiST index on buildings_diskann...'
CREATE INDEX buildings_diskann_gist_idx ON buildings_diskann USING gist (geom);

ANALYZE buildings;
ANALYZE buildings_geovec;
ANALYZE buildings_hnsw;
ANALYZE buildings_diskann;

\echo '>>> Setup complete!'
