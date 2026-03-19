#!/bin/bash
set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

GATEWAY_ADDR="127.0.0.1:8080"
DELAY_US=${1:-1000}
DURATION=${2:-10}
CONNECTIONS=${3:-64}
THREADS=${4:-4}

RESULTS_DIR="./benchmark_results"
mkdir -p $RESULTS_DIR

echo -e "${GREEN}=== Performance Comparison: TCP vs UDS vs SHM ===${NC}"
echo -e "${YELLOW}Configuration:${NC}"
echo -e "  Backend delay: ${DELAY_US}μs"
echo -e "  Test duration: ${DURATION}s"
echo -e "  Connections: ${CONNECTIONS}"
echo -e "  Threads: ${THREADS}"
echo ""

# Build binaries
echo -e "${BLUE}Building binaries...${NC}"
cargo build --release --bin gateway --bin backend 2>&1 | grep -E "(Compiling|Finished)" || true

# Function to run benchmark for a specific transport
run_benchmark() {
    local transport=$1
    local backend_cmd=$2
    local gateway_cmd=$3
    
    echo -e "${YELLOW}=== Testing $transport ===${NC}"
    
    # Clean up
    pkill -f "target/release/backend" 2>/dev/null || true
    pkill -f "target/release/gateway" 2>/dev/null || true
    rm -f /tmp/backend.sock 2>/dev/null || true
    rm -f /dev/shm/backend_* 2>/dev/null || true
    sleep 1
    
    # Start backend
    echo "Starting backend..."
    $backend_cmd > $RESULTS_DIR/backend_${transport}.log 2>&1 &
    BACKEND_PID=$!
    sleep 3
    
    # Start gateway
    echo "Starting gateway..."
    $gateway_cmd > $RESULTS_DIR/gateway_${transport}.log 2>&1 &
    GATEWAY_PID=$!
    sleep 3
    
    # Verify services are running
    if ! kill -0 $BACKEND_PID 2>/dev/null; then
        echo -e "${RED}Backend failed to start${NC}"
        cat $RESULTS_DIR/backend_${transport}.log
        return 1
    fi
    
    if ! kill -0 $GATEWAY_PID 2>/dev/null; then
        echo -e "${RED}Gateway failed to start${NC}"
        cat $RESULTS_DIR/gateway_${transport}.log
        return 1
    fi
    
    # Run wrk benchmark
    echo "Running benchmark..."
    wrk -t${THREADS} -c${CONNECTIONS} -d${DURATION}s \
        --latency \
        -s scripts/wrk.lua \
        http://$GATEWAY_ADDR/echo > $RESULTS_DIR/result_${transport}.txt 2>&1
    
    # Extract metrics
    local qps=$(grep "Requests/sec" $RESULTS_DIR/result_${transport}.txt | awk '{print $2}')
    local avg_latency=$(grep -A 1 "Latency Distribution" $RESULTS_DIR/result_${transport}.txt | tail -1 | awk '{print $2}')
    local p99=$(grep "99%" $RESULTS_DIR/result_${transport}.txt | awk '{print $2}')
    
    echo -e "${GREEN}Results for $transport:${NC}"
    echo "  QPS: $qps"
    echo "  Avg Latency: $avg_latency"
    echo "  P99 Latency: $p99"
    echo ""
    
    # Save to summary file
    echo "$transport,$qps,$avg_latency,$p99" >> $RESULTS_DIR/summary.csv
    
    # Stop services
    kill $BACKEND_PID $GATEWAY_PID 2>/dev/null || true
    sleep 1
}

# Test TCP
run_benchmark "tcp" \
    "./target/release/backend --transport tcp --addr 127.0.0.1:50051 --delay-us $DELAY_US" \
    "./target/release/gateway --listen-addr $GATEWAY_ADDR --backend-addr 127.0.0.1:50051 --transport tcp"

sleep 2

# Test UDS
run_benchmark "uds" \
    "./target/release/backend --transport uds --uds-path /tmp/backend.sock --delay-us $DELAY_US" \
    "./target/release/gateway --listen-addr $GATEWAY_ADDR --backend-addr /tmp/backend.sock --transport uds"

sleep 2

# Test SHM
run_benchmark "shm" \
    "./target/release/backend --transport shm --shm-name backend --delay-us $DELAY_US" \
    "./target/release/gateway --listen-addr $GATEWAY_ADDR --backend-addr backend --transport shm --shm-name backend"

# Generate comparison report
echo -e "${GREEN}=== Performance Comparison Summary ===${NC}"
echo ""
echo "Transport | QPS       | Avg Latency | P99 Latency"
echo "----------|-----------|-------------|------------"
cat $RESULTS_DIR/summary.csv | while IFS=',' read transport qps avg p99; do
    printf "%-9s | %-9s | %-11s | %s\n" "$transport" "$qps" "$avg" "$p99"
done

echo ""
echo -e "${YELLOW}Detailed results saved to: $RESULTS_DIR/${NC}"
echo ""

# Calculate performance differences
echo -e "${BLUE}Performance Analysis:${NC}"
TCP_QPS=$(grep "^tcp," $RESULTS_DIR/summary.csv | cut -d',' -f2)
UDS_QPS=$(grep "^uds," $RESULTS_DIR/summary.csv | cut -d',' -f2)
SHM_QPS=$(grep "^shm," $RESULTS_DIR/summary.csv | cut -d',' -f2)

if [ -n "$TCP_QPS" ] && [ -n "$UDS_QPS" ]; then
    UDS_VS_TCP=$(echo "scale=2; ($UDS_QPS - $TCP_QPS) / $TCP_QPS * 100" | bc)
    echo "  UDS vs TCP: ${UDS_VS_TCP}% $(if [ $(echo "$UDS_VS_TCP > 0" | bc) -eq 1 ]; then echo "faster"; else echo "slower"; fi)"
fi

if [ -n "$TCP_QPS" ] && [ -n "$SHM_QPS" ]; then
    SHM_VS_TCP=$(echo "scale=2; ($SHM_QPS - $TCP_QPS) / $TCP_QPS * 100" | bc)
    echo "  SHM vs TCP: ${SHM_VS_TCP}% $(if [ $(echo "$SHM_VS_TCP > 0" | bc) -eq 1 ]; then echo "faster"; else echo "slower"; fi)"
fi

if [ -n "$UDS_QPS" ] && [ -n "$SHM_QPS" ]; then
    SHM_VS_UDS=$(echo "scale=2; ($SHM_QPS - $UDS_QPS) / $UDS_QPS * 100" | bc)
    echo "  SHM vs UDS: ${SHM_VS_UDS}% $(if [ $(echo "$SHM_VS_UDS > 0" | bc) -eq 1 ]; then echo "faster"; else echo "slower"; fi)"
fi

echo ""
echo -e "${GREEN}Benchmark completed!${NC}"
