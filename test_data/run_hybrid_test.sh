#!/bin/bash
set -euo pipefail

echo "========================================="
echo "  Hybrid Spatial-Seeded Graph Search Test"
echo "========================================="

cd /workspace

# ---------- Download kigoto parquet if missing ----------
PARQUET="/workspace/test_data/kigoto_embeddings.parquet"
PARQUET_URL="https://geobase-docs.s3.amazonaws.com/geobase-ai-assets/duckdb_geoembeddings/kigoto_embeddings.parquet"
if [ ! -f "$PARQUET" ]; then
    echo ">>> Downloading kigoto_embeddings.parquet..."
    curl -fSL -o "$PARQUET" "$PARQUET_URL"
fi

# ---------- Detect architecture for SIMD flags ----------
ARCH=$(uname -m)
if [ "$ARCH" = "x86_64" ]; then
    export RUSTFLAGS="-C target-feature=+avx2,+fma"
elif [ "$ARCH" = "aarch64" ] || [ "$ARCH" = "arm64" ]; then
    export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-$(sw_vers -productVersion 2>/dev/null || echo '')}"
    export RUSTFLAGS="-C target-feature=+neon -C link-arg=-Wl,-undefined,dynamic_lookup"
else
    echo "WARNING: Unknown architecture $ARCH, building without SIMD flags"
fi

# ---------- Build extension ----------
echo ""
echo ">>> Building extension (release) for $ARCH..."
cargo pgrx install --release --no-default-features --features pg17 2>&1

# Symlink .so if pgrx installs with a different name than the control file expects
PG_LIB=$(pg_config --pkglibdir)
ls -la "$PG_LIB"/geo*vec* 2>/dev/null || true

# ---------- Initialize PostgreSQL ----------
echo ""
echo ">>> Initializing PostgreSQL..."
mkdir -p /var/run/postgresql /var/lib/postgresql/17/main
chown postgres:postgres /var/run/postgresql /var/lib/postgresql/17/main
su postgres -c "/usr/lib/postgresql/17/bin/initdb -D /var/lib/postgresql/17/main"

cat >> /var/lib/postgresql/17/main/postgresql.conf <<EOF
shared_buffers = '512MB'
work_mem = '128MB'
maintenance_work_mem = '2GB'
effective_cache_size = '1GB'
max_parallel_maintenance_workers = 4
max_worker_processes = 8
max_parallel_workers = 8
EOF

echo ">>> Starting PostgreSQL..."
su postgres -c "/usr/lib/postgresql/17/bin/pg_ctl -D /var/lib/postgresql/17/main start -l /tmp/pg.log -o '-k /var/run/postgresql'"
for i in $(seq 1 10); do
    su postgres -c "pg_isready -h /var/run/postgresql" >/dev/null 2>&1 && echo ">>> Ready." && break
    sleep 1
done

# ---------- Load data (both datasets) ----------
echo ""
echo ">>> Loading buildings data (8,494 rows, 1024-dim)..."
su postgres -c "python3 /workspace/test_data/load_buildings.py postgres /var/run/postgresql"

echo ""
echo ">>> Loading kigoto data (318K rows, 384-dim)..."
su postgres -c "python3 /workspace/test_data/load_kigoto.py postgres /var/run/postgresql"

# ---------- Create indexes IN PARALLEL ----------
echo ""
echo ">>> Creating geo_vec indexes in parallel..."
echo "    (buildings + kigoto concurrently)"

# Buildings index (small, fast — serial, below parallel threshold) — background
su postgres -c "psql -h /var/run/postgresql -d postgres -c \"
SET maintenance_work_mem = '512MB';
CREATE INDEX buildings_geo_vec_idx ON buildings
    USING geo_vec (embedding vector_cosine_ops, geom geometry_geo_vec_ops);
\"" &
PID_BUILDINGS=$!

# Kigoto index (large — parallel build with SBQ compression, 318K > 65536 threshold) — background
su postgres -c "psql -h /var/run/postgresql -d postgres -c \"
SET maintenance_work_mem = '2GB';
CREATE INDEX kigoto_geo_vec_idx ON kigoto
    USING geo_vec (embedding vector_cosine_ops, geom geometry_geo_vec_ops);
\"" &
PID_KIGOTO=$!

echo "    Buildings PID: $PID_BUILDINGS"
echo "    Kigoto PID: $PID_KIGOTO"

# Wait for both
FAIL=0
wait $PID_BUILDINGS || { echo "ERROR: Buildings index build failed!"; FAIL=1; }
echo "    Buildings index done."
wait $PID_KIGOTO || { echo "ERROR: Kigoto index build failed!"; FAIL=1; }
echo "    Kigoto index done."

if [ $FAIL -ne 0 ]; then
    echo "Index creation failed. Check /tmp/pg.log for details."
    exit 1
fi

# Analyze both tables
su postgres -c "psql -h /var/run/postgresql -d postgres -c 'ANALYZE buildings; ANALYZE kigoto;'"

# ---------- Run tests ----------
echo ""
echo ">>> Running hybrid search tests..."
echo ""
su postgres -c "psql -h /var/run/postgresql -d postgres -f /workspace/test_data/test_hybrid_search.sql"

echo ""
echo "========================================="
echo "  Tests complete!"
echo "========================================="
