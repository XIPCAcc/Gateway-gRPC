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

echo -e "${GREEN}=== Apache Benchmark (ab) ===${NC}"

if ! command -v ab &> /dev/null; then
    echo -e "${RED}ab not found. Please install:${NC}"
    echo "  Ubuntu: sudo apt-get install apache2-utils"
    exit 1
fi

RESULTS_DIR="./results/ab_$(date +%Y%m%d_%H%M%S)"
mkdir -p $RESULTS_DIR

echo -n '{"message": "benchmark", "payload": "test"}' > /tmp/ab_post_data.txt

CONNECTIONS=${1:-"64 256 512 1024"}
REQUESTS=${2:-30000}

echo "Connections: $CONNECTIONS"
echo "Total Requests: $REQUESTS"
echo ""

for conn in $CONNECTIONS; do
    output="$RESULTS_DIR/ab_c${conn}.txt"
    echo -e "${YELLOW}Running ab: $conn concurrent connections${NC}"
    
    ab -n $REQUESTS -c $conn -p /tmp/ab_post_data.txt -T "application/json" \
        http://$GATEWAY_ADDR/echo > "$output" 2>&1
    
    echo -e "${GREEN}Results:${NC}"
    grep "Requests per second" $output || true
    grep "Time per request:" $output | head -1 || true
    grep -E "^\s+(50%|90%|95%|99%)" $output || true
    echo ""
    
    sleep 2
done

echo -e "${GREEN}All results saved to: $RESULTS_DIR${NC}"
