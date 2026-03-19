#!/bin/bash
set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

GATEWAY_ADDR="127.0.0.1:8080"

if ! curl -s http://$GATEWAY_ADDR/health > /dev/null 2>&1; then
    echo -e "${RED}Services not running. Please start services first:${NC}"
    echo "  ./scripts/start_services.sh"
    exit 1
fi

echo -e "${GREEN}=== hey Benchmark ===${NC}"

if ! command -v hey &> /dev/null; then
    echo -e "${RED}hey not found. Please install:${NC}"
    echo "  go install github.com/rakyll/hey@latest"
    echo "  Or download from: https://github.com/rakyll/hey/releases"
    exit 1
fi

RESULTS_DIR="./results/hey_$(date +%Y%m%d_%H%M%S)"
mkdir -p $RESULTS_DIR

CONNECTIONS=${1:-"64 256 512 1024"}
DURATION=${2:-30s}
QPS_LIMIT=${3:-10000}

echo "Connections: $CONNECTIONS"
echo "Duration: $DURATION"
echo "QPS Limit: $QPS_LIMIT"
echo ""

for conn in $CONNECTIONS; do
    output="$RESULTS_DIR/hey_c${conn}.txt"
    qps_per_conn=$((QPS_LIMIT / conn))
    echo -e "${YELLOW}Running hey: $conn connections, QPS/conn: $qps_per_conn${NC}"
    
    hey -z $DURATION -c $conn -q $qps_per_conn -m POST \
        -d '{"message": "benchmark", "payload": "test"}' \
        -H "Content-Type: application/json" \
        http://$GATEWAY_ADDR/echo > "$output" 2>&1
    
    echo -e "${GREEN}Results:${NC}"
    grep "Requests/sec" $output || true
    grep "Average" $output || true
    echo ""
    
    sleep 2
done

echo -e "${GREEN}All results saved to: $RESULTS_DIR${NC}"
