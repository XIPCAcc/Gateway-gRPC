#!/usr/bin/env python3
"""
分析 SHM 传输方式的矩阵乘法性能测试结果。

用法:
    python3 scripts/analyze_matrix_results.py [log_dir] [transport...]

    log_dir: 可选，默认为最新的 log/matrix_* 目录。
             例如: python3 scripts/analyze_matrix_results.py log/matrix_20260511-141214

    transport: 可选，指定要分析的传输方式，例如:
               python3 scripts/analyze_matrix_results.py shm-eventfd shm-uintr

输出:
    1. 完整 QPS/Latency 对比表
    2. 比值分析（按矩阵大小分组）
    3. 通知开销占比分析（推算 UINTR 潜力）
    4. 最有利于 UINTR 的场景排名
"""

import sys
import os
import re
import glob
from collections import defaultdict


def find_latest_log_dir():
    """自动找到最新的 matrix 测试日志目录"""
    dirs = sorted(glob.glob("log/matrix_*"), reverse=True)
    if dirs:
        return dirs[0]
    return None


def parse_wrk_log(filepath):
    """解析 wrk 输出日志，提取 QPS、平均延迟、P99 延迟"""
    if not os.path.exists(filepath):
        return None
    with open(filepath) as f:
        content = f.read()

    if "unable to connect" in content or "Connection refused" in content:
        return None

    qps_match = re.search(r'Requests/sec:\s+([\d.]+)', content)
    lat_match = re.search(r'Latency\s+([\d.]+)(us|ms|s)', content)
    p99_match = re.search(r'99%\s+([\d.]+)(us|ms|s)', content)
    socket_err_match = re.search(r'Socket errors:.*write (\d+)', content)

    if not qps_match:
        return None

    def to_ms(val, unit):
        val = float(val)
        if unit == 'us':
            return val / 1000
        elif unit == 's':
            return val * 1000
        return val

    result = {'qps': float(qps_match.group(1))}

    if lat_match:
        result['avg_lat_ms'] = to_ms(lat_match.group(1), lat_match.group(2))
    if p99_match:
        result['p99_lat_ms'] = to_ms(p99_match.group(1), p99_match.group(2))
    if socket_err_match:
        result['socket_write_errors'] = int(socket_err_match.group(1))

    return result


def main():
    log_dir = None
    filter_transports = None
    
    # Parse arguments
    if len(sys.argv) > 1:
        if os.path.isdir(sys.argv[1]):
            log_dir = sys.argv[1]
            if len(sys.argv) > 2:
                filter_transports = sys.argv[2:]
        else:
            log_dir = find_latest_log_dir()
            filter_transports = sys.argv[1:]
    else:
        log_dir = find_latest_log_dir()
    
    if not log_dir:
        print("Error: No log directory found. Run compare_shm_matrix.sh first.")
        sys.exit(1)

    print(f"Analyzing logs from: {log_dir}")
    if filter_transports:
        print(f"Filtering transports: {filter_transports}")
    print()

    results = {}

    for fname in os.listdir(log_dir):
        if not fname.endswith('.log'):
            continue

        match = re.match(r'(shm-\w+)_m(\d+)_c(\d+)\.log', fname)
        if not match:
            continue

        transport = match.group(1)
        matrix_size = int(match.group(2))
        concurrency = int(match.group(3))
        
        # Apply transport filter
        if filter_transports and transport not in filter_transports:
            continue

        data = parse_wrk_log(os.path.join(log_dir, fname))
        if data:
            results[(transport, matrix_size, concurrency)] = data

    if not results:
        print("Error: No valid results found in log directory.")
        sys.exit(1)

    transports = sorted(set(k[0] for k in results))
    matrix_sizes = sorted(set(k[1] for k in results))
    concurrencies = sorted(set(k[2] for k in results))

    # ================================================================
    # 1. 完整数据表
    # ================================================================
    print()
    print("=" * 110)
    print("  SHM Transport Matrix Multiplication Performance Comparison")
    print("=" * 110)
    print()

    header = f"{'Matrix':>8} {'Conc':>5}"
    for t in transports:
        header += f" {t + '_QPS':>14} {t + '_Lat':>12}"
    
    # Add ratio columns
    if 'shm-uds' in transports:
        if 'shm-eventfd' in transports:
            header += f" {'efd/uds':>12}"
        if 'shm-uintr' in transports:
            header += f" {'uintr/uds':>12}"
            if 'shm-eventfd' in transports:
                header += f" {'uintr/efd':>12}"
    
    print(header)
    print("-" * len(header))

    for ms in matrix_sizes:
        for cc in concurrencies:
            row = f"{ms}x{ms: <4} {cc:>5}"
            uds_qps = None
            efd_qps = None
            uintr_qps = None
            for t in transports:
                key = (t, ms, cc)
                if key in results:
                    d = results[key]
                    qps = d['qps']
                    lat = d.get('avg_lat_ms', None)
                    if lat is not None:
                        row += f" {qps:>14.1f} {lat:>11.2f}ms"
                    else:
                        row += f" {qps:>14.1f} {'N/A':>12}"
                    if t == 'shm-uds':
                        uds_qps = qps
                    elif t == 'shm-eventfd':
                        efd_qps = qps
                    elif t == 'shm-uintr':
                        uintr_qps = qps
                else:
                    row += f" {'N/A':>14} {'N/A':>12}"

            # Add ratio columns
            if 'shm-uds' in transports:
                if 'shm-eventfd' in transports:
                    if uds_qps and efd_qps and uds_qps > 0:
                        ratio = efd_qps / uds_qps
                        row += f" {ratio:>11.2f}x"
                    else:
                        row += f" {'N/A':>12}"
                
                if 'shm-uintr' in transports:
                    if uds_qps and uintr_qps and uds_qps > 0:
                        ratio = uintr_qps / uds_qps
                        row += f" {ratio:>11.2f}x"
                    else:
                        row += f" {'N/A':>12}"
                    
                    if 'shm-eventfd' in transports:
                        if efd_qps and uintr_qps and efd_qps > 0:
                            ratio = uintr_qps / efd_qps
                            row += f" {ratio:>11.2f}x"
                        else:
                            row += f" {'N/A':>12}"

            print(row)
        print()

    # ================================================================
    # 2. 比值分析
    # ================================================================
    print()
    print("=" * 110)
    print("  ANALYSIS: QPS Ratios by Matrix Size")
    print("=" * 110)
    print()

    # Prepare data
    ratio_data = defaultdict(list)
    
    for ms in matrix_sizes:
        for cc in concurrencies:
            key_uds = ('shm-uds', ms, cc)
            key_efd = ('shm-eventfd', ms, cc)
            key_uintr = ('shm-uintr', ms, cc)
            
            # Generate all possible pairwise ratios
            if key_uds in results:
                uds_qps = results[key_uds]['qps']
                
                if key_efd in results and uds_qps > 0:
                    efd_qps = results[key_efd]['qps']
                    ratio_data[ms].append(('efd/uds', cc, efd_qps / uds_qps))
                
                if key_uintr in results and uds_qps > 0:
                    uintr_qps = results[key_uintr]['qps']
                    ratio_data[ms].append(('uintr/uds', cc, uintr_qps / uds_qps))
            
            if key_efd in results and key_uintr in results:
                efd_qps = results[key_efd]['qps']
                uintr_qps = results[key_uintr]['qps']
                if efd_qps > 0:
                    ratio_data[ms].append(('uintr/efd', cc, uintr_qps / efd_qps))

    # Print analysis
    for ms in matrix_sizes:
        print()
        print(f"  {ms}x{ms}:")
        
        # Group ratios by type
        type_ratios = defaultdict(list)
        for ratio_type, cc, ratio in ratio_data.get(ms, []):
            type_ratios[ratio_type].append((cc, ratio))
        
        for ratio_type, ratios in sorted(type_ratios.items()):
            avg_ratio = sum(r[1] for r in ratios) / len(ratios)
            print(f"    {ratio_type}: avg {avg_ratio:.3f}x (over {len(ratios)} concurrencies)")
            
            for cc, ratio in ratios:
                marker = " ***" if ratio > 1.15 else ("  **" if ratio > 1.08 else "")
                print(f"      c={cc:>5}: {ratio:.3f}x{marker}")

    # ================================================================
    # 3. 通知开销占比分析（推算 UINTR 潜力）
    # ================================================================
    print()
    print("=" * 110)
    print("  ANALYSIS: Notification Overhead & UINTR Potential")
    print("=" * 110)
    print()
    print("  方法: 以最大矩阵在最低并发下的延迟为纯计算时间基准，")
    print("        按 O(n^3) 推算小矩阵的计算时间，剩余即为通知开销。")
    print()

    # 找最大矩阵在 c16 的数据作为基准
    baseline_ms = max(matrix_sizes)
    baseline_key_uds = ('shm-uds', baseline_ms, 16)
    baseline_key_efd = ('shm-eventfd', baseline_ms, 16)

    if baseline_key_uds in results:
        base_lat_uds = results[baseline_key_uds]['avg_lat_ms']
        base_lat_efd = results[baseline_key_efd]['avg_lat_ms'] if baseline_key_efd in results else base_lat_uds

        print(f"  Baseline: {baseline_ms}x{baseline_ms} @ c16")
        print(f"    uds latency:     {base_lat_uds*1000:.0f}us")
        print(f"    eventfd latency: {base_lat_efd*1000:.0f}us")
        print()

        print(f"  {'Matrix':>8} {'UDS_Lat':>10} {'EstComp':>10} {'NotifOH':>10} {'Notif%':>8} {'UINTR':>8}")
        print(f"  {'------':>8} {'-------':>10} {'-------':>10} {'-------':>10} {'------':>8} {'-----':>8}")

        for ms in matrix_sizes:
            if ('shm-uds', ms, 16) not in results:
                continue
            actual_lat = results[('shm-uds', ms, 16)]['avg_lat_ms']
            ops_ratio = (baseline_ms ** 3) / (ms ** 3)
            est_compute = base_lat_uds / ops_ratio
            notif_oh = actual_lat - est_compute
            notif_pct = (notif_oh / actual_lat * 100) if actual_lat > 0 else 0

            # UINTR 潜力评估
            if notif_pct > 90:
                uintr_level = "★★★★★"
            elif notif_pct > 70:
                uintr_level = "★★★★"
            elif notif_pct > 40:
                uintr_level = "★★★"
            elif notif_pct > 15:
                uintr_level = "★★"
            else:
                uintr_level = "★"

            print(f"  {ms}x{ms: <4} {actual_lat*1000:>8.0f}us {est_compute*1000:>8.0f}us {notif_oh*1000:>8.0f}us {notif_pct:>7.1f}% {uintr_level:>8}")

    # ================================================================
    # 4. 最有利于 UINTR 的场景排名
    # ================================================================
    print()
    print("=" * 110)
    print("  SUMMARY: Most Favorable Scenarios for UINTR")
    print("=" * 110)
    print()
    print("  UINTR 通过 senduipi 用户态指令替代内核系统调用，优势在于:")
    print("    1. 零内核切换开销")
    print("    2. 更好的 CPU Cache 局部性（不污染内核代码路径）")
    print("    3. 无调度器干扰（内核态切换可能导致进程被抢占）")
    print()
    print("  规律: 矩阵越小 + 并发越高 → UINTR 优势越大")
    print("        因为小矩阵计算极快，通知开销成为绝对瓶颈")
    print()

    # Check if we have actual uintr data
    has_uintr = any(k[0] == 'shm-uintr' for k in results)
    
    if has_uintr:
        print("  Note: Using actual uintr results from tests")
    else:
        print("  Note: No uintr results found, using eventfd/uds ratio as proxy")
    print()

    best_scenarios = []
    for ms in matrix_sizes:
        for cc in concurrencies:
            key_uds = ('shm-uds', ms, cc)
            key_efd = ('shm-eventfd', ms, cc)
            key_uintr = ('shm-uintr', ms, cc)
            
            # Determine which transports to compare
            if key_uds in results and key_efd in results and key_uintr in results:
                # All three available, compare uintr vs uds
                uds_qps = results[key_uds]['qps']
                uintr_qps = results[key_uintr]['qps']
                uds_lat = results[key_uds].get('avg_lat_ms', 0)
                ratio = uintr_qps / uds_qps if uds_qps > 0 else 0
                best_scenarios.append((ratio, ms, cc, uds_qps, uintr_qps, None, uds_lat, 'uintr/uds'))
            elif key_efd in results and key_uintr in results:
                # Only eventfd and uintr available
                efd_qps = results[key_efd]['qps']
                uintr_qps = results[key_uintr]['qps']
                efd_lat = results[key_efd].get('avg_lat_ms', 0)
                ratio = uintr_qps / efd_qps if efd_qps > 0 else 0
                best_scenarios.append((ratio, ms, cc, efd_qps, uintr_qps, None, efd_lat, 'uintr/efd'))
            elif key_uds in results and key_efd in results:
                # Only uds and eventfd available
                uds_qps = results[key_uds]['qps']
                efd_qps = results[key_efd]['qps']
                uds_lat = results[key_uds].get('avg_lat_ms', 0)
                ratio = efd_qps / uds_qps if uds_qps > 0 else 0
                best_scenarios.append((ratio, ms, cc, uds_qps, efd_qps, None, uds_lat, 'efd/uds'))

    best_scenarios.sort(reverse=True)

    if best_scenarios:
        ratio_type = best_scenarios[0][7]
        
        # Build table header based on ratio type
        if ratio_type == 'uintr/uds':
            print(f"  {'Rank':>5} {'Matrix':>8} {'Conc':>6} {'UINTR/UDS':>12} {'UDS_QPS':>10} {'UINTR_QPS':>12} {'UDS_Lat':>10} {'UINTR':>10}")
            print(f"  {'----':>5} {'------':>8} {'----':>6} {'---------':>12} {'-------':>10} {'---------':>12} {'-------':>10} {'-----':>10}")
        elif ratio_type == 'uintr/efd':
            print(f"  {'Rank':>5} {'Matrix':>8} {'Conc':>6} {'UINTR/EFD':>12} {'EFD_QPS':>10} {'UINTR_QPS':>12} {'EFD_Lat':>10} {'UINTR':>10}")
            print(f"  {'----':>5} {'------':>8} {'----':>6} {'---------':>12} {'-------':>10} {'---------':>12} {'-------':>10} {'-----':>10}")
        else:
            print(f"  {'Rank':>5} {'Matrix':>8} {'Conc':>6} {'EFD/UDS':>10} {'UDS_QPS':>10} {'EFD_QPS':>10} {'UDS_Lat':>10} {'UINTR':>10}")
            print(f"  {'----':>5} {'------':>8} {'----':>6} {'-------':>10} {'-------':>10} {'-------':>10} {'-------':>10} {'-----':>10}")

        for i, (ratio, ms, cc, base_qps, comp_qps, _, base_lat, ratio_type) in enumerate(best_scenarios[:20]):
            if ratio > 1.20:
                potential = "★★★★★"
            elif ratio > 1.12:
                potential = "★★★★"
            elif ratio > 1.06:
                potential = "★★★"
            elif ratio > 1.02:
                potential = "★★"
            else:
                potential = "★"
            
            print(f"  {i+1:>5} {ms}x{ms: <4} {cc:>6} {ratio:>11.3f}x {base_qps:>10.1f} {comp_qps:>12.1f} {base_lat:>9.2f}ms {potential:>10}")

    print()
    if best_scenarios:
        ratio_type = best_scenarios[0][7]
        if ratio_type == 'uintr/uds':
            print("  ★★★★★ = UINTR 极佳场景 (uintr 比 uds 快 20%+)")
            print("  ★★★★  = UINTR 优秀场景 (uintr 比 uds 快 12-20%)")
            print("  ★★★   = UINTR 良好场景 (uintr 比 uds 快 6-12%)")
            print("  ★★    = UINTR 一般场景 (uintr 比 uds 快 2-6%)")
            print("  ★     = UINTR 优势微弱 (通知开销不显著)")
        elif ratio_type == 'uintr/efd':
            print("  ★★★★★ = UINTR 极佳场景 (uintr 比 eventfd 快 20%+)")
            print("  ★★★★  = UINTR 优秀场景 (uintr 比 eventfd 快 12-20%)")
            print("  ★★★   = UINTR 良好场景 (uintr 比 eventfd 快 6-12%)")
            print("  ★★    = UINTR 一般场景 (uintr 比 eventfd 快 2-6%)")
            print("  ★     = UINTR 优势微弱 (通知开销不显著)")
        else:
            print("  ★★★★★ = UINTR 极佳场景 (eventfd 比 uds 快 20%+)")
            print("  ★★★★  = UINTR 优秀场景 (eventfd 比 uds 快 12-20%)")
            print("  ★★★   = UINTR 良好场景 (eventfd 比 uds 快 6-12%)")
            print("  ★★    = UINTR 一般场景 (eventfd 比 uds 快 2-6%)")
            print("  ★     = UINTR 优势微弱 (通知开销不显著)")


if __name__ == '__main__':
    main()