#!/bin/bash

# 比较三种 SHM 传输方式在不同矩阵大小和并发度下的矩阵乘法性能
# 目标：找出最有利于 uintr 的场景

set -e

GATEWAY_PORT=8080
TIMESTAMP=$(date +"%Y%m%d-%H%M%S")
LOG_DIR="log/matrix_${TIMESTAMP}"
TEST_DURATION=15

MATRIX_SIZES=(2 4 8 16 32 64 128 256 512)
CONCURRENCIES=(16 32 64 128 256 512 1024)

mkdir -p $LOG_DIR

echo "Recompiling..."
cargo build --release 2>&1 | tail -3
echo "Compile complete!"

pkill -9 backend 2>/dev/null || true
pkill -9 gateway 2>/dev/null || true
sleep 2

cleanup_shm() {
    local name=$1
    rm -f /dev/shm/${name}_req_buf /dev/shm/${name}_resp_buf /dev/shm/${name}_matrix_data 2>/dev/null || true
    rm -f /tmp/${name}_*.sock 2>/dev/null || true
}

run_matrix_test() {
    local transport_name=$1
    local backend_transport=$2
    local gateway_transport=$3
    local matrix_size=$4
    local concurrency=$5
    local shm_name="bench_${transport_name}_m${matrix_size}_c${concurrency}"

    echo ""
    echo "=========================================="
    echo "Testing ${transport_name} | Matrix: ${matrix_size}x${matrix_size} | Concurrency: ${concurrency}"
    echo "=========================================="

    cleanup_shm "$shm_name"

    ./target/release/backend \
        --transport ${backend_transport} \
        --shm-name ${shm_name} \
        --delay-us 0 &
    BACKEND_PID=$!
    sleep 3

    if ! kill -0 $BACKEND_PID 2>/dev/null; then
        echo "ERROR: Backend failed to start!"
        return 1
    fi

    ./target/release/gateway \
        --transport ${gateway_transport} \
        --shm-name ${shm_name} \
        --listen-addr 127.0.0.1:${GATEWAY_PORT} &
    GATEWAY_PID=$!
    sleep 2

    if ! kill -0 $GATEWAY_PID 2>/dev/null; then
        echo "ERROR: Gateway failed to start!"
        kill $BACKEND_PID 2>/dev/null || true
        return 1
    fi

    local log_file="${LOG_DIR}/${transport_name}_m${matrix_size}_c${concurrency}.log"

    echo "Running wrk (matrix ${matrix_size}x${matrix_size}, ${concurrency} connections)..."
    MATRIX_SIZE=${matrix_size} wrk -t8 -c${concurrency} -d${TEST_DURATION}s --latency \
        -s scripts/wrk_matrix.lua \
        "http://127.0.0.1:${GATEWAY_PORT}/matrix" 2>&1 | tee ${log_file}

    kill $GATEWAY_PID $BACKEND_PID 2>/dev/null || true
    wait $GATEWAY_PID $BACKEND_PID 2>/dev/null || true
    sleep 2
    cleanup_shm "$shm_name"
    sleep 1
}

for matrix_size in "${MATRIX_SIZES[@]}"; do
    for concurrency in "${CONCURRENCIES[@]}"; do
        if [ $matrix_size -ge 256 ] && [ $concurrency -ge 512 ]; then
            echo "Skipping matrix=${matrix_size} concurrency=${concurrency} (too heavy)"
            continue
        fi

        run_matrix_test "shm-uds"     "shm-uds"     "shm-uds"     $matrix_size $concurrency || true
        run_matrix_test "shm-eventfd" "shm-eventfd" "shm-eventfd" $matrix_size $concurrency || true
        run_matrix_test "shm-uintr"   "shm-uintr"   "shm-uintr"   $matrix_size $concurrency || true
    done
done

echo ""
echo "=========================================="
echo "All Matrix Multiplication Tests Complete!"
echo "=========================================="
echo ""
echo "Results saved to: ${LOG_DIR}/"

echo ""
echo "=========================================="
echo "Generating Summary Report..."
echo "=========================================="

SUMMARY_FILE="${LOG_DIR}/summary.txt"

echo "Matrix Multiplication Performance Comparison" > ${SUMMARY_FILE}
echo "===========================================" >> ${SUMMARY_FILE}
echo "Date: $(date)" >> ${SUMMARY_FILE}
echo "" >> ${SUMMARY_FILE}
echo "Matrix Sizes: ${MATRIX_SIZES[*]}" >> ${SUMMARY_FILE}
echo "Concurrencies: ${CONCURRENCIES[*]}" >> ${SUMMARY_FILE}
echo "Test Duration: ${TEST_DURATION}s each" >> ${SUMMARY_FILE}
echo "" >> ${SUMMARY_FILE}

printf "%-12s %-8s %-12s %-14s %-14s %-14s\n" "Transport" "Matrix" "Concurrency" "QPS(req/s)" "AvgLat(ms)" "P99Lat(ms)" >> ${SUMMARY_FILE}
printf "%-12s %-8s %-12s %-14s %-14s %-14s\n" "---------" "------" "----------" "----------" "----------" "----------" >> ${SUMMARY_FILE}

for transport in "shm-uds" "shm-eventfd" "shm-uintr"; do
    for matrix_size in "${MATRIX_SIZES[@]}"; do
        for concurrency in "${CONCURRENCIES[@]}"; do
            if [ $matrix_size -ge 256 ] && [ $concurrency -ge 512 ]; then
                continue
            fi
            log_file="${LOG_DIR}/${transport}_m${matrix_size}_c${concurrency}.log"
            if [ -f "$log_file" ]; then
                qps=$(grep "Requests/sec:" "$log_file" | awk '{print $2}')
                avg_lat=$(grep "Latency" "$log_file" | head -1 | awk '{print $2}')
                p99_lat=$(grep "99%" "$log_file" | head -1 | awk '{print $2}')
                qps=${qps:-"N/A"}
                avg_lat=${avg_lat:-"N/A"}
                p99_lat=${p99_lat:-"N/A"}
                printf "%-12s %-8s %-12s %-14s %-14s %-14s\n" \
                    "$transport" "${matrix_size}x${matrix_size}" "$concurrency" "$qps" "$avg_lat" "$p99_lat" >> ${SUMMARY_FILE}
            fi
        done
    done
    echo "" >> ${SUMMARY_FILE}
done

echo "Summary saved to: ${SUMMARY_FILE}"
cat ${SUMMARY_FILE}