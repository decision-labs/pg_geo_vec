#!/bin/bash
set -euo pipefail

REBUILD_GEOVEC=false
for arg in "$@"; do
    case $arg in
        --rebuild) REBUILD_GEOVEC=true ;;
    esac
done

echo "========================================="
echo "  geo_vec vs pgvector+GiST Benchmark"
echo "  (buildings — 8,494 rows, 1024-dim)"
echo "========================================="

cd /workspace

PGDATA="/pgdata"

# Detect architecture for SIMD flags
ARCH=$(uname -m)
if [ "$ARCH" = "x86_64" ]; then
    export RUSTFLAGS="-C target-feature=+avx2,+fma"
elif [ "$ARCH" = "aarch64" ] || [ "$ARCH" = "arm64" ]; then
    export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-$(sw_vers -productVersion 2>/dev/null || echo '')}"
    export RUSTFLAGS="-C target-feature=+neon -C link-arg=-Wl,-undefined,dynamic_lookup"
else
    echo "WARNING: Unknown architecture $ARCH, building without SIMD flags"
fi

# Always rebuild extension .so (source code may have changed)
echo ""
echo ">>> Building extension (release) for $ARCH..."
cargo pgrx install --release --no-default-features --features pg17 2>&1

PG_LIB=$(pg_config --pkglibdir)
ls -la "$PG_LIB"/geo_vec* 2>/dev/null || true

# ---------- PostgreSQL init / start ----------
FRESH=false
if [ ! -f "$PGDATA/PG_VERSION" ]; then
    FRESH=true
    echo ""
    echo ">>> Initializing PostgreSQL (fresh data dir)..."
    mkdir -p /var/run/postgresql "$PGDATA"
    chown postgres:postgres /var/run/postgresql "$PGDATA"
    su postgres -c "/usr/lib/postgresql/17/bin/initdb -D $PGDATA"

    cat >> "$PGDATA/postgresql.conf" <<EOF
shared_buffers = '256MB'
work_mem = '64MB'
maintenance_work_mem = '512MB'
effective_cache_size = '512MB'
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

# ---------- One-time setup ----------
if [ "$FRESH" = true ]; then
    echo ""
    echo ">>> Loading building data (8,494 rows, 1024-dim)..."
    su postgres -c "python3 /workspace/test_data/load_buildings.py postgres /var/run/postgresql"

    echo ""
    echo ">>> Creating benchmark tables and indexes (one-time setup)..."
    su postgres -c "psql -h /var/run/postgresql -d postgres -f /workspace/test_data/setup_buildings_benchmark.sql"
fi

# ---------- Optional geo_vec index rebuild ----------
if [ "$REBUILD_GEOVEC" = true ] && [ "$FRESH" = false ]; then
    echo ""
    echo ">>> Rebuilding geo_vec index..."
    su postgres -c "psql -h /var/run/postgresql -d postgres -c \"
        DROP INDEX IF EXISTS buildings_geovec_idx;
        SET maintenance_work_mem = '512MB';
        CREATE INDEX buildings_geovec_idx ON buildings_geovec
            USING geo_vec (embedding vector_cosine_ops, geom geometry_geo_vec_ops)
            WITH (storage_layout = plain);
        ANALYZE buildings_geovec;
    \""
fi

# ---------- Run benchmark ----------
echo ""
echo ">>> Running benchmark comparison..."
echo ""
su postgres -c "psql -h /var/run/postgresql -d postgres -f /workspace/test_data/benchmark_comparison.sql"
