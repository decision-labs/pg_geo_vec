#!/bin/bash
# CI integration test: build extension, load buildings data, run recall tests
set -euo pipefail

cd /workspace

# ---------- Build extension ----------
echo ">>> Building geo_vec extension (release)..."
RUSTFLAGS="-C target-feature=+avx2,+fma" cargo pgrx install --release --no-default-features --features pg17 2>&1

echo ""
echo ">>> Installed files:"
PG_LIB=$(pg_config --pkglibdir)
ls -la "$PG_LIB"/geo_vec* 2>/dev/null || true
ls -la "$(pg_config --sharedir)"/extension/geo_vec* 2>/dev/null || true

# ---------- Initialize PostgreSQL ----------
echo ""
echo ">>> Initializing PostgreSQL..."
mkdir -p /var/run/postgresql /var/lib/postgresql/17/main
chown postgres:postgres /var/run/postgresql /var/lib/postgresql/17/main
su postgres -c "/usr/lib/postgresql/17/bin/initdb -D /var/lib/postgresql/17/main"

cat >> /var/lib/postgresql/17/main/postgresql.conf <<EOF
shared_buffers = '256MB'
work_mem = '64MB'
maintenance_work_mem = '512MB'
EOF

echo ">>> Starting PostgreSQL..."
su postgres -c "/usr/lib/postgresql/17/bin/pg_ctl -D /var/lib/postgresql/17/main start -l /tmp/pg.log -o '-k /var/run/postgresql'"
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
echo ">>> All CI tests passed!"
