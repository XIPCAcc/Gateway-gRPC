# Benchmark Scripts

## Quick Start

```bash
# Start services
./scripts/start_services.sh [delay_us]

# Run benchmarks
./scripts/run_wrk.sh              # wrk benchmark
./scripts/run_ab.sh               # Apache Benchmark
./scripts/run_hey.sh              # hey benchmark (requires hey)
./scripts/run_ghz.sh              # ghz benchmark (requires ghz)

# Stop services
./scripts/stop_services.sh
```

## Complete Benchmark Suite

Run all available benchmarks:

```bash
./scripts/benchmark.sh [wrk|ab|hey|ghz|custom|all]
```

## Prerequisites

### wrk
```bash
# Ubuntu
sudo apt-get install wrk

# Build from source
git clone https://github.com/wg/wrk.git
cd wrk && make
sudo cp wrk /usr/local/bin/
```

### Apache Benchmark (ab)
```bash
# Ubuntu
sudo apt-get install apache2-utils
```

### hey
```bash
# Go install
go install github.com/rakyll/hey@latest

# Or download binary
# https://github.com/rakyll/hey/releases
```

### ghz
```bash
# Go install
go install github.com/bojand/ghz/cmd/ghz@latest

# Or download binary
# https://github.com/bojand/ghz/releases
```

## Usage Examples

### Start services with custom delay
```bash
./scripts/start_services.sh 100  # 100us delay
./scripts/start_services.sh 1000 # 1ms delay
```

### Run wrk with custom parameters
```bash
# Default: 64 256 512 1024 connections, 8 threads, 30s
./scripts/run_wrk.sh "64 256" 8 10  # 64 and 256 connections, 8 threads, 10s
```

### Run ab with custom parameters
```bash
# Default: 64 256 512 1024 connections, 30000 requests
./scripts/run_ab.sh "64 256" 10000  # 64 and 256 connections, 10000 requests
```

### Run hey with custom parameters
```bash
# Default: 64 256 512 1024 connections, 30s duration, 10000 QPS limit
./scripts/run_hey.sh "64 256" 20s 5000
```

### Run ghz with custom parameters
```bash
# Default: 64 256 512 1024 connections, 30000 calls
./scripts/run_ghz.sh "64 256" 10000
```

## Results

All results are saved in `./results/` directory with timestamps:
- `./results/wrk_YYYYMMDD_HHMMSS/`
- `./results/ab_YYYYMMDD_HHMMSS/`
- `./results/hey_YYYYMMDD_HHMMSS/`
- `./results/ghz_YYYYMMDD_HHMMSS/`
