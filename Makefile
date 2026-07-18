IMAGE := geo_vec_test
PG_FEATURE := pg17
RUSTFLAGS_X86 := -C target-feature=+avx2,+fma

.PHONY: docker-image test test-unit test-integration clippy bench-buildings bench-kigoto bench-buildings-rebuild bench-kigoto-rebuild

test-unit:
	cargo test --no-default-features --features $(PG_FEATURE)

test-integration: docker-image
	docker run --rm -v $(PWD):/workspace $(IMAGE) \
		bash -c 'cd /workspace && cargo pgrx test --no-default-features --features $(PG_FEATURE)'

test: test-unit

clippy:
	cargo clippy --no-default-features --features $(PG_FEATURE)

docker-image:
	docker build -t $(IMAGE) -f Containerfile.test .

bench-buildings: docker-image
	docker run --rm -v $(PWD):/workspace -v buildings_pgdata:/pgdata $(IMAGE) \
		bash /workspace/test_data/run_benchmark.sh

bench-kigoto: docker-image
	docker run --rm -v $(PWD):/workspace -v kigoto_pgdata:/pgdata $(IMAGE) \
		bash /workspace/test_data/run_kigoto_benchmark.sh

bench-buildings-rebuild: docker-image
	docker run --rm -v $(PWD):/workspace -v buildings_pgdata:/pgdata $(IMAGE) \
		bash /workspace/test_data/run_benchmark.sh --rebuild

bench-kigoto-rebuild: docker-image
	docker run --rm -v $(PWD):/workspace -v kigoto_pgdata:/pgdata $(IMAGE) \
		bash /workspace/test_data/run_kigoto_benchmark.sh --rebuild
