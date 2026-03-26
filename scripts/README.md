# Test Scripts

## Overview

This directory contains scripts for testing different transport implementations in the gateway-gRPC project.

## Available Scripts

### Individual Transport Tests

Each script tests a specific transport implementation:

| Script | Transport | Description |
|--------|-----------|-------------|
| `run_tcp.sh` | TCP | Standard TCP socket transport |
| `run_uds.sh` | UDS | Unix Domain Socket transport |
| `run_shm.sh` | SHM | Shared Memory with polling |
| `run_shm_uds.sh` | SHM-UDS | Shared Memory with UDS notification |
| `run_shm_eventfd.sh` | SHM-Eventfd | Shared Memory with eventfd notification |

### Comparison Tests

| Script | Description |
|--------|-------------|
| `compare_all_transports.sh` | Tests all transport implementations and compares performance |

## Usage

### Run Individual Transport Test

```bash
# Basic usage (default: 1000us delay, 30s test duration)
./scripts/run_tcp.sh
./scripts/run_uds.sh
./scripts/run_shm.sh
./scripts/run_shm_uds.sh
./scripts/run_shm_eventfd.sh

# Custom delay and test duration
./scripts/run_tcp.sh 500 60        # 500us delay, 60s test
./scripts/run_shm_eventfd.sh 2000 45  # 2000us delay, 45s test
```

### Run All Transports Comparison

```bash
# Test all transports with default settings
./scripts/compare_all_transports.sh
```

## Test Configuration

Each test runs three concurrency levels:
- **Low**: 64 connections
- **Medium**: 256 connections
- **High**: 1024 connections

Default parameters:
- Backend delay: 1000μs
- Test duration: 30s per concurrency level
- wrk threads: 8

## Output

### Log Directory

All logs are saved to the `./log/` directory in the project root:

```
log/
├── backend_tcp.log              # Backend logs
├── gateway_tcp.log              # Gateway logs
├── tcp_test_low.log            # Test results (64 connections)
├── tcp_test_medium.log         # Test results (256 connections)
├── tcp_test_high.log           # Test results (1024 connections)
├── backend_uds.log
├── gateway_uds.log
├── uds_test_low.log
├── uds_test_medium.log
├── uds_test_high.log
├── backend_shm.log
├── gateway_shm.log
├── shm_test_low.log
├── shm_test_medium.log
├── shm_test_high.log
├── backend_shm_uds.log
├── gateway_shm_uds.log
├── shm_uds_test_low.log
├── shm_uds_test_medium.log
├── shm_uds_test_high.log
├── backend_shm_eventfd.log
├── gateway_shm_eventfd.log
├── shm_eventfd_test_low.log
├── shm_eventfd_test_medium.log
└── shm_eventfd_test_high.log
```

### Test Results

Each test result file contains:
- Request latency statistics (avg, stdev, min, max)
- Requests per second (QPS)
- Latency distribution (50%, 75%, 90%, 99%)
- Transfer statistics

## Prerequisites

### wrk (HTTP benchmarking tool)

```bash
# Ubuntu/Debian
sudo apt-get install wrk

# Build from source
git clone https://github.com/wg/wrk.git
cd wrk && make
sudo cp wrk /usr/local/bin/
```

## Port Allocation

Each transport uses a different gateway port:

| Transport | Gateway Port |
|-----------|---------------|
| TCP | 8080 |
| UDS | 8081 |
| SHM | 8082 |
| SHM-UDS | 8083 |
| SHM-Eventfd | 8084 |

## Troubleshooting

### Port Already in Use

If you see "Address already in use" error:
```bash
# Kill existing processes
pkill -9 backend
pkill -9 gateway
```

### Shared Memory Cleanup

If SHM tests fail:
```bash
# Clean up shared memory
rm -f /dev/shm/backend_*

# Clean up sockets
rm -f /tmp/backend*.sock
```

### View Logs

Check logs for errors:
```bash
# View backend log
cat log/backend_tcp.log

# View gateway log
cat log/gateway_tcp.log

# View test results
cat log/tcp_test_high.log
```

## Performance Comparison

After running `compare_all_transports.sh`, you can compare results:

```bash
# View all high-concurrency results
cat log/*_test_high.log | grep "Requests/sec"

# Compare latency
cat log/*_test_high.log | grep "Latency"
```

## Expected Performance

Based on previous benchmarks:

| Transport | QPS (1024 conn) | Avg Latency |
|-----------|-------------------|-------------|
| TCP | ~33.4K | ~30.66ms |
| UDS | ~36.2K | ~28.19ms |
| SHM | ~476 | ~833.36ms |
| SHM-UDS | ~92K | ~11.07ms |
| SHM-Eventfd | ~93.6K | ~10.87ms |

Note: Actual performance may vary based on system configuration and workload.
