#!/bin/bash
set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

GATEWAY_ADDR="127.0.0.1:8080"
DELAY_US=${1:-1000}
DURATION=${2:-10}
CONNECTIONS=${3:-64}
THREADS=${4:-4}

RESULTS_DIR="./benchmark_results"
mkdir -p $RESULTS_DIR

echo -e "${GREEN}=== Testing TCP Transport ===${NC}"
echo "Configuration: delay=${DELAY_US}us, duration=${DURATION}s, connections=${CONNECTIONS}"

# Clean up
pkill -f "target/release/backend" 2>/dev/null || true
pkill -f "target/release/gateway" 2>/dev/null || true
rm -f /tmp/backend.sock 2>/dev/null || true
rm -f /dev/shm/backend_* 2>/dev/null || true
sleep 2

# TCP Test
echo "Starting TCP backend..."
./target/release/backend --transport tcp --addr 127.0.0.1:50051 --delay-us $DELAY_US &
BACKEND_PID=$!
sleep 3

echo "Starting TCP gateway..."
./target/release/gateway --listen-addr $GATEWAY_ADDR --backend-addr 127.0.0.1:50051 --transport tcp &
GATEWAY_PID=$!
sleep 3

echo "Running TCP benchmark..."
wrk -t${THREADS} -c${CONNECTIONS} -d${DURATION}s --latency -s scripts/wrk.lua http://$GATEWAY_ADDR/echo > $RESULTS_DIR/tcp_result.txt 2>&1

echo -e "${GREEN}TCP Results:${NC}"
grep "Requests/sec" $RESULTS_DIR/tcp_result.txt
grep -A 1 "Latency Distribution" $RESULTS_DIR/tcp_result.txt | tail -1

kill $BACKEND_PID $GATEWAY_PID 2>/dev/null || true
sleep 2

echo ""
echo -e "${GREEN}=== Testing UDS Transport ===${NC}"

# Clean up
pkill -f "target/release/backend" 2>/dev/null || true
pkill -f "target/release/gateway" 2>/dev/null || true
rm -f /tmp/backend.sock 2>/dev/null || true
sleep 2

# UDS Test
echo "Starting UDS backend..."
./target/release/backend --transport uds --uds-path /tmp/backend.sock --delay-us $DELAY_US &
BACKEND_PID=$!
sleep 3

echo "Checking if UDS socket exists..."
ls -la /tmp/backend.sock || echo "Socket not found!"

echo "Starting UDS gateway..."
./target/release/gateway --listen-addr $GATEWAY_ADDR --transport uds --uds-path /tmp/backend.sock &
GATEWAY_PID=$!
sleep 3

echo "Running UDS benchmark..."
wrk -t${THREADS} -c${CONNECTIONS} -d${DURATION}s --latency -s scripts/wrk.lua http://$GATEWAY_ADDR/echo > $RESULTS_DIR/uds_result.txt 2>&1

echo -e "${GREEN}UDS Results:${NC}"
grep "Requests/sec" $RESULTS_DIR/uds_result.txt
grep -A 1 "Latency Distribution" $RESULTS_DIR/uds_result.txt | tail -1

kill $BACKEND_PID $GATEWAY_PID 2>/dev/null || true
sleep 2

echo ""
echo -e "${GREEN}=== Testing SHM Transport ===${NC}"

# Clean up
pkill -f "target/release/backend" 2>/dev/null || true
pkill -f "target/release/gateway" 2>/dev/null || true
rm -f /dev/shm/backend_* 2>/dev/null || true
sleep 2

# SHM Test
echo "Starting SHM backend..."
./target/release/backend --transport shm --shm-name backend --delay-us $DELAY_US &
BACKEND_PID=$!
sleep 3

echo "Checking shared memory..."
ls -la /dev/shm/backend_* || echo "Shared memory not found!"

echo "Starting SHM gateway..."
./target/release/gateway --listen-addr $GATEWAY_ADDR --backend-addr backend --transport shm --shm-name backend &
GATEWAY_PID=$!
sleep 3

echo "Running SHM benchmark..."
wrk -t${THREADS} -c${CONNECTIONS} -d${DURATION}s --latency -s scripts/wrk.lua http://$GATEWAY_ADDR/echo > $RESULTS_DIR/shm_result.txt 2>&1

echo -e "${GREEN}SHM Results:${NC}"
grep "Requests/sec" $RESULTS_DIR/shm_result.txt
grep -A 1 "Latency Distribution" $RESULTS_DIR/shm_result.txt | tail -1

kill $BACKEND_PID $GATEWAY_PID 2>/dev/null || true

echo ""
echo -e "${GREEN}=== All Tests Completed ===${NC}"
