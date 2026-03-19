.PHONY: all build test clean run-backend run-gateway benchmark fmt clippy

# Default target
all: build

# Build all components
build:
	cargo build --release

# Build debug version
debug:
	cargo build

# Run tests
test:
	cargo test --all

# Format code
fmt:
	cargo fmt --all

# Run clippy
clippy:
	cargo clippy --all -- -D warnings

# Clean build artifacts
clean:
	cargo clean
	rm -rf results/

# Run backend service
run-backend:
	./target/release/backend --addr 127.0.0.1:50051 --delay-us 1000

# Run gateway (TCP)
run-gateway-tcp:
	./target/release/gateway --listen-addr 127.0.0.1:8080 --backend-addr 127.0.0.1:50051 --transport tcp

# Run gateway (UDS)
run-gateway-uds:
	./target/release/gateway --listen-addr 127.0.0.1:8080 --transport uds

# Run gateway (Shared Memory)
run-gateway-shm:
	./target/release/gateway --listen-addr 127.0.0.1:8080 --transport shm

# Quick HTTP benchmark
benchmark-http:
	./target/release/benchmark http --target http://127.0.0.1:8080 --connections 64 --duration-secs 30 --qps 10000

# Quick gRPC benchmark
benchmark-grpc:
	./target/release/benchmark grpc --target http://127.0.0.1:50051 --connections 64 --duration-secs 30 --qps 10000

# Full benchmark suite
benchmark-suite:
	./scripts/run_benchmark.sh

# Monitor resources
monitor:
	./scripts/monitor_resources.sh 60

# Setup (install dependencies)
setup:
	@echo "Installing dependencies..."
	@echo "Please install the following tools manually:"
	@echo "  - wrk: https://github.com/wg/wrk/wiki/Installing-Wrk-on-Linux"
	@echo "  - vegeta: go install github.com/tsenart/vegeta@latest"
	@echo "  - ghz: https://github.com/bojand/ghz#install"
	@echo "  - sysstat (for pidstat): apt-get install sysstat"
	@echo "  - linux-tools (for perf): apt-get install linux-tools-common"

# Docker build
docker-build:
	docker build -t gateway-grpc:latest .

# Docker run
docker-run:
	docker-compose up -d

# Docker stop
docker-stop:
	docker-compose down

# Generate documentation
doc:
	cargo doc --all --no-deps

# Pre-commit checks
check: fmt clippy test
	@echo "All checks passed!"

# Performance profiling with perf
profile-backend:
	perf record -g ./target/release/backend --addr 127.0.0.1:50051 --delay-us 1000
	perf report

profile-gateway:
	perf record -g ./target/release/gateway --listen-addr 127.0.0.1:8080 --transport tcp
	perf report

# Flamegraph generation (requires inferno or flamegraph.pl)
flamegraph-backend:
	perf record -g -- ./target/release/backend --addr 127.0.0.1:50051 --delay-us 1000 &
	 sleep 30 && kill %1
	perf script | inferno-collapse-perf | inferno-flamegraph > backend_flamegraph.svg

flamegraph-gateway:
	perf record -g -- ./target/release/gateway --listen-addr 127.0.0.1:8080 --transport tcp &
	 sleep 30 && kill %1
	perf script | inferno-collapse-perf | inferno-flamegraph > gateway_flamegraph.svg

# Help
help:
	@echo "Available targets:"
	@echo "  build              - Build all components (release)"
	@echo "  debug              - Build all components (debug)"
	@echo "  test               - Run all tests"
	@echo "  clean              - Clean build artifacts"
	@echo "  fmt                - Format code"
	@echo "  clippy             - Run clippy lints"
	@echo "  check              - Run all pre-commit checks"
	@echo "  run-backend        - Start backend service"
	@echo "  run-gateway-tcp    - Start gateway (TCP mode)"
	@echo "  run-gateway-uds    - Start gateway (UDS mode)"
	@echo "  run-gateway-shm    - Start gateway (Shared Memory mode)"
	@echo "  benchmark-http     - Run quick HTTP benchmark"
	@echo "  benchmark-grpc     - Run quick gRPC benchmark"
	@echo "  benchmark-suite    - Run full benchmark suite"
	@echo "  monitor            - Monitor system resources"
	@echo "  profile-backend    - Profile backend with perf"
	@echo "  profile-gateway    - Profile gateway with perf"
	@echo "  doc                - Generate documentation"
	@echo "  setup              - Show setup instructions"
	@echo "  help               - Show this help message"
