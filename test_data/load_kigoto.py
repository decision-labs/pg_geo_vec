#!/usr/bin/env python3
"""Load kigoto_embeddings.parquet into PostgreSQL for benchmarking."""

import sys
import psycopg2

DB = sys.argv[1] if len(sys.argv) > 1 else "postgres"
HOST = sys.argv[2] if len(sys.argv) > 2 else "/var/run/postgresql"

conn = psycopg2.connect(dbname=DB, host=HOST)
conn.autocommit = True
cur = conn.cursor()

cur.execute("CREATE EXTENSION IF NOT EXISTS postgis;")
cur.execute("CREATE EXTENSION IF NOT EXISTS vector;")
cur.execute('CREATE EXTENSION IF NOT EXISTS geo_vec;')

cur.execute("DROP TABLE IF EXISTS kigoto CASCADE;")
cur.execute("""
    CREATE TABLE kigoto (
        id serial PRIMARY KEY,
        embedding vector(384),
        geom geometry(Geometry, 4326)
    );
""")

import pyarrow.parquet as pq

t = pq.read_table("/workspace/test_data/kigoto_embeddings.parquet")
embeddings = t.column("embedding")
geometries = t.column("geometry")

print(f"Loading {len(t)} rows...")

batch_size = 1000
values_list = []
for i in range(len(t)):
    emb = embeddings[i].as_py()
    emb_str = "[" + ",".join(str(v) for v in emb) + "]"
    geom_bytes = geometries[i].as_py()
    geom_hex = geom_bytes.hex()
    values_list.append(f"('{emb_str}'::vector, ST_GeomFromWKB(decode('{geom_hex}', 'hex'), 4326))")

    if len(values_list) >= batch_size:
        sql = "INSERT INTO kigoto (embedding, geom) VALUES " + ",".join(values_list) + ";"
        cur.execute(sql)
        values_list = []
        if (i + 1) % 50000 == 0:
            print(f"  loaded {i+1} rows...")

if values_list:
    sql = "INSERT INTO kigoto (embedding, geom) VALUES " + ",".join(values_list) + ";"
    cur.execute(sql)

cur.execute("SELECT count(*) FROM kigoto;")
count = cur.fetchone()[0]
print(f"Loaded {count} rows into kigoto table.")

cur.execute("""
    SELECT ST_XMin(e), ST_XMax(e), ST_YMin(e), ST_YMax(e)
    FROM (SELECT ST_Extent(geom) AS e FROM kigoto) t;
""")
ext = cur.fetchone()
print(f"Spatial extent: x=[{ext[0]:.6f}, {ext[1]:.6f}], y=[{ext[2]:.6f}, {ext[3]:.6f}]")

cur.close()
conn.close()
print("Done.")
