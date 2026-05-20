#!/bin/bash

# UINTR 矩阵乘法性能测试
# backend + gateway 只启动一次，wrk 跑所有负载组合

set -e

GATEWAY_PORT=8080
SHM_NAME="uintr_bench"
TIMESTAMP=$(date +"%Y%m%d-%H%M%S")
LOG_DIR="log/uintr_matrix_${TIMESTAMP}"
TEST_DURATION=5

MATRIX_SIZES=(2 4 8 16 32 64 128 256)
CONCURRENCIES=(8 16 32)

mkdir -p $LOG_DIR

echo "Recompiling..."
cargo build --release 2>&1 | tail -3
echo "Compile complete!"

cleanup() {
    pkill -9 backend 2>/dev/null || true
    pkill -9 gateway 2>/dev/null || true
    sleep 2
    rm -f /dev/shm/${SHM_NAME}_req_buf /dev/shm/${SHM_NAME}_resp_buf /dev/shm/${SHM_NAME}_matrix_data 2>/dev/null || true
    rm -f /tmp/${SHM_NAME}_*.sock 2>/dev/null || true
}

cleanup

echo ""
echo "Starting backend and gateway..."
echo ""

./target/release/backend \
    --transport shm-uintr \
    --shm-name ${SHM_NAME} \
    --delay-us 0 &
BACKEND_PID=$!

if ! kill -0 $BACKEND_PID 2>/dev/null; then
    echo "ERROR: Backend failed to start!"
    exit 1
fi

GATEWAY_LOG="${LOG_DIR}/gateway.log"
(RUST_LOG=warn,gateway=warn ./target/release/gateway \
    --transport shm-uintr \
    --shm-name ${SHM_NAME} \
    --listen-addr 127.0.0.1:${GATEWAY_PORT}) > ${GATEWAY_LOG} 2>&1 &
GATEWAY_PID=$!
sleep 5

if ! kill -0 $GATEWAY_PID 2>/dev/null; then
    echo "ERROR: Gateway failed to start!"
    kill $BACKEND_PID 2>/dev/null || true
    exit 1
fi

echo "Backend PID: $BACKEND_PID, Gateway PID: $GATEWAY_PID"
echo ""

run_wrk() {
    local matrix_size=$1
    local concurrency=$2

    echo "=========================================="
    echo "UINTR | Matrix: ${matrix_size}x${matrix_size} | Concurrency: ${concurrency}"
    echo "=========================================="

    local log_file="${LOG_DIR}/uintr_m${matrix_size}_c${concurrency}.log"
    MATRIX_SIZE=${matrix_size} wrk -t8 -c${concurrency} -d${TEST_DURATION}s --latency \
        -s scripts/wrk_matrix.lua \
        "http://127.0.0.1:${GATEWAY_PORT}/matrix" 2>&1 | tee ${log_file}

    echo ""
}

for matrix_size in "${MATRIX_SIZES[@]}"; do
    for concurrency in "${CONCURRENCIES[@]}"; do
        if [ $matrix_size -ge 256 ] && [ $concurrency -ge 512 ]; then
            echo "Skipping matrix=${matrix_size} concurrency=${concurrency} (too heavy)"
            continue
        fi
        run_wrk $matrix_size $concurrency
    done
done

echo "All tests complete, stopping backend and gateway..."
kill $GATEWAY_PID $BACKEND_PID 2>/dev/null || true
wait $GATEWAY_PID $BACKEND_PID 2>/dev/null || true
cleanup

echo ""
echo "=========================================="
echo "UINTR Matrix Tests Complete!"
echo "=========================================="
echo ""
echo "Results saved to: ${LOG_DIR}/"

SUMMARY_FILE="${LOG_DIR}/summary.txt"

echo "UINTR Matrix Multiplication Performance" > ${SUMMARY_FILE}
echo "=======================================" >> ${SUMMARY_FILE}
echo "Date: $(date)" >> ${SUMMARY_FILE}
echo "" >> ${SUMMARY_FILE}
echo "Transport: shm-uintr" >> ${SUMMARY_FILE}
echo "Matrix Sizes: ${MATRIX_SIZES[*]}" >> ${SUMMARY_FILE}
echo "Concurrencies: ${CONCURRENCIES[*]}" >> ${SUMMARY_FILE}
echo "Test Duration: ${TEST_DURATION}s each" >> ${SUMMARY_FILE}
echo "" >> ${SUMMARY_FILE}

printf "%-8s %-12s %-14s %-14s %-14s\n" "Matrix" "Concurrency" "QPS(req/s)" "AvgLat(ms)" "P99Lat(ms)" >> ${SUMMARY_FILE}
printf "%-8s %-12s %-14s %-14s %-14s\n" "------" "----------" "----------" "----------" "----------" >> ${SUMMARY_FILE}

for matrix_size in "${MATRIX_SIZES[@]}"; do
    for concurrency in "${CONCURRENCIES[@]}"; do
        if [ $matrix_size -ge 256 ] && [ $concurrency -ge 512 ]; then
            continue
        fi
        log_file="${LOG_DIR}/uintr_m${matrix_size}_c${concurrency}.log"
        if [ -f "$log_file" ]; then
            qps=$(grep "Requests/sec:" "$log_file" | awk '{print $2}')
            avg_lat=$(grep "Latency" "$log_file" | head -1 | awk '{print $2}')
            p99_lat=$(grep "99%" "$log_file" | head -1 | awk '{print $2}')
            qps=${qps:-"N/A"}
            avg_lat=${avg_lat:-"N/A"}
            p99_lat=${p99_lat:-"N/A"}
            printf "%-8s %-12s %-14s %-14s %-14s\n" \
                "${matrix_size}x${matrix_size}" "$concurrency" "$qps" "$avg_lat" "$p99_lat" >> ${SUMMARY_FILE}
        fi
    done
    echo "" >> ${SUMMARY_FILE}
done

echo "Summary saved to: ${SUMMARY_FILE}"
cat ${SUMMARY_FILE}