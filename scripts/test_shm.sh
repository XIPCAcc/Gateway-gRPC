#!/bin/bash
set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

GATEWAY_ADDR="127.0.0.1:8080"
SHM_NAME="backend"
DELAY_US=${1:-1000}

echo -e "${GREEN}=== Testing Shared Memory Communication ===${NC}"
echo -e "${YELLOW}Configuration:${NC}"
echo -e "  Backend delay: ${DELAY_US}μs"
echo -e "  Shared memory name: ${SHM_NAME}"
echo ""

# Check if binaries exist
if [ ! -f ./target/debug/backend ]; then
    echo -e "${RED}Backend binary not found. Building...${NC}"
    cargo build --bin backend
fi

if [ ! -f ./target/debug/gateway ]; then
    echo -e "${RED}Gateway binary not found. Building...${NC}"
    cargo build --bin gateway
fi

# Clean up any existing shared memory
rm -f /dev/shm/${SHM_NAME}_req /dev/shm/${SHM_NAME}_resp 2>/dev/null || true

# Stop any existing services
pkill -f "target/debug/backend" 2>/dev/null || true
pkill -f "target/debug/gateway" 2>/dev/null || true
sleep 1

echo -e "${YELLOW}Starting backend with shared memory...${NC}"
./target/debug/backend --transport shm --shm-name "$SHM_NAME" --delay-us $DELAY_US > /tmp/backend_shm.log 2>&1 &
BACKEND_PID=$!
echo "Backend PID: $BACKEND_PID"

# Wait for backend to create shared memory
sleep 2

echo -e "${YELLOW}Starting gateway with shared memory...${NC}"
./target/debug/gateway --listen-addr $GATEWAY_ADDR --backend-addr "$SHM_NAME" --transport shm --shm-name "$SHM_NAME" > /tmp/gateway_shm.log 2>&1 &
GATEWAY_PID=$!
echo "Gateway PID: $GATEWAY_PID"

# Wait for gateway to start
sleep 2

echo -e "${YELLOW}Testing shared memory communication...${NC}"
echo ""

# Test 1: Health check
echo -e "${BLUE}Test 1: Health check${NC}"
if curl -s http://$GATEWAY_ADDR/health > /dev/null; then
    echo -e "${GREEN}✓ Health check passed${NC}"
else
    echo -e "${RED}✗ Health check failed${NC}"
    echo "Backend log:"
    tail -20 /tmp/backend_shm.log
    echo "Gateway log:"
    tail -20 /tmp/gateway_shm.log
    exit 1
fi
echo ""

# Test 2: Echo request
echo -e "${BLUE}Test 2: Echo request${NC}"
RESPONSE=$(curl -s -X POST http://$GATEWAY_ADDR/echo \
    -H "Content-Type: application/json" \
    -d '{"message": "Hello Shared Memory!"}')

if echo "$RESPONSE" | grep -q "Hello Shared Memory"; then
    echo -e "${GREEN}✓ Echo request successful${NC}"
    echo "Response: $RESPONSE"
else
    echo -e "${RED}✗ Echo request failed${NC}"
    echo "Response: $RESPONSE"
    echo "Backend log:"
    tail -20 /tmp/backend_shm.log
    echo "Gateway log:"
    tail -20 /tmp/gateway_shm.log
    exit 1
fi
echo ""

# Test 3: Multiple requests
echo -e "${BLUE}Test 3: Multiple requests${NC}"
for i in {1..5}; do
    RESPONSE=$(curl -s -X POST http://$GATEWAY_ADDR/echo \
        -H "Content-Type: application/json" \
        -d "{\"message\": \"Request $i\"}")
    if echo "$RESPONSE" | grep -q "Request $i"; then
        echo -e "${GREEN}✓ Request $i successful${NC}"
    else
        echo -e "${RED}✗ Request $i failed${NC}"
    fi
done
echo ""

# Test 4: Performance test with wrk (if available)
if command -v wrk &> /dev/null; then
    echo -e "${BLUE}Test 4: Performance test (5 seconds)${NC}"
    wrk -t4 -c64 -d5s -s scripts/wrk.lua http://$GATEWAY_ADDR/echo
else
    echo -e "${YELLOW}wrk not installed, skipping performance test${NC}"
fi

echo ""
echo -e "${GREEN}=== Shared Memory Test Completed ===${NC}"
echo ""
echo "To stop services:"
echo "  kill $BACKEND_PID $GATEWAY_PID"
