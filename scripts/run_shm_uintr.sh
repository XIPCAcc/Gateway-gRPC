#!/bin/bash

set -e

DELAY_US=300
SHM_NAME="backend"
GATEWAY_PORT=8080
TIMESTAMP=$(date +"%Y%m%d-%H%M%S")
LOG_DIR="log/${TIMESTAMP}"
TEST_DURATION=30

mkdir -p $LOG_DIR

echo "Recompiling..."
cargo build --release
echo "Compile complete!"

pkill -9 backend 2>/dev/null || true
pkill -9 gateway 2>/dev/null || true
sleep 2

# 修复：统一的共享内存清理函数
cleanup_shm() {
    local name=$1
    # 正确的共享内存路径是 /dev/shm/（而不是 /tmp/shm/）
    rm -f /dev/shm/${name}_req_buf /dev/shm/${name}_resp_buf /dev/shm/${name}_matrix_data 2>/dev/null || true
    rm -f /tmp/${name}_*.sock 2>/dev/null || true
}

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

    # 修复：使用正确的清理路径
    cleanup_shm "$SHM_NAME"

    $backend_cmd &
    BACKEND_PID=$!

    sleep 3

    $gateway_cmd &
    GATEWAY_PID=$!

    sleep 2

    echo "Running wrk with $concurrency connections..."
    wrk -t8 -c$concurrency -d${TEST_DURATION}s --latency \
        -s scripts/wrk.lua \
        "http://127.0.0.1:$GATEWAY_PORT/api/echo" 2>&1 | tee $LOG_DIR/${name}_test_${concurrency}.log

    kill $GATEWAY_PID $BACKEND_PID 2>/dev/null || true
    # 修复：添加 wait 确保进程完全退出
    wait $GATEWAY_PID $BACKEND_PID 2>/dev/null || true
    sleep 2
    # 修复：测试结束后再次清理
    cleanup_shm "$SHM_NAME"
    sleep 1
}

for concurrency in 16 32 64 256 1024 4096; do
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
echo "  $LOG_DIR/shm-uintr_test_*.log"