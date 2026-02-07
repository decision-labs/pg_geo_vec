#!/bin/bash
set -euo pipefail

echo "========================================="
echo "  geo_vec Spatial Cell Index Tests"
echo "========================================="

cd /workspace

# Build and install the extension
echo ""
echo ">>> Building extension (release)..."
RUSTFLAGS="-C target-feature=+avx2,+fma" cargo pgrx install --release --no-default-features --features pg17 2>&1

# Fix library naming mismatch
PG_LIB=$(pg_config --pkglibdir)
if [ -f "$PG_LIB/geo-vec-0.1.0.so" ] && [ ! -f "$PG_LIB/pg_geo_vec-0.1.0.so" ]; then
    echo ">>> Fixing library symlink..."
    ln -sf "$PG_LIB/geo-vec-0.1.0.so" "$PG_LIB/pg_geo_vec-0.1.0.so"
fi
# Also check the reverse
if [ -f "$PG_LIB/pg_geo_vec-0.1.0.so" ] && [ ! -f "$PG_LIB/geo-vec-0.1.0.so" ]; then
    ln -sf "$PG_LIB/pg_geo_vec-0.1.0.so" "$PG_LIB/geo-vec-0.1.0.so"
fi

echo ">>> Extension files:"
ls -la "$PG_LIB"/geo*vec* "$PG_LIB"/pg_geo_vec* 2>/dev/null || true
ls -la /usr/share/postgresql/17/extension/geo-vec* 2>/dev/null || true

# Initialize and start PostgreSQL
echo ""
echo ">>> Initializing PostgreSQL..."
mkdir -p /var/lib/postgresql/17/main
chown postgres:postgres /var/lib/postgresql/17/main
su postgres -c "/usr/lib/postgresql/17/bin/initdb -D /var/lib/postgresql/17/main"

echo ""
echo ">>> Starting PostgreSQL..."
su postgres -c "/usr/lib/postgresql/17/bin/pg_ctl -D /var/lib/postgresql/17/main start -l /tmp/pg.log -o '-k /var/run/postgresql'"

# Wait for PG to be ready
for i in $(seq 1 10); do
    if su postgres -c "pg_isready -h /var/run/postgresql" >/dev/null 2>&1; then
        echo ">>> PostgreSQL is ready."
        break
    fi
    sleep 1
done

# Ensure socket directory exists
mkdir -p /var/run/postgresql
chown postgres:postgres /var/run/postgresql

# Install python deps for data loading
echo ""
echo ">>> Installing Python dependencies..."
pip3 install --break-system-packages pyarrow psycopg2-binary 2>&1 | tail -3

# Create the postgres database if it doesn't exist
su postgres -c "psql -h /var/run/postgresql -c 'SELECT 1;'" 2>/dev/null || true

# Load test data
echo ""
echo ">>> Loading building data..."
su postgres -c "python3 /workspace/test_data/load_buildings.py postgres /var/run/postgresql"

# Run the SQL tests
echo ""
echo ">>> Running spatial cell index tests..."
echo ""
su postgres -c "psql -h /var/run/postgresql -d postgres -f /workspace/test_data/test_spatial_cell_index.sql"

echo ""
echo "========================================="
echo "  Tests complete!"
echo "========================================="
