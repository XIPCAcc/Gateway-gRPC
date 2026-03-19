#!/bin/bash
set -e

# Vegeta-based benchmark script
# Requires vegeta to be installed: https://github.com/tsenart/vegeta

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo -e "${GREEN}=== Vegeta-based Benchmark ===${NC}"

# Check if vegeta is installed
if ! command -v vegeta &> /dev/null; then
    echo -e "${RED}vegeta not found. Please install vegeta first.${NC}"
    echo "Installation: go install github.com/tsenart/vegeta@latest"
    exit 1
fi

GATEWAY_ADDR="127.0.0.1:8080"
RESULTS_DIR="./results/vegeta"
mkdir -p $RESULTS_DIR

# Create vegeta target file
cat > /tmp/vegeta_targets.txt << EOF
POST http://$GATEWAY_ADDR/
Content-Type: application/json
@{"message": "benchmark", "payload": "test"}
EOF

# Function to run vegeta benchmark
run_vegeta() {
    local rate=$1
    local duration=$2
    local connections=$3
    local output_file=$4
    
    echo -e "${YELLOW}Running vegeta: $rate RPS, $connections connections, ${duration}s${NC}"
    
    echo "POST http://$GATEWAY_ADDR/" | vegeta attack \
        -rate=$rate \
        -duration=${duration}s \
        -connections=$connections \
        -workers=$connections \
        -max-workers=$connections \
        | tee "$output_file.bin" | vegeta report
    
    # Generate detailed report
    vegeta report -type=json "$output_file.bin" > "$output_file.json"
    vegeta report -type=hist[0,1ms,2ms,5ms,10ms,20ms,50ms,100ms,200ms,500ms,1s] "$output_file.bin"
    
    echo -e "${GREEN}Results saved to: $output_file.json${NC}"
}

# Test configurations
RATES=(1000 5000 10000 20000)
CONNECTIONS=(64 256 1024)
DURATION=30

echo -e "${YELLOW}Starting vegeta benchmarks...${NC}"

for rate in "${RATES[@]}"; do
    for conn in "${CONNECTIONS[@]}"; do
        output="$RESULTS_DIR/vegeta_r${rate}_c${conn}"
        run_vegeta $rate $DURATION $conn $output
        
        sleep 2
    done
done

# Generate summary
echo -e "${GREEN}=== Vegeta Benchmark Summary ===${NC}" > "$RESULTS_DIR/summary.txt"

for f in "$RESULTS_DIR"/*.json; do
    echo "--- $(basename $f) ---" >> "$RESULTS_DIR/summary.txt"
    cat "$f" | python3 -m json.tool 2>/dev/null || cat "$f" >> "$RESULTS_DIR/summary.txt"
    echo "" >> "$RESULTS_DIR/summary.txt"
done

echo -e "${GREEN}All results saved to: $RESULTS_DIR/${NC}"
