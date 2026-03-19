#!/bin/bash

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo -e "${YELLOW}Stopping services...${NC}"

pkill -f "target/debug/backend" 2>/dev/null && echo -e "${GREEN}✓ Backend stopped${NC}" || echo -e "${RED}Backend not running${NC}"
pkill -f "target/debug/gateway" 2>/dev/null && echo -e "${GREEN}✓ Gateway stopped${NC}" || echo -e "${RED}Gateway not running${NC}"

echo -e "${GREEN}All services stopped${NC}"
