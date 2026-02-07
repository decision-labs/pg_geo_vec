#!/bin/bash
set -euo pipefail

echo "========================================="
echo "  Kigoto 318K Benchmark"
echo "========================================="

cd /workspace

PARQUET="/workspace/test_data/kigoto_embeddings.parquet"
PARQUET_URL="https://geobase-docs.s3.amazonaws.com/geobase-ai-assets/duckdb_geoembeddings/kigoto_embeddings.parquet"
if [ ! -f "$PARQUET" ]; then
    echo ">>> Downloading kigoto_embeddings.parquet..."
    curl -fSL -o "$PARQUET" "$PARQUET_URL"
fi

# Build and install
echo ""
echo ">>> Building extension (release)..."
RUSTFLAGS="-C target-feature=+avx2,+fma" cargo pgrx install --release --no-default-features --features pg17 2>&1

# Verify installed .so
PG_LIB=$(pg_config --pkglibdir)
ls -la "$PG_LIB"/geo_vec* 2>/dev/null || true

# Init and start PG
echo ""
echo ">>> Initializing PostgreSQL..."
mkdir -p /var/run/postgresql /var/lib/postgresql/17/main
chown postgres:postgres /var/run/postgresql /var/lib/postgresql/17/main
su postgres -c "/usr/lib/postgresql/17/bin/initdb -D /var/lib/postgresql/17/main"

cat >> /var/lib/postgresql/17/main/postgresql.conf <<EOF
shared_buffers = '512MB'
work_mem = '128MB'
maintenance_work_mem = '1GB'
effective_cache_size = '1GB'
max_parallel_maintenance_workers = 0
EOF

echo ""
echo ">>> Starting PostgreSQL..."
su postgres -c "/usr/lib/postgresql/17/bin/pg_ctl -D /var/lib/postgresql/17/main start -l /tmp/pg.log -o '-k /var/run/postgresql'"

for i in $(seq 1 10); do
    if su postgres -c "pg_isready -h /var/run/postgresql" >/dev/null 2>&1; then
        echo ">>> PostgreSQL is ready."
        break
    fi
    sleep 1
done

# Load data
echo ""
echo ">>> Loading kigoto data (318K rows, 384-dim)..."
su postgres -c "python3 /workspace/test_data/load_kigoto.py postgres /var/run/postgresql"

# Create geo_vec index
echo ""
echo ">>> Creating geo_vec index..."
su postgres -c "psql -h /var/run/postgresql -d postgres -c \"
SET maintenance_work_mem = '1GB';
CREATE INDEX kigoto_geo_vec_idx ON kigoto
    USING geo_vec (embedding vector_cosine_ops, geom geometry_geo_vec_ops)
    WITH (storage_layout = plain);
\""

# Run benchmark
echo ""
echo ">>> Running benchmark..."
echo ""
su postgres -c "psql -h /var/run/postgresql -d postgres -f /workspace/test_data/benchmark_kigoto.sql"
