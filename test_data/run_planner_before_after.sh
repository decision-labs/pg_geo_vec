#!/bin/bash
# Before/after planner-choice for amcostestimate (issue #4).
set -euo pipefail
cd /workspace

ARCH=$(uname -m)
if [ "$ARCH" = "x86_64" ]; then
    export RUSTFLAGS="-C target-feature=+avx2,+fma"
fi

PGDATA=/pgdata
mkdir -p /var/run/postgresql
chown postgres:postgres /var/run/postgresql

if [ ! -f "$PGDATA/PG_VERSION" ]; then
    echo "ERROR: kigoto_pgdata not initialized; run make bench-kigoto first."
    exit 1
fi

echo ">>> Starting PostgreSQL..."
su postgres -c "/usr/lib/postgresql/17/bin/pg_ctl -D $PGDATA start -l /tmp/pg.log -o '-k /var/run/postgresql'" || true
for i in $(seq 1 15); do
    su postgres -c "pg_isready -h /var/run/postgresql" >/dev/null 2>&1 && break
    sleep 1
done

HAS_KIGOTO=$(su postgres -c "psql -h /var/run/postgresql -d postgres -Atc \"SELECT to_regclass('public.kigoto') IS NOT NULL\"")
if [ "$HAS_KIGOTO" != "t" ]; then
    echo "ERROR: public.kigoto missing in volume"
    exit 1
fi

cp /workspace/test_data/_cost_estimate.before.rs src/access_method/cost_estimate.rs

install_ext() {
    echo ""
    echo ">>> Building+installing geo_vec ($1)..."
    cargo pgrx install --release --no-default-features --features pg17 2>&1 | tail -20
    # Force sessions to load new .so
    su postgres -c "/usr/lib/postgresql/17/bin/pg_ctl -D $PGDATA restart -l /tmp/pg.log -o '-k /var/run/postgresql'" || true
    sleep 2
    for i in $(seq 1 15); do
        su postgres -c "pg_isready -h /var/run/postgresql" >/dev/null 2>&1 && break
        sleep 1
    done
}

run_smoke() {
    local label=$1
    local out=/tmp/planner_${label}.txt
    echo ""
    echo ">>> Running planner smoke ($label)..."
    su postgres -c "psql -h /var/run/postgresql -d postgres -v ON_ERROR_STOP=1 -f /workspace/test_data/planner_choice_kigoto.sql" \
        >"$out" 2>&1 || { echo "FAILED $label"; tail -40 "$out"; exit 1; }
    echo "=== $label: Chosen plan summary (natural) ==="
    awk '
      /^=== Chosen plan summary \(natural\) ===/ {p=1; print; next}
      p && /^=== / {exit}
      p {print}
    ' "$out"
    cp "$out" "/workspace/test_data/planner_choice_${label}.log"
}

install_ext before
run_smoke before

cp /workspace/test_data/_cost_estimate.after.rs src/access_method/cost_estimate.rs
install_ext after
run_smoke after

# Leave working tree on the after implementation.
cp /workspace/test_data/_cost_estimate.after.rs src/access_method/cost_estimate.rs

echo ""
echo "========================================="
echo "  BEFORE vs AFTER (chosen + cost)"
echo "========================================="
echo ""
echo "--- BEFORE (n/100) ---"
awk '
  /^=== Chosen plan summary \(natural\) ===/ {p=1; next}
  p && /^=== / {exit}
  p && NF {print}
' /tmp/planner_before.txt
echo ""
echo "--- AFTER (spatial+LIMIT-aware) ---"
awk '
  /^=== Chosen plan summary \(natural\) ===/ {p=1; next}
  p && /^=== / {exit}
  p && NF {print}
' /tmp/planner_after.txt

echo ""
echo "Full logs: test_data/planner_choice_before.log / planner_choice_after.log"
