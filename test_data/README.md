# Test data

## Datasets

| File | Used by | How to obtain |
|------|---------|----------------|
| `building_detection_embeddings.parquet` (~35 MB) | `ci_test.sh`, `load_buildings.py` | Currently committed. **Before public release:** confirm redistribution rights; prefer hosting + download (same pattern as kigoto) and stop tracking the binary. |
| `kigoto_embeddings.parquet` | kigoto / hybrid benchmarks | Not committed. Downloaded at runtime from `https://geobase-docs.s3.amazonaws.com/geobase-ai-assets/duckdb_geoembeddings/kigoto_embeddings.parquet`. |

## CI note

Integration CI expects `building_detection_embeddings.parquet` to be present in this directory when `load_buildings.py` runs.
