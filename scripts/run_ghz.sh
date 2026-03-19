#!/bin/bash
set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

BACKEND_ADDR="127.0.0.1:50051"

if ! pgrep -f "target/debug/backend" > /dev/null; then
    echo -e "${RED}Backend service not running. Please start services first:${NC}"
    echo "  ./scripts/start_services.sh"
    exit 1
fi

echo -e "${GREEN}=== ghz Benchmark (gRPC) ===${NC}"

if ! command -v ghz &> /dev/null; then
    echo -e "${RED}ghz not found. Please install:${NC}"
    echo "  https://github.com/bojand/ghz#install"
    echo "  Or: go install github.com/bojand/ghz/cmd/ghz@latest"
    exit 1
fi

RESULTS_DIR="./results/ghz_$(date +%Y%m%d_%H%M%S)"
mkdir -p $RESULTS_DIR

CONNECTIONS=${1:-"64 256 512 1024"}
CALLS=${2:-30000}

echo "Connections: $CONNECTIONS"
echo "Total Calls: $CALLS"
echo "Target: $BACKEND_ADDR"
echo ""

for conn in $CONNECTIONS; do
    output="$RESULTS_DIR/ghz_c${conn}.txt"
    echo -e "${YELLOW}Running ghz: $conn concurrent connections${NC}"
    
    ghz --insecure \
        --proto proto/src/echo.proto \
        --call echo.EchoService.Echo \
        -c $conn \
        -n $CALLS \
        -d '{"message": "benchmark", "timestamp_ns": 0, "payload": "dGVzdA=="}' \
        $BACKEND_ADDR > "$output" 2>&1
    
    echo -e "${GREEN}Results:${NC}"
    grep -E "(Requests|latency)" $output || grep "rps" $output || true
    echo ""
    
    sleep 2
done

echo -e "${GREEN}All results saved to: $RESULTS_DIR${NC}"
