#!/bin/bash

# TCP Transport Test Script
# Tests gateway-gRPC with TCP transport

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
LOG_DIR="$PROJECT_ROOT/log"

cd "$PROJECT_ROOT"

# Create log directory if it doesn't exist
mkdir -p "$LOG_DIR"

echo "=========================================="
echo "TCP Transport Test"
echo "=========================================="
echo ""

# Configuration
BACKEND_ADDR="127.0.0.1:50053"
GATEWAY_ADDR="127.0.0.1:8080"
DELAY_US=${1:-1000}
TEST_DURATION=${2:-30}

echo "Configuration:"
echo "  Backend Address: $BACKEND_ADDR"
echo "  Gateway Address: $GATEWAY_ADDR"
echo "  Backend Delay: ${DELAY_US}us"
echo "  Test Duration: ${TEST_DURATION}s"
echo ""

# Clean up existing processes
echo "Cleaning up existing processes..."
pkill -9 backend 2>/dev/null || true
pkill -9 gateway 2>/dev/null || true
sleep 2

# Build binaries
echo "Building release binaries..."
cargo build --release 2>&1 | grep -E "(Compiling|Finished)" || true

# Start backend
echo ""
echo "Starting backend (TCP)..."
./target/release/backend \
    --transport tcp \
    --addr "$BACKEND_ADDR" \
    --delay-us "$DELAY_US" \
    > "$LOG_DIR/backend_tcp.log" 2>&1 &
BACKEND_PID=$!
sleep 2

if ! kill -0 $BACKEND_PID 2>/dev/null; then
    echo "ERROR: Backend failed to start"
    cat "$LOG_DIR/backend_tcp.log"
    exit 1
fi
echo "Backend started (PID: $BACKEND_PID)"

# Start gateway
echo "Starting gateway (TCP)..."
./target/release/gateway \
    --transport tcp \
    --backend-addr "$BACKEND_ADDR" \
    --listen-addr "$GATEWAY_ADDR" \
    > "$LOG_DIR/gateway_tcp.log" 2>&1 &
GATEWAY_PID=$!
sleep 2

if ! kill -0 $GATEWAY_PID 2>/dev/null; then
    echo "ERROR: Gateway failed to start"
    cat "$LOG_DIR/gateway_tcp.log"
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
    2>&1 | tee "$LOG_DIR/tcp_test_low.log"

echo ""
echo "Test 2: Medium Concurrency (256 connections)"
wrk -t8 -c256 -d${TEST_DURATION}s --latency \
    -s "$SCRIPT_DIR/wrk.lua" \
    "http://$GATEWAY_ADDR/api/echo" \
    2>&1 | tee "$LOG_DIR/tcp_test_medium.log"

echo ""
echo "Test 3: High Concurrency (1024 connections)"
wrk -t8 -c1024 -d${TEST_DURATION}s --latency \
    -s "$SCRIPT_DIR/wrk.lua" \
    "http://$GATEWAY_ADDR/api/echo" \
    2>&1 | tee "$LOG_DIR/tcp_test_high.log"

# Cleanup
echo ""
echo "=========================================="
echo "Cleaning up..."
echo "=========================================="
kill $GATEWAY_PID 2>/dev/null || true
kill $BACKEND_PID 2>/dev/null || true
sleep 2

echo ""
echo "=========================================="
echo "TCP Test Complete!"
echo "=========================================="
echo ""
echo "Results saved to:"
echo "  $LOG_DIR/tcp_test_low.log"
echo "  $LOG_DIR/tcp_test_medium.log"
echo "  $LOG_DIR/tcp_test_high.log"
echo ""
echo "Logs saved to:"
echo "  $LOG_DIR/backend_tcp.log"
echo "  $LOG_DIR/gateway_tcp.log"
