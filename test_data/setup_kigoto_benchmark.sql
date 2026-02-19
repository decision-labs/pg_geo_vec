-- One-time setup: create separate tables per index approach
-- Run by run_kigoto_benchmark.sh on first init only
\set ON_ERROR_STOP on
\timing on

-- kigoto base table already exists (loaded by load_kigoto.py)
-- It has no vector/spatial indexes — used for ground truth seq scans

\echo '>>> Creating kigoto_geovec table (copy for geo_vec index)...'
CREATE TABLE kigoto_geovec AS TABLE kigoto;
ALTER TABLE kigoto_geovec ADD PRIMARY KEY (id);

\echo '>>> Creating kigoto_hnsw table (copy for HNSW + GiST indexes)...'
CREATE TABLE kigoto_hnsw AS TABLE kigoto;
ALTER TABLE kigoto_hnsw ADD PRIMARY KEY (id);

\echo '>>> Building geo_vec index on kigoto_geovec...'
SET maintenance_work_mem = '1GB';
CREATE INDEX kigoto_geovec_idx ON kigoto_geovec
    USING geo_vec (embedding vector_cosine_ops, geom geometry_geo_vec_ops)
    WITH (storage_layout = plain);

\echo '>>> Building HNSW index on kigoto_hnsw...'
CREATE INDEX kigoto_hnsw_idx ON kigoto_hnsw
    USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 100);

\echo '>>> Building GiST index on kigoto_hnsw...'
CREATE INDEX kigoto_hnsw_gist_idx ON kigoto_hnsw USING gist (geom);

\echo '>>> Creating kigoto_diskann table (copy for DiskANN + GiST indexes)...'
CREATE TABLE kigoto_diskann AS TABLE kigoto;
ALTER TABLE kigoto_diskann ADD PRIMARY KEY (id);

\echo '>>> Building DiskANN index on kigoto_diskann...'
SET maintenance_work_mem = '1GB';
CREATE INDEX kigoto_diskann_idx ON kigoto_diskann
    USING diskann (embedding vector_cosine_ops);

\echo '>>> Building GiST index on kigoto_diskann...'
CREATE INDEX kigoto_diskann_gist_idx ON kigoto_diskann USING gist (geom);

ANALYZE kigoto;
ANALYZE kigoto_geovec;
ANALYZE kigoto_hnsw;
ANALYZE kigoto_diskann;

\echo '>>> Setup complete!'
