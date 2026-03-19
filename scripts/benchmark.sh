#!/bin/bash
set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

GATEWAY_ADDR="127.0.0.1:8080"
BACKEND_ADDR="127.0.0.1:50051"
RESULTS_DIR="./results/benchmark_$(date +%Y%m%d_%H%M%S)"
DELAY_US=1000

mkdir -p $RESULTS_DIR

echo -e "${GREEN}=== gRPC Gateway Benchmark Suite ===${NC}"
echo -e "${BLUE}Results directory: $RESULTS_DIR${NC}"

cleanup() {
    echo -e "${YELLOW}Cleaning up...${NC}"
    if [ ! -z "$BACKEND_PID" ]; then
        kill $BACKEND_PID 2>/dev/null || true
    fi
    if [ ! -z "$GATEWAY_PID" ]; then
        kill $GATEWAY_PID 2>/dev/null || true
    fi
    echo -e "${GREEN}Cleanup complete${NC}"
}

trap cleanup EXIT

start_services() {
    echo -e "${YELLOW}Starting backend service on $BACKEND_ADDR (delay: ${DELAY_US}us)${NC}"
    ./target/debug/backend --addr $BACKEND_ADDR --delay-us $DELAY_US > $RESULTS_DIR/backend.log 2>&1 &
    BACKEND_PID=$!
    sleep 2
    
    echo -e "${YELLOW}Starting gateway service on $GATEWAY_ADDR${NC}"
    ./target/debug/gateway --listen-addr $GATEWAY_ADDR --backend-addr $BACKEND_ADDR --transport tcp > $RESULTS_DIR/gateway.log 2>&1 &
    GATEWAY_PID=$!
    sleep 2
    
    if curl -s http://$GATEWAY_ADDR/health > /dev/null; then
        echo -e "${GREEN}Services started successfully${NC}"
    else
        echo -e "${RED}Failed to start services${NC}"
        exit 1
    fi
}

stop_services() {
    echo -e "${YELLOW}Stopping services...${NC}"
    if [ ! -z "$GATEWAY_PID" ]; then
        kill $GATEWAY_PID 2>/dev/null || true
    fi
    if [ ! -z "$BACKEND_PID" ]; then
        kill $BACKEND_PID 2>/dev/null || true
    fi
    sleep 1
}

run_wrk() {
    echo -e "\n${GREEN}=== wrk Benchmark ===${NC}"
    
    if ! command -v wrk &> /dev/null; then
        echo -e "${RED}wrk not found. Skipping...${NC}"
        echo "Install: https://github.com/wg/wrk"
        return
    fi
    
    cat > /tmp/wrk_script.lua << 'EOF'
wrk.method = "POST"
wrk.body   = '{"message": "benchmark", "payload": "test"}'
wrk.headers["Content-Type"] = "application/json"
EOF
    
    local CONNECTIONS=(64 256 512 1024)
    local THREADS=8
    local DURATION=30
    
    for conn in "${CONNECTIONS[@]}"; do
        local output="$RESULTS_DIR/wrk_c${conn}.txt"
        echo -e "${YELLOW}Running wrk: $conn connections, $THREADS threads, ${DURATION}s${NC}"
        
        wrk -t$THREADS -c$conn -d${DURATION}s -s/tmp/wrk_script.lua --latency \
            http://$GATEWAY_ADDR > "$output" 2>&1
        
        echo -e "${GREEN}Results saved to: $output${NC}"
        grep "Requests/sec" $output || true
        sleep 2
    done
}

run_ab() {
    echo -e "\n${GREEN}=== Apache Benchmark (ab) ===${NC}"
    
    if ! command -v ab &> /dev/null; then
        echo -e "${RED}ab not found. Skipping...${NC}"
        echo "Install: sudo apt-get install apache2-utils"
        return
    fi
    
    local CONNECTIONS=(64 256 512 1024)
    local REQUESTS=30000
    
    for conn in "${CONNECTIONS[@]}"; do
        local output="$RESULTS_DIR/ab_c${conn}.txt"
        echo -e "${YELLOW}Running ab: $conn connections, $REQUESTS requests${NC}"
        
        ab -n $REQUESTS -c $conn -p /tmp/ab_post_data.txt -T "application/json" \
            http://$GATEWAY_ADDR/echo > "$output" 2>&1
        
        echo -e "${GREEN}Results saved to: $output${NC}"
        grep "Requests per second" $output || true
        grep "Time per request" $output || true
        sleep 2
    done
}

run_hey() {
    echo -e "\n${GREEN}=== hey Benchmark ===${NC}"
    
    if ! command -v hey &> /dev/null; then
        echo -e "${RED}hey not found. Skipping...${NC}"
        echo "Install: go install github.com/rakyll/hey@latest"
        return
    fi
    
    local CONNECTIONS=(64 256 512 1024)
    local DURATION=30s
    local QPS=10000
    
    for conn in "${CONNECTIONS[@]}"; do
        local output="$RESULTS_DIR/hey_c${conn}.txt"
        echo -e "${YELLOW}Running hey: $conn connections, ${DURATION}, QPS limit: $QPS${NC}"
        
        hey -z $DURATION -c $conn -q $((QPS / conn)) -m POST \
            -d '{"message": "benchmark", "payload": "test"}' \
            -H "Content-Type: application/json" \
            http://$GATEWAY_ADDR/echo > "$output" 2>&1
        
        echo -e "${GREEN}Results saved to: $output${NC}"
        grep "Requests/sec" $output || true
        sleep 2
    done
}

run_ghz() {
    echo -e "\n${GREEN}=== ghz Benchmark (gRPC) ===${NC}"
    
    if ! command -v ghz &> /dev/null; then
        echo -e "${RED}ghz not found. Skipping...${NC}"
        echo "Install: https://github.com/bojand/ghz#install"
        return
    fi
    
    local CONNECTIONS=(64 256 512 1024)
    local CALLS=30000
    
    for conn in "${CONNECTIONS[@]}"; do
        local output="$RESULTS_DIR/ghz_c${conn}.txt"
        echo -e "${YELLOW}Running ghz: $conn connections, $CALLS calls${NC}"
        
        ghz --insecure \
            --proto proto/src/echo.proto \
            --call echo.EchoService.Echo \
            -c $conn \
            -n $CALLS \
            -d '{"message": "benchmark", "timestamp_ns": 0, "payload": "dGVzdA=="}' \
            $BACKEND_ADDR > "$output" 2>&1
        
        echo -e "${GREEN}Results saved to: $output${NC}"
        grep "Requests/sec" $output || grep "rps" $output || true
        sleep 2
    done
}

run_custom_benchmark() {
    echo -e "\n${GREEN}=== Custom Benchmark Tool ===${NC}"
    
    local CONNECTIONS=(64 256 512 1024)
    local DURATION=30
    
    for conn in "${CONNECTIONS[@]}"; do
        local output="$RESULTS_DIR/custom_c${conn}.txt"
        local qps=$((conn * 50))
        echo -e "${YELLOW}Running custom: $conn connections, ${DURATION}s, QPS target: $qps${NC}"
        
        ./target/debug/benchmark http \
            --target http://$GATEWAY_ADDR/health \
            --connections $conn \
            --duration-secs $DURATION \
            --qps $qps > "$output" 2>&1
        
        echo -e "${GREEN}Results saved to: $output${NC}"
        cat "$output"
        sleep 2
    done
}

generate_report() {
    echo -e "\n${GREEN}=== Generating Summary Report ===${NC}"
    
    local report="$RESULTS_DIR/summary_report.txt"
    
    echo "=== Benchmark Summary Report ===" > $report
    echo "Date: $(date)" >> $report
    echo "Backend Delay: ${DELAY_US}us" >> $report
    echo "" >> $report
    
    echo "=== wrk Results ===" >> $report
    for f in "$RESULTS_DIR"/wrk_*.txt; do
        [ -f "$f" ] || continue
        echo "--- $(basename $f) ---" >> $report
        grep -E "(Requests/sec|Latency|50%|90%|99%)" "$f" >> $report 2>/dev/null || true
        echo "" >> $report
    done
    
    echo "=== ab Results ===" >> $report
    for f in "$RESULTS_DIR"/ab_*.txt; do
        [ -f "$f" ] || continue
        echo "--- $(basename $f) ---" >> $report
        grep -E "(Requests per second|Time per request|50%|90%|99%)" "$f" >> $report 2>/dev/null || true
        echo "" >> $report
    done
    
    echo "=== hey Results ===" >> $report
    for f in "$RESULTS_DIR"/hey_*.txt; do
        [ -f "$f" ] || continue
        echo "--- $(basename $f) ---" >> $report
        grep -E "(Requests/sec|Latency|50%|90%|99%)" "$f" >> $report 2>/dev/null || true
        echo "" >> $report
    done
    
    echo "=== ghz Results ===" >> $report
    for f in "$RESULTS_DIR"/ghz_*.txt; do
        [ -f "$f" ] || continue
        echo "--- $(basename $f) ---" >> $report
        grep -E "(Requests|latency|50%|90%|99%)" "$f" >> $report 2>/dev/null || true
        echo "" >> $report
    done
    
    echo -e "${GREEN}Summary report saved to: $report${NC}"
    cat $report
}

prepare_test_data() {
    echo -e "${YELLOW}Preparing test data...${NC}"
    echo -n '{"message": "benchmark", "payload": "test"}' > /tmp/ab_post_data.txt
}

main() {
    local tools=${1:-"all"}
    
    prepare_test_data
    start_services
    
    case "$tools" in
        "wrk")
            run_wrk
            ;;
        "ab")
            run_ab
            ;;
        "hey")
            run_hey
            ;;
        "ghz")
            run_ghz
            ;;
        "custom")
            run_custom_benchmark
            ;;
        "all")
            run_wrk
            run_ab
            run_hey
            run_ghz
            run_custom_benchmark
            ;;
        *)
            echo -e "${RED}Unknown tool: $tools${NC}"
            echo "Usage: $0 [wrk|ab|hey|ghz|custom|all]"
            exit 1
            ;;
    esac
    
    generate_report
    
    echo -e "\n${GREEN}=== Benchmark Complete ===${NC}"
    echo -e "${BLUE}All results saved to: $RESULTS_DIR${NC}"
}

main "$@"
