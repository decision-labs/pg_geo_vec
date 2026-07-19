IMAGE := geo_vec_test
# Local builds from Containerfile.test are often tagged :pg17 (see docker images).
DOCKER_IMAGE ?= $(IMAGE):pg17
PG_FEATURE := pg17
RUSTFLAGS_X86 := -C target-feature=+avx2,+fma

.PHONY: docker-image test test-unit test-integration clippy bench-buildings bench-kigoto bench-buildings-rebuild bench-kigoto-rebuild bench-planner-choice bench-planner-choice-rebuild

test-unit:
	cargo test --no-default-features --features $(PG_FEATURE)

test-integration: docker-image
	docker run --rm -v $(PWD):/workspace $(DOCKER_IMAGE) \
		bash -c 'cd /workspace && cargo pgrx test --no-default-features --features $(PG_FEATURE)'

test: test-unit

clippy:
	cargo clippy --no-default-features --features $(PG_FEATURE)

docker-image:
	docker build -t $(DOCKER_IMAGE) -f Containerfile.test .

bench-buildings: docker-image
	docker run --rm -v $(PWD):/workspace -v buildings_pgdata:/pgdata $(DOCKER_IMAGE) \
		bash /workspace/test_data/run_benchmark.sh

bench-kigoto: docker-image
	docker run --rm -v $(PWD):/workspace -v kigoto_pgdata:/pgdata $(DOCKER_IMAGE) \
		bash /workspace/test_data/run_kigoto_benchmark.sh

bench-buildings-rebuild: docker-image
	docker run --rm -v $(PWD):/workspace -v buildings_pgdata:/pgdata $(DOCKER_IMAGE) \
		bash /workspace/test_data/run_benchmark.sh --rebuild

bench-kigoto-rebuild: docker-image
	docker run --rm -v $(PWD):/workspace -v kigoto_pgdata:/pgdata $(DOCKER_IMAGE) \
		bash /workspace/test_data/run_kigoto_benchmark.sh --rebuild

# EXPLAIN smoke: geo_vec vs HNSW+GiST on one table (issue #4 planner cardinality).
# Reuses kigoto_pgdata. First run builds kigoto_planner (~minutes for indexes).
bench-planner-choice:
	docker run --rm -v $(PWD):/workspace -v kigoto_pgdata:/pgdata $(DOCKER_IMAGE) \
		bash /workspace/test_data/run_planner_choice.sh

bench-planner-choice-rebuild:
	docker run --rm -v $(PWD):/workspace -v kigoto_pgdata:/pgdata $(DOCKER_IMAGE) \
		bash /workspace/test_data/run_planner_choice.sh --rebuild