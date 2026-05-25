#!/bin/bash

# 比较三种 SHM 传输方式在不同矩阵大小和并发度下的矩阵乘法性能
# 目标：找出最有利于 uintr 的场景

set -e

GATEWAY_PORT=8080
TIMESTAMP=$(date +"%Y%m%d-%H%M%S")
LOG_DIR="log/matrix_latency_${TIMESTAMP}"
TEST_DURATION=60
BPFTRACE_ENABLE=${BPFTRACE_ENABLE:-0}
BPFTRACE_DURATION=$((TEST_DURATION + 20))

MATRIX_SIZES=(2)
CONCURRENCIES=(8)

mkdir -p $LOG_DIR

if [ "$BPFTRACE_ENABLE" = "1" ]; then
    BPFTRACE_LOG="${LOG_DIR}/bpftrace.log"
    echo "Starting bpftrace (${BPFTRACE_DURATION}s)..."
    sudo bpftrace -e "
        uprobe:target/release/backend:rust_interrupt_callback { @irq_backend++; }
        uprobe:target/release/backend:*uintr_wait* { @wait_backend++; }
        uprobe:target/release/backend:*process_global_uintr_wakers* { @wakers_backend++; }
        uprobe:target/release/gateway:rust_interrupt_callback { @irq_gateway++; }
        uprobe:target/release/gateway:*uintr_wait* { @wait_gateway++; }
        uprobe:target/release/gateway:*process_global_uintr_wakers* { @wakers_gateway++; }
        uprobe:target/release/gateway:*senduipi* { @send_uipi_gateway++; }
        uprobe:target/release/backend:*senduipi* { @send_uipi_backend++; }
        uprobe:target/release/gateway:*ui_handler* { @ui_handler_gateway++; }
        uprobe:target/release/backend:*ui_handler* { @ui_handler_backend++; }

        interval:s:${BPFTRACE_DURATION} {
            printf(\"=== %ds report ===\n\", ${BPFTRACE_DURATION});
            printf(\"--- Backend ---\n\");
            printf(\"intr_callback: %d\n\", @irq_backend);
            printf(\"ui_handler: %d\n\", @ui_handler_backend);
            printf(\"uintr_wait:     %d\n\", @wait_backend);
            printf(\"wakers:         %d\n\", @wakers_backend);
            printf(\"send_uipi: %d\n\", @send_uipi_backend);
            printf(\"--- Gateway ---\n\");
            printf(\"intr_callback: %d\n\", @irq_gateway);
            printf(\"ui_handler: %d\n\", @ui_handler_gateway);
            printf(\"uintr_wait:     %d\n\", @wait_gateway);
            printf(\"wakers:         %d\n\", @wakers_gateway);
            printf(\"send_uipi: %d\n\", @send_uipi_gateway);
            exit();
        }
    " > "${BPFTRACE_LOG}" 2>&1 &
    BPFTRACE_PID=$!
    echo "bpftrace PID: ${BPFTRACE_PID}"
    sleep 3
fi

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
    local shm_name="bench_${transport_name}_m${matrix_size}_c${concurrency}${run_label:+_}$run_label"

    echo ""
    echo "=========================================="
    echo "Testing ${transport_name} | Matrix: ${matrix_size}x${matrix_size} | Concurrency: ${concurrency}${run_label:+ ($run_label)}"
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

    local file_suffix="${run_label:+_}${run_label}"
    local gateway_log="${LOG_DIR}/${transport_name}_m${matrix_size}_c${concurrency}${file_suffix}_gateway.log"
    
    echo "Starting gateway (logging to ${gateway_log})..."
    # 设置日志级别为 warn，确保 SHM 延迟日志能输出
    # 使用子shell确保环境变量正确传递
    (RUST_LOG=warn,gateway=warn ./target/release/gateway \
        --transport ${gateway_transport} \
        --shm-name ${shm_name} \
        --listen-addr 127.0.0.1:${GATEWAY_PORT}) > ${gateway_log} 2>&1 &
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
echo "All Matrix Multiplication Tests Complete!"
echo "=========================================="
echo ""
echo "Results saved to: ${LOG_DIR}/"

if [ "$BPFTRACE_ENABLE" = "1" ] && [ -n "${BPFTRACE_PID:-}" ]; then
    wait $BPFTRACE_PID 2>/dev/null || true
    echo ""
    echo "bpftrace results:"
    cat "${BPFTRACE_LOG}"
fi
