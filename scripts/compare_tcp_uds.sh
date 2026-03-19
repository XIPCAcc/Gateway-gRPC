#!/bin/bash
set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

GATEWAY_ADDR="127.0.0.1:8080"
BACKEND_ADDR="127.0.0.1:50051"
UDS_PATH="/tmp/backend.sock"
DELAY_US=${1:-1000}
DURATION=${2:-30}
RESULTS_DIR="./benchmark_results"

echo -e "${GREEN}=== TCP vs UDS Performance Comparison ===${NC}"
echo -e "${YELLOW}Configuration:${NC}"
echo -e "  Backend delay: ${DELAY_US}μs"
echo -e "  Test duration: ${DURATION}s"
echo -e "  Concurrency levels: 64, 256, 1024"
echo ""

# Create results directory
mkdir -p "$RESULTS_DIR"

# Function to stop services
stop_services() {
    echo -e "${YELLOW}Stopping services...${NC}"
    pkill -f "target/debug/backend" 2>/dev/null || true
    pkill -f "target/debug/gateway" 2>/dev/null || true
    sleep 2
}

# Function to start TCP services
start_tcp_services() {
    echo -e "${YELLOW}Starting TCP services...${NC}"
    
    # Start backend
    ./target/debug/backend --addr $BACKEND_ADDR --delay-us $DELAY_US > /tmp/backend_tcp.log 2>&1 &
    BACKEND_PID=$!
    echo "Backend PID: $BACKEND_PID"
    
    sleep 2
    
    # Start gateway
    ./target/debug/gateway --listen-addr $GATEWAY_ADDR --backend-addr $BACKEND_ADDR --transport tcp > /tmp/gateway_tcp.log 2>&1 &
    GATEWAY_PID=$!
    echo "Gateway PID: $GATEWAY_PID"
    
    sleep 2
    
    # Check if services are running
    if curl -s http://$GATEWAY_ADDR/health > /dev/null; then
        echo -e "${GREEN}✓ TCP services started successfully${NC}"
    else
        echo -e "${RED}✗ Failed to start TCP services${NC}"
        cat /tmp/backend_tcp.log
        cat /tmp/gateway_tcp.log
        exit 1
    fi
}

# Function to start UDS services
start_uds_services() {
    echo -e "${YELLOW}Starting UDS services...${NC}"
    
    # Clean up existing socket file
    if [ -S "$UDS_PATH" ]; then
        rm -f "$UDS_PATH"
    fi
    
    # Start backend
    ./target/debug/backend --transport uds --uds-path "$UDS_PATH" --delay-us $DELAY_US > /tmp/backend_uds.log 2>&1 &
    BACKEND_PID=$!
    echo "Backend PID: $BACKEND_PID"
    
    sleep 2
    
    # Start gateway
    ./target/debug/gateway --listen-addr $GATEWAY_ADDR --backend-addr "$UDS_PATH" --transport uds --uds-path "$UDS_PATH" > /tmp/gateway_uds.log 2>&1 &
    GATEWAY_PID=$!
    echo "Gateway PID: $GATEWAY_PID"
    
    sleep 2
    
    # Check if services are running
    if curl -s http://$GATEWAY_ADDR/health > /dev/null; then
        echo -e "${GREEN}✓ UDS services started successfully${NC}"
    else
        echo -e "${RED}✗ Failed to start UDS services${NC}"
        cat /tmp/backend_uds.log
        cat /tmp/gateway_uds.log
        exit 1
    fi
}

# Function to run wrk benchmark
run_wrk_benchmark() {
    local transport=$1
    local concurrency=$2
    local output_file="$RESULTS_DIR/${transport}_c${concurrency}.txt"
    
    echo -e "${BLUE}Running wrk: ${transport}, concurrency=${concurrency}, duration=${DURATION}s${NC}"
    
    wrk -t8 -c${concurrency} -d${DURATION}s \
        -s scripts/wrk.lua \
        http://$GATEWAY_ADDR/echo > "$output_file" 2>&1
    
    # Extract key metrics
    local requests=$(grep "Requests/sec" "$output_file" | awk '{print $2}')
    local latency_avg=$(grep "Latency" "$output_file" | awk '{print $2}')
    local latency_stdev=$(grep "Latency" "$output_file" | awk '{print $3}')
    
    echo -e "${GREEN}  QPS: $requests${NC}"
    echo -e "${GREEN}  Latency (avg/stdev): ${latency_avg}/${latency_stdev}${NC}"
    echo ""
}

# Function to create summary
create_summary() {
    local summary_file="$RESULTS_DIR/summary.txt"
    
    echo -e "${GREEN}=== Performance Summary ===${NC}" | tee "$summary_file"
    echo "" | tee -a "$summary_file"
    echo -e "${BLUE}Configuration:${NC}" | tee -a "$summary_file"
    echo "  Backend delay: ${DELAY_US}μs" | tee -a "$summary_file"
    echo "  Test duration: ${DURATION}s" | tee -a "$summary_file"
    echo "" | tee -a "$summary_file"
    
    for concurrency in 64 256 1024; do
        echo -e "${BLUE}Concurrency: ${concurrency}${NC}" | tee -a "$summary_file"
        
        # Extract TCP metrics
        local tcp_file="$RESULTS_DIR/tcp_c${concurrency}.txt"
        local tcp_qps=$(grep "Requests/sec" "$tcp_file" | awk '{print $2}')
        local tcp_avg=$(grep "Latency" "$tcp_file" | awk '{print $2}' | sed 's/ms//')
        
        # Extract UDS metrics
        local uds_file="$RESULTS_DIR/uds_c${concurrency}.txt"
        local uds_qps=$(grep "Requests/sec" "$uds_file" | awk '{print $2}')
        local uds_avg=$(grep "Latency" "$uds_file" | awk '{print $2}' | sed 's/ms//')
        
        # Calculate improvement
        local qps_improvement=$(echo "scale=2; ($uds_qps - $tcp_qps) / $tcp_qps * 100" | bc)
        local avg_improvement=$(echo "scale=2; ($tcp_avg - $uds_avg) / $tcp_avg * 100" | bc)
        
        echo -e "  TCP:" | tee -a "$summary_file"
        echo "    QPS: $tcp_qps" | tee -a "$summary_file"
        echo "    Avg Latency: ${tcp_avg}ms" | tee -a "$summary_file"
        echo -e "  UDS:" | tee -a "$summary_file"
        echo "    QPS: $uds_qps" | tee -a "$summary_file"
        echo "    Avg Latency: ${uds_avg}ms" | tee -a "$summary_file"
        echo -e "  Improvement (UDS vs TCP):" | tee -a "$summary_file"
        echo "    QPS: ${qps_improvement}%" | tee -a "$summary_file"
        echo "    Avg Latency: ${avg_improvement}%" | tee -a "$summary_file"
        echo "" | tee -a "$summary_file"
    done
    
    echo -e "${GREEN}Detailed results saved to: $RESULTS_DIR${NC}"
    echo -e "${GREEN}Summary saved to: $summary_file${NC}"
}

# Check if binaries exist
if [ ! -f ./target/debug/backend ]; then
    echo -e "${RED}Backend binary not found. Building...${NC}"
    cargo build --bin backend
fi

if [ ! -f ./target/debug/gateway ]; then
    echo -e "${RED}Gateway binary not found. Building...${NC}"
    cargo build --bin gateway
fi

# Check if wrk is installed
if ! command -v wrk &> /dev/null; then
    echo -e "${RED}wrk is not installed. Please install wrk first.${NC}"
    exit 1
fi

# Check if bc is installed (for calculations)
if ! command -v bc &> /dev/null; then
    echo -e "${RED}bc is not installed. Please install bc first.${NC}"
    exit 1
fi

# Test TCP
echo -e "${GREEN}=== Testing TCP ===${NC}"
stop_services
start_tcp_services

for concurrency in 64 256 1024; do
    run_wrk_benchmark "tcp" $concurrency
done

stop_services

# Test UDS
echo -e "${GREEN}=== Testing UDS ===${NC}"
start_uds_services

for concurrency in 64 256 1024; do
    run_wrk_benchmark "uds" $concurrency
done

stop_services

# Create summary
create_summary

echo -e "${GREEN}=== Performance comparison completed ===${NC}"