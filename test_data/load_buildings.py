#!/usr/bin/env python3
"""Load building_detection_embeddings.parquet into PostgreSQL for testing."""

import sys
import struct
import psycopg2
import pyarrow.parquet as pq

DB = sys.argv[1] if len(sys.argv) > 1 else "postgres"
HOST = sys.argv[2] if len(sys.argv) > 2 else "localhost"

conn = psycopg2.connect(dbname=DB, host=HOST)
conn.autocommit = True
cur = conn.cursor()

# Create extensions
cur.execute("CREATE EXTENSION IF NOT EXISTS postgis;")
cur.execute("CREATE EXTENSION IF NOT EXISTS vector;")
cur.execute('CREATE EXTENSION IF NOT EXISTS geo_vec;')
cur.execute('CREATE EXTENSION IF NOT EXISTS vectorscale CASCADE;')

# Create table
cur.execute("DROP TABLE IF EXISTS buildings CASCADE;")
cur.execute("""
    CREATE TABLE buildings (
        id serial PRIMARY KEY,
        embedding vector(1024),
        geom geometry(Geometry, 4326)
    );
""")

# Load parquet
t = pq.read_table("test_data/building_detection_embeddings.parquet")
embeddings = t.column("embedding")
geometries = t.column("geometry")

print(f"Loading {len(t)} rows...")

batch_size = 500
values_list = []
for i in range(len(t)):
    emb = embeddings[i].as_py()
    emb_str = "[" + ",".join(str(v) for v in emb) + "]"
    geom_bytes = geometries[i].as_py()
    geom_hex = geom_bytes.hex()
    values_list.append(f"('{emb_str}'::vector, ST_GeomFromWKB(decode('{geom_hex}', 'hex'), 4326))")

    if len(values_list) >= batch_size:
        sql = "INSERT INTO buildings (embedding, geom) VALUES " + ",".join(values_list) + ";"
        cur.execute(sql)
        values_list = []
        if (i + 1) % 2000 == 0:
            print(f"  loaded {i+1} rows...")

if values_list:
    sql = "INSERT INTO buildings (embedding, geom) VALUES " + ",".join(values_list) + ";"
    cur.execute(sql)

cur.execute("SELECT count(*) FROM buildings;")
count = cur.fetchone()[0]
print(f"Loaded {count} rows into buildings table.")

# Print spatial extent
cur.execute("""
    SELECT ST_XMin(e), ST_XMax(e), ST_YMin(e), ST_YMax(e)
    FROM (SELECT ST_Extent(geom) AS e FROM buildings) t;
""")
ext = cur.fetchone()
print(f"Spatial extent: x=[{ext[0]:.6f}, {ext[1]:.6f}], y=[{ext[2]:.6f}, {ext[3]:.6f}]")

cur.close()
conn.close()
print("Done.")
