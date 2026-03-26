#!/bin/bash

# Run all transport tests and save results
set -e

cd /home/zwp/gateway-gRPC

# Create log directory
mkdir -p log

echo "=========================================="
echo "Running All Transport Tests"
echo "=========================================="
echo ""

# Clean up
pkill -9 backend 2>/dev/null || true
pkill -9 gateway 2>/dev/null || true
sleep 2
rm -f /dev/shm/backend_* /tmp/backend*.sock 2>/dev/null || true

# Test TCP
echo "Testing TCP..."
./target/release/backend --transport tcp --addr 127.0.0.1:50051 --delay-us 1000 > log/backend_tcp.log 2>&1 &
BACKEND_PID=$!
sleep 2
./target/release/gateway --transport tcp --backend-addr 127.0.0.1:50051 --listen-addr 127.0.0.1:8080 > log/gateway_tcp.log 2>&1 &
GATEWAY_PID=$!
sleep 2
wrk -t8 -c64 -d30s --latency -s scripts/wrk.lua http://127.0.0.1:8080/api/echo 2>&1 | tee log/tcp_test_low.log
wrk -t8 -c256 -d30s --latency -s scripts/wrk.lua http://127.0.0.1:8080/api/echo 2>&1 | tee log/tcp_test_medium.log
wrk -t8 -c1024 -d30s --latency -s scripts/wrk.lua http://127.0.0.1:8080/api/echo 2>&1 | tee log/tcp_test_high.log
kill $GATEWAY_PID 2>/dev/null || true
kill $BACKEND_PID 2>/dev/null || true
sleep 2

# Test UDS
echo "Testing UDS..."
./target/release/backend --transport uds --uds-path /tmp/backend.sock --delay-us 1000 > log/backend_uds.log 2>&1 &
BACKEND_PID=$!
sleep 2
./target/release/gateway --transport uds --uds-path /tmp/backend.sock --listen-addr 127.0.0.1:8081 > log/gateway_uds.log 2>&1 &
GATEWAY_PID=$!
sleep 2
wrk -t8 -c64 -d30s --latency -s scripts/wrk.lua http://127.0.0.1:8081/api/echo 2>&1 | tee log/uds_test_low.log
wrk -t8 -c256 -d30s --latency -s scripts/wrk.lua http://127.0.0.1:8081/api/echo 2>&1 | tee log/uds_test_medium.log
wrk -t8 -c1024 -d30s --latency -s scripts/wrk.lua http://127.0.0.1:8081/api/echo 2>&1 | tee log/uds_test_high.log
kill $GATEWAY_PID 2>/dev/null || true
kill $BACKEND_PID 2>/dev/null || true
rm -f /tmp/backend.sock
sleep 2

# Test SHM
echo "Testing SHM..."
./target/release/backend --transport shm --shm-name backend --delay-us 1000 > log/backend_shm.log 2>&1 &
BACKEND_PID=$!
sleep 2
./target/release/gateway --transport shm --shm-name backend --listen-addr 127.0.0.1:8082 > log/gateway_shm.log 2>&1 &
GATEWAY_PID=$!
sleep 2
wrk -t8 -c64 -d30s --latency -s scripts/wrk.lua http://127.0.0.1:8082/api/echo 2>&1 | tee log/shm_test_low.log
wrk -t8 -c256 -d30s --latency -s scripts/wrk.lua http://127.0.0.1:8082/api/echo 2>&1 | tee log/shm_test_medium.log
wrk -t8 -c1024 -d30s --latency -s scripts/wrk.lua http://127.0.0.1:8082/api/echo 2>&1 | tee log/shm_test_high.log
kill $GATEWAY_PID 2>/dev/null || true
kill $BACKEND_PID 2>/dev/null || true
rm -f /dev/shm/backend_*
sleep 2

# Test SHM-UDS
echo "Testing SHM-UDS..."
./target/release/backend --transport shm-uds --shm-name backend --delay-us 1000 > log/backend_shm_uds.log 2>&1 &
BACKEND_PID=$!
sleep 2
./target/release/gateway --transport shm-uds --shm-name backend --listen-addr 127.0.0.1:8083 > log/gateway_shm_uds.log 2>&1 &
GATEWAY_PID=$!
sleep 2
wrk -t8 -c64 -d30s --latency -s scripts/wrk.lua http://127.0.0.1:8083/api/echo 2>&1 | tee log/shm_uds_test_low.log
wrk -t8 -c256 -d30s --latency -s scripts/wrk.lua http://127.0.0.1:8083/api/echo 2>&1 | tee log/shm_uds_test_medium.log
wrk -t8 -c1024 -d30s --latency -s scripts/wrk.lua http://127.0.0.1:8083/api/echo 2>&1 | tee log/shm_uds_test_high.log
kill $GATEWAY_PID 2>/dev/null || true
kill $BACKEND_PID 2>/dev/null || true
rm -f /dev/shm/backend_* /tmp/backend_*.sock
sleep 2

# Test SHM-Eventfd
echo "Testing SHM-Eventfd..."
./target/release/backend --transport shm-eventfd --shm-name backend --delay-us 1000 > log/backend_shm_eventfd.log 2>&1 &
BACKEND_PID=$!
sleep 2
./target/release/gateway --transport shm-eventfd --shm-name backend --listen-addr 127.0.0.1:8084 > log/gateway_shm_eventfd.log 2>&1 &
GATEWAY_PID=$!
sleep 2
wrk -t8 -c64 -d30s --latency -s scripts/wrk.lua http://127.0.0.1:8084/api/echo 2>&1 | tee log/shm_eventfd_test_low.log
wrk -t8 -c256 -d30s --latency -s scripts/wrk.lua http://127.0.0.1:8084/api/echo 2>&1 | tee log/shm_eventfd_test_medium.log
wrk -t8 -c1024 -d30s --latency -s scripts/wrk.lua http://127.0.0.1:8084/api/echo 2>&1 | tee log/shm_eventfd_test_high.log
kill $GATEWAY_PID 2>/dev/null || true
kill $BACKEND_PID 2>/dev/null || true
rm -f /dev/shm/backend_* /tmp/backend_*.sock
sleep 2

echo ""
echo "=========================================="
echo "All Tests Complete!"
echo "=========================================="
echo "Results saved to log/ directory"
