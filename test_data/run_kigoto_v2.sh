#!/bin/bash
set -euo pipefail

echo "========================================="
echo "  Kigoto 318K Benchmark v2"
echo "========================================="

cd /workspace

PARQUET="/workspace/test_data/kigoto_embeddings.parquet"
PARQUET_URL="https://geobase-docs.s3.amazonaws.com/geobase-ai-assets/duckdb_geoembeddings/kigoto_embeddings.parquet"
if [ ! -f "$PARQUET" ]; then
    echo ">>> Downloading kigoto_embeddings.parquet..."
    curl -fSL -o "$PARQUET" "$PARQUET_URL"
fi

echo ">>> Building extension (release)..."
RUSTFLAGS="-C target-feature=+avx2,+fma" cargo pgrx install --release --no-default-features --features pg17 2>&1

PG_LIB=$(pg_config --pkglibdir)
[ -f "$PG_LIB/geo-vec-0.1.0.so" ] && [ ! -f "$PG_LIB/pg_geo_vec-0.1.0.so" ] && \
    ln -sf "$PG_LIB/geo-vec-0.1.0.so" "$PG_LIB/pg_geo_vec-0.1.0.so"
[ -f "$PG_LIB/pg_geo_vec-0.1.0.so" ] && [ ! -f "$PG_LIB/geo-vec-0.1.0.so" ] && \
    ln -sf "$PG_LIB/pg_geo_vec-0.1.0.so" "$PG_LIB/geo-vec-0.1.0.so"

echo ">>> Initializing PostgreSQL..."
mkdir -p /var/run/postgresql /var/lib/postgresql/17/main
chown postgres:postgres /var/run/postgresql /var/lib/postgresql/17/main
su postgres -c "/usr/lib/postgresql/17/bin/initdb -D /var/lib/postgresql/17/main"

cat >> /var/lib/postgresql/17/main/postgresql.conf <<EOF
shared_buffers = '512MB'
work_mem = '128MB'
maintenance_work_mem = '2GB'
effective_cache_size = '1GB'
max_parallel_maintenance_workers = 0
EOF

echo ">>> Starting PostgreSQL..."
su postgres -c "/usr/lib/postgresql/17/bin/pg_ctl -D /var/lib/postgresql/17/main start -l /tmp/pg.log -o '-k /var/run/postgresql'"
for i in $(seq 1 10); do
    su postgres -c "pg_isready -h /var/run/postgresql" >/dev/null 2>&1 && echo ">>> Ready." && break
    sleep 1
done

echo ">>> Loading kigoto data..."
su postgres -c "python3 /workspace/test_data/load_kigoto.py postgres /var/run/postgresql"

echo ">>> Creating geo_vec index..."
su postgres -c "psql -h /var/run/postgresql -d postgres -c \"
SET maintenance_work_mem = '2GB';
CREATE INDEX kigoto_geo_vec_idx ON kigoto
    USING geo_vec (embedding vector_cosine_ops, geom geometry_geo_vec_ops)
    WITH (storage_layout = plain);
\""

echo ""
echo ">>> Running benchmark v2..."
su postgres -c "psql -h /var/run/postgresql -d postgres -f /workspace/test_data/benchmark_kigoto_v2.sql"
