#!/bin/bash

# System resource monitoring script using perf, pidstat, and other tools

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Configuration
OUTPUT_DIR="./results/metrics"
DURATION=${1:-60}
SAMPLE_INTERVAL=${2:-1}

mkdir -p $OUTPUT_DIR

echo -e "${GREEN}=== System Resource Monitor ===${NC}"
echo "Duration: ${DURATION}s"
echo "Sample interval: ${SAMPLE_INTERVAL}s"
echo "Output: $OUTPUT_DIR"
echo ""

# Find PIDs
get_pids() {
    BACKEND_PID=$(pgrep -f "./target/release/backend" | head -1)
    GATEWAY_PID=$(pgrep -f "./target/release/gateway" | head -1)
}

# Monitor with perf stat
monitor_perf() {
    local pid=$1
    local name=$2
    
    if [ -z "$pid" ]; then
        echo -e "${YELLOW}Skipping perf for $name (PID not found)${NC}"
        return
    fi
    
    echo -e "${BLUE}Starting perf stat for $name (PID: $pid)${NC}"
    
    perf stat \
        -p $pid \
        -e syscalls:sys_enter,syscalls:sys_exit \
        -e context-switches,cpu-migrations \
        -e cpu-clock,task-clock \
        -e cycles,instructions,cache-references,cache-misses \
        -e branches,branch-misses \
        -I $((SAMPLE_INTERVAL * 1000) \
        -o "$OUTPUT_DIR/${name}_perf.log" \
        sleep $DURATION &
}

# Monitor with pidstat
monitor_pidstat() {
    local pid=$1
    local name=$2
    
    if [ -z "$pid" ]; then
        echo -e "${YELLOW}Skipping pidstat for $name (PID not found)${NC}"
        return
    fi
    
    echo -e "${BLUE}Starting pidstat for $name (PID: $pid)${NC}"
    
    # CPU and memory stats
    pidstat -u -r -s -p $pid $SAMPLE_INTERVAL $DURATION > "$OUTPUT_DIR/${name}_pidstat.log" 2>&1 &
    
    # Context switches
    pidstat -w -p $pid $SAMPLE_INTERVAL $DURATION > "$OUTPUT_DIR/${name}_pidstat_ctxsw.log" 2>&1 &
}

# Monitor with top (snapshot)
monitor_top() {
    echo -e "${BLUE}Taking top snapshot${NC}"
    top -bn1 | head -20 > "$OUTPUT_DIR/top_snapshot.log"
}

# Monitor /proc/PID/stat
monitor_proc_stat() {
    local pid=$1
    local name=$2
    
    if [ -z "$pid" ]; then
        return
    fi
    
    echo -e "${BLUE}Monitoring /proc/$pid/stat${NC}"
    
    echo "timestamp,utime,stime,cutime,cstime,num_threads,vsize,rss" > "$OUTPUT_DIR/${name}_procstat.csv"
    
    for ((i=0; i<DURATION; i+=SAMPLE_INTERVAL)); do
        if [ -f "/proc/$pid/stat" ]; then
            local stat=$(cat /proc/$pid/stat 2>/dev/null)
            if [ -n "$stat" ]; then
                # Extract fields from /proc/PID/stat
                # Format: pid (comm) state ppid pgrp session tty_nr tpgid flags minflt cminflt majflt cmajflt utime stime cutime cstime priority nice num_threads itrealvalue starttime vsize rss rsslim ...
                local utime=$(echo "$stat" | awk '{print $14}')
                local stime=$(echo "$stat" | awk '{print $15}')
                local cutime=$(echo "$stat" | awk '{print $16}')
                local cstime=$(echo "$stat" | awk '{print $17}')
                local num_threads=$(echo "$stat" | awk '{print $20}')
                local vsize=$(echo "$stat" | awk '{print $23}')
                local rss=$(echo "$stat" | awk '{print $24}')
                
                echo "$(date +%s),$utime,$stime,$cutime,$cstime,$num_threads,$vsize,$rss" >> "$OUTPUT_DIR/${name}_procstat.csv"
            fi
        fi
        sleep $SAMPLE_INTERVAL
    done &
}

# Monitor network stats
monitor_network() {
    echo -e "${BLUE}Monitoring network stats${NC}"
    
    echo "timestamp,interface,rx_bytes,tx_bytes,rx_packets,tx_packets" > "$OUTPUT_DIR/network.csv"
    
    for ((i=0; i<DURATION; i+=SAMPLE_INTERVAL)); do
        for iface in $(ls /sys/class/net/ | grep -E "^(eth|ens|enp)"); do
            if [ -f "/sys/class/net/$iface/statistics/rx_bytes" ]; then
                local rx_bytes=$(cat /sys/class/net/$iface/statistics/rx_bytes)
                local tx_bytes=$(cat /sys/class/net/$iface/statistics/tx_bytes)
                local rx_packets=$(cat /sys/class/net/$iface/statistics/rx_packets)
                local tx_packets=$(cat /sys/class/net/$iface/statistics/tx_packets)
                
                echo "$(date +%s),$iface,$rx_bytes,$tx_bytes,$rx_packets,$tx_packets" >> "$OUTPUT_DIR/network.csv"
            fi
        done
        sleep $SAMPLE_INTERVAL
    done &
}

# Generate summary report
generate_report() {
    echo -e "${GREEN}=== Generating Resource Report ===${NC}"
    
    local report_file="$OUTPUT_DIR/resource_report.txt"
    
    echo "Resource Monitoring Report" > $report_file
    echo "Generated: $(date)" >> $report_file
    echo "Duration: ${DURATION}s" >> $report_file
    echo "=======================================" >> $report_file
    echo "" >> $report_file
    
    # Process perf results
    for f in "$OUTPUT_DIR"/*_perf.log; do
        if [ -f "$f" ]; then
            echo "--- $(basename $f) ---" >> $report_file
            cat "$f" >> $report_file
            echo "" >> $report_file
        fi
    done
    
    # Process pidstat results
    for f in "$OUTPUT_DIR"/*_pidstat.log; do
        if [ -f "$f" ]; then
            echo "--- $(basename $f) ---" >> $report_file
            tail -20 "$f" >> $report_file
            echo "" >> $report_file
        fi
    done
    
    echo -e "${GREEN}Report saved to: $report_file${NC}"
}

# Main execution
main() {
    get_pids
    
    echo -e "${YELLOW}Backend PID: ${BACKEND_PID:-Not found}${NC}"
    echo -e "${YELLOW}Gateway PID: ${GATEWAY_PID:-Not found}${NC}"
    echo ""
    
    # Start monitors
    monitor_perf "$BACKEND_PID" "backend"
    monitor_perf "$GATEWAY_PID" "gateway"
    
    monitor_pidstat "$BACKEND_PID" "backend"
    monitor_pidstat "$GATEWAY_PID" "gateway"
    
    monitor_proc_stat "$BACKEND_PID" "backend"
    monitor_proc_stat "$GATEWAY_PID" "gateway"
    
    monitor_network
    monitor_top
    
    echo -e "${YELLOW}Monitoring for ${DURATION}s...${NC}"
    
    # Wait for completion
    sleep $DURATION
    wait
    
    # Generate report
    generate_report
    
    echo -e "${GREEN}=== Monitoring Complete ===${NC}"
    echo "Results saved to: $OUTPUT_DIR/"
}

# Handle Ctrl+C
trap 'echo -e "${RED}Interrupted${NC}"; exit 1' INT

main
