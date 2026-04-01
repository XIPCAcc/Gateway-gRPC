#!/bin/bash

# Performance comparison script: TCP vs UDS vs SHM vs SHM-Linux vs SHM-Eventfd
# This script tests all implementations and compares their performance

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
LOG_DIR="$PROJECT_ROOT/log"

cd "$PROJECT_ROOT"

# Create log directory if it doesn't exist
mkdir -p "$LOG_DIR"

echo "=========================================="
echo "Performance Comparison Test"
echo "TCP vs UDS vs SHM vs SHM-UDS vs SHM-Eventfd"
echo "=========================================="
echo ""

# Test configuration
DELAY_US=1000
GATEWAY_PORT=8082
TCP_BACKEND_PORT=50053
SHM_NAME="backend"
TEST_DURATION=30

echo "Configuration:"
echo "  Delay: ${DELAY_US}us"
echo "  Gateway Port: $GATEWAY_PORT"
echo "  Test Duration: ${TEST_DURATION}s"
echo ""

# Clean up any existing processes
echo "Cleaning up existing processes..."
pkill -9 backend 2>/dev/null || true
pkill -9 gateway 2>/dev/null || true
sleep 2

# Clean up shared memory and sockets
rm -f /dev/shm/backend_* 2>/dev/null || true
rm -f /tmp/backend*.sock 2>/dev/null || true

echo "Building release binaries..."
cargo build --release 2>&1 | grep -E "(Compiling|Finished)" || true

run_test() {
    local name=$1
    local backend_cmd=$2
    local gateway_cmd=$3
    
    echo ""
    echo "=========================================="
    echo "Test: $name"
    echo "=========================================="
    echo ""
    
    # Start backend
    echo "Starting backend ($name)..."
    $backend_cmd > "$LOG_DIR/backend_${name}.log" 2>&1 &
    BACKEND_PID=$!
    sleep 2
    
    if ! kill -0 $BACKEND_PID 2>/dev/null; then
        echo "ERROR: Backend failed to start"
        cat "$LOG_DIR/backend_${name}.log"
        return 1
    fi
    echo "Backend started (PID: $BACKEND_PID)"
    
    # Start gateway
    echo "Starting gateway ($name)..."
    $gateway_cmd > "$LOG_DIR/gateway_${name}.log" 2>&1 &
    GATEWAY_PID=$!
    sleep 2
    
    if ! kill -0 $GATEWAY_PID 2>/dev/null; then
        echo "ERROR: Gateway failed to start"
        cat "$LOG_DIR/gateway_${name}.log"
        kill $BACKEND_PID 2>/dev/null || true
        return 1
    fi
    echo "Gateway started (PID: $GATEWAY_PID)"
    
    # Run tests
    echo ""
    echo "Test 1: Low Concurrency (64 connections)"
    wrk -t8 -c64 -d${TEST_DURATION}s --latency \
        -s "$SCRIPT_DIR/wrk.lua" \
        "http://127.0.0.1:$GATEWAY_PORT/api/echo" \
        2>&1 | tee "$LOG_DIR/${name}_test_low.log"
    
    echo ""
    echo "Test 2: Medium Concurrency (256 connections)"
    wrk -t8 -c256 -d${TEST_DURATION}s --latency \
        -s "$SCRIPT_DIR/wrk.lua" \
        "http://127.0.0.1:$GATEWAY_PORT/api/echo" \
        2>&1 | tee "$LOG_DIR/${name}_test_medium.log"
    
    echo ""
    echo "Test 3: High Concurrency (1024 connections)"
    wrk -t8 -c1024 -d${TEST_DURATION}s --latency \
        -s "$SCRIPT_DIR/wrk.lua" \
        "http://127.0.0.1:$GATEWAY_PORT/api/echo" \
        2>&1 | tee "$LOG_DIR/${name}_test_high.log"
    
    # Cleanup
    echo ""
    echo "Cleaning up..."
    kill $GATEWAY_PID 2>/dev/null || true
    kill $BACKEND_PID 2>/dev/null || true
    rm -f /dev/shm/backend_* 2>/dev/null || true
    rm -f /tmp/backend*.sock 2>/dev/null || true
    sleep 2
}

# Test TCP
run_test "tcp" \
    "./target/release/backend --transport tcp --addr 127.0.0.1:$TCP_BACKEND_PORT --delay-us $DELAY_US" \
    "./target/release/gateway --transport tcp --backend-addr 127.0.0.1:$TCP_BACKEND_PORT --listen-addr 127.0.0.1:$GATEWAY_PORT"

# Test UDS
run_test "uds" \
    "./target/release/backend --transport uds --uds-path /tmp/backend.sock --delay-us $DELAY_US" \
    "./target/release/gateway --transport uds --uds-path /tmp/backend.sock --listen-addr 127.0.0.1:$GATEWAY_PORT"

# Test SHM (original)
run_test "shm" \
    "./target/release/backend --transport shm --shm-name backend --delay-us $DELAY_US" \
    "./target/release/gateway --transport shm --shm-name backend --listen-addr 127.0.0.1:$GATEWAY_PORT"

# Test SHM-UDS (Unix Domain Socket notification)
run_test "shm-uds" \
    "./target/release/backend --transport shm-uds --shm-name backend --delay-us $DELAY_US" \
    "./target/release/gateway --transport shm-uds --shm-name backend --listen-addr 127.0.0.1:$GATEWAY_PORT"

# Test SHM-Eventfd (eventfd notification)
run_test "shm-eventfd" \
    "./target/release/backend --transport shm-eventfd --shm-name backend --delay-us $DELAY_US" \
    "./target/release/gateway --transport shm-eventfd --shm-name backend --listen-addr 127.0.0.1:$GATEWAY_PORT"

# Test SHM-UINTR (UINTR notification)
run_test "shm-uintr" \
    "./target/release/backend --transport shm-uintr --shm-name backend --delay-us $DELAY_US" \
    "./target/release/gateway --transport shm-uintr --shm-name backend --listen-addr 127.0.0.1:$GATEWAY_PORT"

echo ""
echo "=========================================="
echo "All Tests Complete!"
echo "=========================================="
echo ""
echo "Results saved to:"
echo "  $LOG_DIR/*_test_*.log"
