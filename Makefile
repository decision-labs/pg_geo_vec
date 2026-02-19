IMAGE := geo_vec_test

.PHONY: docker-image bench-buildings bench-kigoto bench-buildings-rebuild bench-kigoto-rebuild

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
