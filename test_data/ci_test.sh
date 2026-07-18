#!/bin/bash
# CI integration test: build extension, load buildings data, run recall tests
# Optional: PG_MAJOR=17|18 (default 17)
set -euo pipefail

cd /workspace

PG_MAJOR="${PG_MAJOR:-17}"
PG_FEATURE="pg${PG_MAJOR}"
PG_BIN="/usr/lib/postgresql/${PG_MAJOR}/bin"
PGDATA="/var/lib/postgresql/${PG_MAJOR}/main"

echo ">>> Target PostgreSQL ${PG_MAJOR} (feature ${PG_FEATURE})"

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
echo ">>> Building geo_vec extension (release) for $ARCH / PG${PG_MAJOR}..."
cargo pgrx install --release --no-default-features --features "${PG_FEATURE}" 2>&1

echo ""
echo ">>> Installed files:"
PG_LIB=$(pg_config --pkglibdir)
ls -la "$PG_LIB"/geo_vec* 2>/dev/null || true
ls -la "$(pg_config --sharedir)"/extension/geo_vec* 2>/dev/null || true

# ---------- Initialize PostgreSQL ----------
echo ""
echo ">>> Initializing PostgreSQL ${PG_MAJOR}..."
mkdir -p /var/run/postgresql "${PGDATA}"
chown postgres:postgres /var/run/postgresql "${PGDATA}"
su postgres -c "${PG_BIN}/initdb -D ${PGDATA}"

cat >> "${PGDATA}/postgresql.conf" <<EOF
shared_buffers = '256MB'
work_mem = '64MB'
maintenance_work_mem = '512MB'
EOF

echo ">>> Starting PostgreSQL..."
su postgres -c "${PG_BIN}/pg_ctl -D ${PGDATA} start -l /tmp/pg.log -o '-k /var/run/postgresql'"
for i in $(seq 1 10); do
    su postgres -c "pg_isready -h /var/run/postgresql" >/dev/null 2>&1 && echo ">>> Ready." && break
    sleep 1
done

# ---------- Load data ----------
echo ""
echo ">>> Loading buildings data (8,494 rows, 1024-dim)..."
su postgres -c "python3 /workspace/test_data/load_buildings.py postgres /var/run/postgresql"

# ---------- Create index ----------
echo ""
echo ">>> Creating geo_vec index..."
su postgres -c "psql -h /var/run/postgresql -d postgres -c \"
SET maintenance_work_mem = '512MB';
CREATE INDEX buildings_geo_vec_idx ON buildings
    USING geo_vec (embedding vector_cosine_ops, geom geometry_geo_vec_ops);
ANALYZE buildings;
\""

# ---------- Run tests ----------
echo ""
echo ">>> Running spatial integration tests..."
su postgres -c "psql -h /var/run/postgresql -d postgres -f /workspace/test_data/test_buildings_only.sql"

echo ""
echo ">>> Running label filtering tests..."
su postgres -c "psql -h /var/run/postgresql -d postgres -f /workspace/test_data/test_label_filtering.sql"

echo ""
echo ">>> Coexistence smoke: build diskann index alongside geo_vec..."
su postgres -c "psql -h /var/run/postgresql -d postgres -c \"
CREATE INDEX IF NOT EXISTS buildings_diskann_idx ON buildings
    USING diskann (embedding vector_cosine_ops);
SELECT amname
FROM pg_index i
JOIN pg_class c ON c.oid = i.indexrelid
JOIN pg_am am ON am.oid = c.relam
WHERE i.indrelid = 'buildings'::regclass
ORDER BY amname;
\""

echo ""
echo ">>> All CI tests passed (PG${PG_MAJOR})!"
