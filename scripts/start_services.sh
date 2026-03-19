#!/bin/bash
set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

GATEWAY_ADDR="127.0.0.1:8080"
BACKEND_ADDR="127.0.0.1:50051"
UDS_PATH="/tmp/backend.sock"
DELAY_US=${1:-1000}
TRANSPORT=${2:-"tcp"}

echo -e "${GREEN}=== Starting Services ===${NC}"

if [ ! -f ./target/debug/backend ]; then
    echo -e "${RED}Backend binary not found. Building...${NC}"
    cargo build --bin backend
fi

if [ ! -f ./target/debug/gateway ]; then
    echo -e "${RED}Gateway binary not found. Building...${NC}"
    cargo build --bin gateway
fi

pkill -f "target/debug/backend" 2>/dev/null || true
pkill -f "target/debug/gateway" 2>/dev/null || true
sleep 1

# Clean up existing socket file if using UDS
if [ "$TRANSPORT" = "uds" ]; then
    if [ -S "$UDS_PATH" ]; then
        echo -e "${YELLOW}Removing existing socket file: $UDS_PATH${NC}"
        rm -f "$UDS_PATH"
    fi
    
    echo -e "${YELLOW}Starting backend on $UDS_PATH (UDS, delay: ${DELAY_US}us)${NC}"
    ./target/debug/backend --transport uds --uds-path "$UDS_PATH" --delay-us $DELAY_US > /tmp/backend.log 2>&1 &
else
    echo -e "${YELLOW}Starting backend on $BACKEND_ADDR (TCP, delay: ${DELAY_US}us)${NC}"
    ./target/debug/backend --addr $BACKEND_ADDR --delay-us $DELAY_US > /tmp/backend.log 2>&1 &
fi

BACKEND_PID=$!
echo "Backend PID: $BACKEND_PID"

sleep 2

if [ "$TRANSPORT" = "uds" ]; then
    echo -e "${YELLOW}Starting gateway on $GATEWAY_ADDR (UDS)${NC}"
    ./target/debug/gateway --listen-addr $GATEWAY_ADDR --backend-addr "$UDS_PATH" --transport uds --uds-path "$UDS_PATH" > /tmp/gateway.log 2>&1 &
else
    echo -e "${YELLOW}Starting gateway on $GATEWAY_ADDR (TCP)${NC}"
    ./target/debug/gateway --listen-addr $GATEWAY_ADDR --backend-addr $BACKEND_ADDR --transport tcp > /tmp/gateway.log 2>&1 &
fi

GATEWAY_PID=$!
echo "Gateway PID: $GATEWAY_PID"

sleep 2

echo -e "${YELLOW}Checking services...${NC}"
if curl -s http://$GATEWAY_ADDR/health > /dev/null; then
    echo -e "${GREEN}✓ Services started successfully${NC}"
    if [ "$TRANSPORT" = "uds" ]; then
        echo -e "${GREEN}  Backend: $UDS_PATH (UDS)${NC}"
    else
        echo -e "${GREEN}  Backend: $BACKEND_ADDR (TCP)${NC}"
    fi
    echo -e "${GREEN}  Gateway: $GATEWAY_ADDR (PID: $GATEWAY_PID)${NC}"
    echo -e "${GREEN}  Transport: $TRANSPORT${NC}"
    echo ""
    echo "To stop services: ./scripts/stop_services.sh"
else
    echo -e "${RED}✗ Failed to start services${NC}"
    echo "Backend log:"
    tail -20 /tmp/backend.log
    echo "Gateway log:"
    tail -20 /tmp/gateway.log
    exit 1
fi
