#!/bin/bash

# SHM Transport Test Script (Polling)
# Tests gateway-gRPC with Shared Memory transport using polling

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
LOG_DIR="$PROJECT_ROOT/log"

cd "$PROJECT_ROOT"

# Create log directory if it doesn't exist
mkdir -p "$LOG_DIR"

echo "=========================================="
echo "SHM Transport Test (Polling)"
echo "=========================================="
echo ""

# Configuration
SHM_NAME="backend"
GATEWAY_ADDR="127.0.0.1:8082"
DELAY_US=${1:-1000}
TEST_DURATION=${2:-30}

echo "Configuration:"
echo "  SHM Name: $SHM_NAME"
echo "  Gateway Address: $GATEWAY_ADDR"
echo "  Backend Delay: ${DELAY_US}us"
echo "  Test Duration: ${TEST_DURATION}s"
echo ""

# Clean up existing processes and shared memory
echo "Cleaning up existing processes and shared memory..."
pkill -9 backend 2>/dev/null || true
pkill -9 gateway 2>/dev/null || true
rm -f /dev/shm/${SHM_NAME}_* 2>/dev/null || true
sleep 2

# Build binaries
echo "Building release binaries..."
cargo build --release 2>&1 | grep -E "(Compiling|Finished)" || true

# Start backend
echo ""
echo "Starting backend (SHM)..."
./target/release/backend \
    --transport shm \
    --shm-name "$SHM_NAME" \
    --delay-us "$DELAY_US" \
    > "$LOG_DIR/backend_shm.log" 2>&1 &
BACKEND_PID=$!
sleep 2

if ! kill -0 $BACKEND_PID 2>/dev/null; then
    echo "ERROR: Backend failed to start"
    cat "$LOG_DIR/backend_shm.log"
    exit 1
fi
echo "Backend started (PID: $BACKEND_PID)"

# Start gateway
echo "Starting gateway (SHM)..."
./target/release/gateway \
    --transport shm \
    --shm-name "$SHM_NAME" \
    --listen-addr "$GATEWAY_ADDR" \
    > "$LOG_DIR/gateway_shm.log" 2>&1 &
GATEWAY_PID=$!
sleep 2

if ! kill -0 $GATEWAY_PID 2>/dev/null; then
    echo "ERROR: Gateway failed to start"
    cat "$LOG_DIR/gateway_shm.log"
    kill $BACKEND_PID 2>/dev/null || true
    exit 1
fi
echo "Gateway started (PID: $GATEWAY_PID)"

# Run tests
echo ""
echo "=========================================="
echo "Running Performance Tests"
echo "=========================================="
echo ""

echo "Test 1: Low Concurrency (64 connections)"
wrk -t8 -c64 -d${TEST_DURATION}s --latency \
    -s "$SCRIPT_DIR/wrk.lua" \
    "http://$GATEWAY_ADDR/api/echo" \
    2>&1 | tee "$LOG_DIR/shm_test_low.log"

echo ""
echo "Test 2: Medium Concurrency (256 connections)"
wrk -t8 -c256 -d${TEST_DURATION}s --latency \
    -s "$SCRIPT_DIR/wrk.lua" \
    "http://$GATEWAY_ADDR/api/echo" \
    2>&1 | tee "$LOG_DIR/shm_test_medium.log"

echo ""
echo "Test 3: High Concurrency (1024 connections)"
wrk -t8 -c1024 -d${TEST_DURATION}s --latency \
    -s "$SCRIPT_DIR/wrk.lua" \
    "http://$GATEWAY_ADDR/api/echo" \
    2>&1 | tee "$LOG_DIR/shm_test_high.log"

# Cleanup
echo ""
echo "=========================================="
echo "Cleaning up..."
echo "=========================================="
kill $GATEWAY_PID 2>/dev/null || true
kill $BACKEND_PID 2>/dev/null || true
rm -f /dev/shm/${SHM_NAME}_* 2>/dev/null || true
sleep 2

echo ""
echo "=========================================="
echo "SHM Test Complete!"
echo "=========================================="
echo ""
echo "Results saved to:"
echo "  $LOG_DIR/shm_test_low.log"
echo "  $LOG_DIR/shm_test_medium.log"
echo "  $LOG_DIR/shm_test_high.log"
echo ""
echo "Logs saved to:"
echo "  $LOG_DIR/backend_shm.log"
echo "  $LOG_DIR/gateway_shm.log"
