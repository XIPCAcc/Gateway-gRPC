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

echo -e "${GREEN}=== wrk Benchmark ===${NC}"

if ! command -v wrk &> /dev/null; then
    echo -e "${RED}wrk not found. Please install:${NC}"
    echo "  Ubuntu: sudo apt-get install wrk"
    echo "  Or build from: https://github.com/wg/wrk"
    exit 1
fi

RESULTS_DIR="./results/wrk_$(date +%Y%m%d_%H%M%S)"
mkdir -p $RESULTS_DIR

cat > /tmp/wrk_script.lua << 'EOF'
wrk.method = "POST"
wrk.body   = '{"message": "benchmark", "payload": "test"}'
wrk.headers["Content-Type"] = "application/json"
EOF

CONNECTIONS=${1:-"64 256 512 1024"}
THREADS=${2:-8}
DURATION=${3:-30}

echo "Connections: $CONNECTIONS"
echo "Threads: $THREADS"
echo "Duration: ${DURATION}s"
echo ""

for conn in $CONNECTIONS; do
    output="$RESULTS_DIR/wrk_c${conn}.txt"
    echo -e "${YELLOW}Running wrk: $conn connections${NC}"
    
    wrk -t$THREADS -c$conn -d${DURATION}s -s/tmp/wrk_script.lua --latency \
        http://$GATEWAY_ADDR/echo > "$output" 2>&1
    
    echo -e "${GREEN}Results:${NC}"
    grep "Requests/sec" $output || true
    grep -E "^\s+(50%|90%|99%)" $output || true
    echo ""
    
    sleep 2
done

echo -e "${GREEN}All results saved to: $RESULTS_DIR${NC}"
