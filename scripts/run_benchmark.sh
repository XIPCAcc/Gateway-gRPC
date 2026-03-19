#!/bin/bash
set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}=== gRPC Gateway Benchmark Suite ===${NC}"

# Configuration
BACKEND_ADDR="127.0.0.1:50051"
GATEWAY_ADDR="127.0.0.1:8080"
UDS_PATH="/tmp/gateway.sock"
SHM_NAME="gateway_shm"
RESULTS_DIR="./results"

# Create results directory
mkdir -p $RESULTS_DIR

# Build the project
echo -e "${YELLOW}Building project...${NC}"
cargo build --release

# Function to cleanup processes
cleanup() {
    echo -e "${YELLOW}Cleaning up...${NC}"
    pkill -f "backend" || true
    pkill -f "gateway" || true
    sleep 1
}
trap cleanup EXIT

# Function to start backend
start_backend() {
    local delay_us=$1
    echo -e "${YELLOW}Starting backend with ${delay_us}us delay...${NC}"
    ./target/release/backend --addr $BACKEND_ADDR --delay-us $delay_us &
    sleep 2
}

# Function to start gateway
start_gateway() {
    local transport=$1
    echo -e "${YELLOW}Starting gateway with $transport transport...${NC}"
    
    case $transport in
        tcp)
            ./target/release/gateway --listen-addr $GATEWAY_ADDR --backend-addr $BACKEND_ADDR --transport tcp &
            ;;
        uds)
            ./target/release/gateway --listen-addr $GATEWAY_ADDR --uds-path $UDS_PATH --transport uds &
            ;;
        shm)
            ./target/release/gateway --listen-addr $GATEWAY_ADDR --shm-name $SHM_NAME --transport shm &
            ;;
    esac
    
    sleep 2
}

# Function to run benchmark
run_benchmark() {
    local transport=$1
    local connections=$2
    local qps=$3
    local duration=$4
    local output_file=$5
    
    echo -e "${YELLOW}Running benchmark: transport=$transport, connections=$connections, qps=$qps${NC}"
    
    # Collect system metrics in background
    local backend_pid=$(pgrep -f "backend" | head -1)
    local gateway_pid=$(pgrep -f "gateway" | head -1)
    
    if [ -n "$backend_pid" ]; then
        perf stat -p $backend_pid -e syscalls:sys_enter,context-switches,cpu-clock sleep $duration 2> "$RESULTS_DIR/backend_perf_${transport}_${connections}.log" &
    fi
    
    if [ -n "$gateway_pid" ]; then
        perf stat -p $gateway_pid -e syscalls:sys_enter,context-switches,cpu-clock sleep $duration 2> "$RESULTS_DIR/gateway_perf_${transport}_${connections}.log" &
    fi
    
    # Run the benchmark
    ./target/release/benchmark http \
        --target "http://$GATEWAY_ADDR" \
        --connections $connections \
        --duration-secs $duration \
        --qps $qps > "$output_file"
    
    wait
}

# Test configurations
CONNECTIONS=(64 256 1024)
QPS_LEVELS=(1000 5000 10000 20000)
TRANSPORTS=("tcp" "uds" "shm")
DURATION=30

echo -e "${GREEN}=== Starting Benchmarks ===${NC}"

for transport in "${TRANSPORTS[@]}"; do
    echo -e "${GREEN}Testing transport: $transport${NC}"
    
    # Start services
    cleanup
    start_backend 1000  # 1ms delay
    start_gateway $transport
    
    # Warmup
    echo -e "${YELLOW}Warming up...${NC}"
    sleep 5
    
    for connections in "${CONNECTIONS[@]}"; do
        for qps in "${QPS_LEVELS[@]}"; do
            output_file="$RESULTS_DIR/result_${transport}_${connections}_${qps}.json"
            
            echo -e "${YELLOW}Benchmark: $transport, C=$connections, QPS=$qps${NC}"
            
            if run_benchmark $transport $connections $qps $DURATION $output_file; then
                echo -e "${GREEN}✓ Completed${NC}"
            else
                echo -e "${RED}✗ Failed${NC}"
            fi
            
            # Cooldown
            sleep 2
        done
    done
done

# Generate report
echo -e "${GREEN}=== Generating Report ===${NC}"

# Combine all results
echo "[" > "$RESULTS_DIR/all_results.json"
first=true
for f in "$RESULTS_DIR"/result_*.json; do
    if [ "$first" = true ]; then
        first=false
    else
        echo "," >> "$RESULTS_DIR/all_results.json"
    fi
    cat "$f" >> "$RESULTS_DIR/all_results.json"
done
echo "]" >> "$RESULTS_DIR/all_results.json"

# Run analysis
./target/release/latency_analyzer \
    --input "$RESULTS_DIR/all_results.json" \
    --output "$RESULTS_DIR/analysis_report.json"

echo -e "${GREEN}=== Benchmark Complete ===${NC}"
echo "Results saved to: $RESULTS_DIR/"
echo ""
echo "Summary:"
cat "$RESULTS_DIR/analysis_report.json" | grep -A 20 "Best Configuration" || true
