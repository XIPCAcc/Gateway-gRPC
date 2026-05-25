#!/bin/bash

# 比较三种 SHM 传输方式的 QPS（纯吞吐量，不输出 SHM 延迟日志）
# 日志输出对 QPS 影响很大，因此关闭 gateway 日志，只关注吞吐量

set -e

GATEWAY_PORT=8080
TIMESTAMP=$(date +"%Y%m%d-%H%M%S")
LOG_DIR="log/matrix_qps_${TIMESTAMP}"
TEST_DURATION=60

MATRIX_SIZES=(2 4 8 16 32 64 128)
CONCURRENCIES=(8 8 8 16 16 16 32 32 32 64 64 64 128 128 128 256 256 256)

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
    local run_label=${6:-}
    local shm_name="qps_${transport_name}_m${matrix_size}_c${concurrency}${run_label:+_}$run_label"

    echo ""
    echo "=========================================="
    echo "${transport_name} | Matrix: ${matrix_size}x${matrix_size} | Concurrency: ${concurrency}${run_label:+ ($run_label)}"
    echo "=========================================="

    cleanup_shm "$shm_name"

    ./target/release/backend \
        --transport ${backend_transport} \
        --shm-name ${shm_name} \
        --delay-us 0 &
    BACKEND_PID=$!

    sleep 2

    if ! kill -0 $BACKEND_PID 2>/dev/null; then
        echo "ERROR: Backend failed to start!"
        return 1
    fi

    echo "Starting gateway (no latency logging)..."
    ./target/release/gateway \
        --transport ${gateway_transport} \
        --shm-name ${shm_name} \
        --listen-addr 127.0.0.1:${GATEWAY_PORT} &
    GATEWAY_PID=$!

    sleep 3

    if ! kill -0 $GATEWAY_PID 2>/dev/null; then
        echo "ERROR: Gateway failed to start!"
        kill $BACKEND_PID 2>/dev/null || true
        return 1
    fi

    local file_suffix="${run_label:+_}${run_label}"
    local log_file="${LOG_DIR}/${transport_name}_m${matrix_size}_c${concurrency}${file_suffix}.log"

    echo "Running oha (matrix ${matrix_size}x${matrix_size}, ${concurrency} connections)..."
    oha -z ${TEST_DURATION}s -c ${concurrency} -m POST \
        -T 'application/json' \
        -d "{\"matrix_size\":${matrix_size}}" \
        --no-tui \
        "http://127.0.0.1:${GATEWAY_PORT}/matrix" 2>&1 | tee ${log_file}

    kill $GATEWAY_PID $BACKEND_PID 2>/dev/null || true
    wait $GATEWAY_PID $BACKEND_PID 2>/dev/null || true
    sleep 2
    cleanup_shm "$shm_name"
    sleep 1
}

declare -A run_count

for matrix_size in "${MATRIX_SIZES[@]}"; do
    for concurrency in "${CONCURRENCIES[@]}"; do
        if [ $matrix_size -ge 256 ] && [ $concurrency -ge 512 ]; then
            echo "Skipping matrix=${matrix_size} concurrency=${concurrency} (too heavy)"
            continue
        fi

        run_count_key="${matrix_size}_${concurrency}"
        run_count[$run_count_key]=$(( ${run_count[$run_count_key]:-0} + 1 ))
        run_label="r${run_count[$run_count_key]}"

        run_matrix_test "shm-uintr"   "shm-uintr"   "shm-uintr"   $matrix_size $concurrency $run_label || true
        run_matrix_test "shm-eventfd" "shm-eventfd" "shm-eventfd" $matrix_size $concurrency $run_label || true
        run_matrix_test "shm-uds"     "shm-uds"     "shm-uds"     $matrix_size $concurrency $run_label || true
        
    done
done

echo ""
echo "=========================================="
echo "All QPS Tests Complete!"
echo "=========================================="
echo ""
echo "Results saved to: ${LOG_DIR}/"

echo ""
echo "=========================================="
echo "Generating Summary Report..."
echo "=========================================="

SUMMARY_FILE="${LOG_DIR}/summary.txt"

echo "QPS Performance Comparison (no latency logging)" > ${SUMMARY_FILE}
echo "================================================" >> ${SUMMARY_FILE}
echo "Date: $(date)" >> ${SUMMARY_FILE}
echo "" >> ${SUMMARY_FILE}
echo "Matrix Sizes: ${MATRIX_SIZES[*]}" >> ${SUMMARY_FILE}
echo "Concurrencies: ${CONCURRENCIES[*]}" >> ${SUMMARY_FILE}
echo "Test Duration: ${TEST_DURATION}s each" >> ${SUMMARY_FILE}
echo "" >> ${SUMMARY_FILE}

printf "%-12s %-8s %-12s %-6s %-14s %-14s\n" \
    "Transport" "Matrix" "Concurrency" "Run" "QPS(req/s)" "AvgLat" >> ${SUMMARY_FILE}
printf "%-12s %-8s %-12s %-6s %-14s %-14s\n" \
    "---------" "------" "----------" "----" "----------" "----------" >> ${SUMMARY_FILE}

UNIQUE_CONCURRENCIES=($(echo "${CONCURRENCIES[@]}" | tr ' ' '\n' | sort -nu))

for transport in "shm-uds" "shm-eventfd" "shm-uintr"; do
    for matrix_size in "${MATRIX_SIZES[@]}"; do
        for concurrency in "${UNIQUE_CONCURRENCIES[@]}"; do
            for fname in "${LOG_DIR}/${transport}_m${matrix_size}_c${concurrency}"*.log; do
                [ -f "$fname" ] || continue
                log_file="$fname"
                run_label=$(echo "$log_file" | sed -n 's/.*_r\([0-9]*\)\.log/\1/p')
                run_label=${run_label:-1}
                qps=$(grep "Requests/sec:" "$log_file" | awk '{print $2}')
                avg_lat=$(grep "Average:" "$log_file" | head -1 | awk '{print $2, $3}')
                qps=${qps:-"N/A"}
                avg_lat=${avg_lat:-"N/A"}
                printf "%-12s %-8s %-12s %-6s %-14s %-14s\n" \
                    "$transport" "${matrix_size}x${matrix_size}" "$concurrency" "$run_label" \
                    "$qps" "$avg_lat" >> ${SUMMARY_FILE}
            done
        done
    done
    echo "" >> ${SUMMARY_FILE}
done

echo "Summary saved to: ${SUMMARY_FILE}"
cat ${SUMMARY_FILE}