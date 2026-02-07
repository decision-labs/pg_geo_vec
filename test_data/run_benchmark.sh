#!/bin/bash
set -euo pipefail

echo "========================================="
echo "  geo_vec vs pgvector+GiST Benchmark"
echo "========================================="

cd /workspace

# Build and install the extension
echo ""
echo ">>> Building extension (release)..."
RUSTFLAGS="-C target-feature=+avx2,+fma" cargo pgrx install --release --no-default-features --features pg17 2>&1

# Verify installed .so
PG_LIB=$(pg_config --pkglibdir)
ls -la "$PG_LIB"/geo_vec* 2>/dev/null || true

# Initialize and start PostgreSQL
echo ""
echo ">>> Initializing PostgreSQL..."
mkdir -p /var/run/postgresql /var/lib/postgresql/17/main
chown postgres:postgres /var/run/postgresql /var/lib/postgresql/17/main
su postgres -c "/usr/lib/postgresql/17/bin/initdb -D /var/lib/postgresql/17/main"

# Tune for benchmarking
cat >> /var/lib/postgresql/17/main/postgresql.conf <<EOF
shared_buffers = '256MB'
work_mem = '64MB'
maintenance_work_mem = '512MB'
effective_cache_size = '512MB'
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

# Load test data
echo ""
echo ">>> Loading building data (8,494 rows, 1024-dim)..."
su postgres -c "python3 /workspace/test_data/load_buildings.py postgres /var/run/postgresql"

# Create geo_vec index first (benchmark script expects it)
echo ""
echo ">>> Creating geo_vec index..."
su postgres -c "psql -h /var/run/postgresql -d postgres -c \"
SET maintenance_work_mem = '512MB';
CREATE INDEX buildings_geo_vec_idx ON buildings
    USING geo_vec (embedding vector_cosine_ops, geom geometry_geo_vec_ops)
    WITH (storage_layout = plain);
\""

# Run the benchmark
echo ""
echo ">>> Running benchmark comparison..."
echo ""
su postgres -c "psql -h /var/run/postgresql -d postgres -f /workspace/test_data/benchmark_comparison.sql"
