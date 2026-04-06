#!/bin/bash

# 比较三种 SHM 传输方式的性能

set -e

DELAY_US=1000
SHM_NAME="backend"
GATEWAY_PORT=8080
TIMESTAMP=$(date +"%Y%m%d_%H%M%S")
LOG_DIR="log/${TIMESTAMP}"
TEST_DURATION=30

echo "Building project..."
cargo build --release

mkdir -p $LOG_DIR

pkill -9 backend 2>/dev/null || true
pkill -9 gateway 2>/dev/null || true
sleep 2

run_test() {
    local name=$1
    local backend_cmd=$2
    local gateway_cmd=$3
    local concurrency=$4
    
    echo ""
    echo "=========================================="
    echo "Testing $name ($concurrency connections)..."
    echo "=========================================="
    echo ""
    
    rm -f /dev/shm/${SHM_NAME}_* 2>/dev/null || true
    rm -f /tmp/${SHM_NAME}_*.sock 2>/dev/null || true
    
    $backend_cmd > $LOG_DIR/${name}_backend_${concurrency}.log 2>&1 &
    BACKEND_PID=$!
    
    sleep 3
    
    $gateway_cmd > $LOG_DIR/${name}_gateway_${concurrency}.log 2>&1 &
    GATEWAY_PID=$!
    
    sleep 2
    
    echo "Running wrk with $concurrency connections..."
    wrk -t8 -c$concurrency -d${TEST_DURATION}s --latency \
        -s scripts/wrk.lua \
        "http://127.0.0.1:$GATEWAY_PORT/api/echo" 2>&1 | tee $LOG_DIR/${name}_test_${concurrency}.log
    
    kill $GATEWAY_PID $BACKEND_PID 2>/dev/null || true
    sleep 2
    rm -f /dev/shm/${SHM_NAME}_* 2>/dev/null || true
    rm -f /tmp/${SHM_NAME}_*.sock 2>/dev/null || true
    sleep 1
}

for concurrency in 64 256 1024; do
    run_test "shm-uds" \
        "./target/release/backend --transport shm-uds --shm-name $SHM_NAME --delay-us $DELAY_US" \
        "./target/release/gateway --transport shm-uds --shm-name $SHM_NAME --listen-addr 127.0.0.1:$GATEWAY_PORT" \
        $concurrency
done

for concurrency in 64 256 1024; do
    run_test "shm-eventfd" \
        "./target/release/backend --transport shm-eventfd --shm-name $SHM_NAME --delay-us $DELAY_US" \
        "./target/release/gateway --transport shm-eventfd --shm-name $SHM_NAME --listen-addr 127.0.0.1:$GATEWAY_PORT" \
        $concurrency
done

for concurrency in 64 256 1024; do
    run_test "shm-uintr" \
        "./target/release/backend --transport shm-uintr --shm-name $SHM_NAME --delay-us $DELAY_US" \
        "./target/release/gateway --transport shm-uintr --shm-name $SHM_NAME --listen-addr 127.0.0.1:$GATEWAY_PORT" \
        $concurrency
done

echo ""
echo "=========================================="
echo "All Tests Complete!"
echo "=========================================="
echo ""
echo "Results saved to:"
echo "  $LOG_DIR/shm-uds_test_*.log"
echo "  $LOG_DIR/shm-eventfd_test_*.log"
echo "  $LOG_DIR/shm-uintr_test_*.log"
