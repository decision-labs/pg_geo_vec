#!/bin/bash
# Planner-choice smoke: EXPLAIN picks among geo_vec vs HNSW+GiST on one table.
# Reuses the kigoto_pgdata volume (same as make bench-kigoto).
set -euo pipefail

REBUILD_GEOVEC=false
for arg in "$@"; do
    case $arg in
        --rebuild) REBUILD_GEOVEC=true ;;
    esac
done

echo "========================================="
echo "  Kigoto planner-choice smoke"
echo "========================================="

cd /workspace

PGDATA="/pgdata"
PARQUET="/workspace/test_data/kigoto_embeddings.parquet"
PARQUET_URL="https://geobase-docs.s3.amazonaws.com/geobase-ai-assets/duckdb_geoembeddings/kigoto_embeddings.parquet"

if [ ! -f "$PARQUET" ]; then
    echo ">>> Downloading kigoto_embeddings.parquet..."
    curl -fSL -o "$PARQUET" "$PARQUET_URL"
fi

ARCH=$(uname -m)
if [ "$ARCH" = "x86_64" ]; then
    export RUSTFLAGS="-C target-feature=+avx2,+fma"
elif [ "$ARCH" = "aarch64" ] || [ "$ARCH" = "arm64" ]; then
    export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-$(sw_vers -productVersion 2>/dev/null || echo '')}"
    export RUSTFLAGS="-C target-feature=+neon -C link-arg=-Wl,-undefined,dynamic_lookup"
else
    echo "WARNING: Unknown architecture $ARCH, building without SIMD flags"
fi

echo ""
echo ">>> Building extension (release) for $ARCH..."
cargo pgrx install --release --no-default-features --features pg17 2>&1

FRESH=false
if [ ! -f "$PGDATA/PG_VERSION" ]; then
    FRESH=true
    echo ""
    echo ">>> Initializing PostgreSQL (fresh data dir)..."
    mkdir -p /var/run/postgresql "$PGDATA"
    chown postgres:postgres /var/run/postgresql "$PGDATA"
    su postgres -c "/usr/lib/postgresql/17/bin/initdb -D $PGDATA"

    cat >> "$PGDATA/postgresql.conf" <<EOF
shared_buffers = '1GB'
work_mem = '128MB'
maintenance_work_mem = '1GB'
effective_cache_size = '3GB'
max_parallel_maintenance_workers = 0
EOF
else
    echo ""
    echo ">>> Reusing existing data dir at $PGDATA"
    mkdir -p /var/run/postgresql
    chown postgres:postgres /var/run/postgresql
fi

echo ""
echo ">>> Starting PostgreSQL..."
su postgres -c "/usr/lib/postgresql/17/bin/pg_ctl -D $PGDATA start -l /tmp/pg.log -o '-k /var/run/postgresql'"

for i in $(seq 1 10); do
    if su postgres -c "pg_isready -h /var/run/postgresql" >/dev/null 2>&1; then
        echo ">>> PostgreSQL is ready."
        break
    fi
    sleep 1
done

if [ "$FRESH" = true ]; then
    echo ""
    echo ">>> Loading kigoto data..."
    su postgres -c "python3 /workspace/test_data/load_kigoto.py postgres /var/run/postgresql"
fi

# Ensure base kigoto table exists (volume may be from prior bench with only copies).
HAS_KIGOTO=$(su postgres -c "psql -h /var/run/postgresql -d postgres -Atc \"SELECT to_regclass('public.kigoto') IS NOT NULL\"")
if [ "$HAS_KIGOTO" != "t" ]; then
    echo ">>> kigoto missing; loading..."
    su postgres -c "python3 /workspace/test_data/load_kigoto.py postgres /var/run/postgresql"
fi

if [ "$REBUILD_GEOVEC" = true ]; then
    echo ""
    echo ">>> Dropping kigoto_planner so indexes rebuild with current geo_vec..."
    su postgres -c "psql -h /var/run/postgresql -d postgres -c 'DROP TABLE IF EXISTS kigoto_planner CASCADE;'"
fi

echo ""
echo ">>> Running planner-choice smoke..."
echo ""
su postgres -c "psql -h /var/run/postgresql -d postgres -f /workspace/test_data/planner_choice_kigoto.sql"
