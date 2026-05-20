#!/usr/bin/env python3
"""
专门分析 SHM 往返延迟的脚本（不涉及 QPS）
从 gateway 日志中提取 SHM 延迟数据，保存结果到文件
"""

import sys
import os
import re
import csv
from datetime import datetime

def parse_gateway_log(filepath):
    if not os.path.exists(filepath):
        return None

    with open(filepath) as f:
        content = f.read()

    latency_matches = re.findall(r'SHM:\s*(\d+)us', content)

    if not latency_matches:
        return None

    latencies = [int(x) for x in latency_matches]
    latencies_sorted = sorted(latencies)
    n = len(latencies)

    avg_latency_us = sum(latencies) / n
    mean = avg_latency_us
    variance = sum((x - mean) ** 2 for x in latencies) / n
    std_latency_us = variance ** 0.5

    return {
        'shm_roundtrip_us': avg_latency_us,
        'shm_min_us': latencies_sorted[0],
        'shm_max_us': latencies_sorted[-1],
        'shm_std_us': std_latency_us,
        'shm_p50_us': latencies_sorted[int(n * 0.5)],
        'shm_p75_us': latencies_sorted[int(n * 0.75)],
        'shm_p90_us': latencies_sorted[int(n * 0.9)],
        'shm_p95_us': latencies_sorted[int(n * 0.95)],
        'shm_p99_us': latencies_sorted[int(n * 0.99)],
        'shm_count': n,
        'shm_latencies': latencies
    }


def main():
    if len(sys.argv) < 2:
        print("Usage: python3 scripts/analyze_shm_latency.py <log_dir>")
        print("Example: python3 scripts/analyze_shm_latency.py log/matrix_20260517-231426")
        sys.exit(1)

    log_dir = sys.argv[1]

    if not os.path.exists(log_dir):
        print(f"Error: Log directory {log_dir} not found")
        sys.exit(1)

    output_dir = os.path.join(log_dir, "shm_analysis")
    os.makedirs(output_dir, exist_ok=True)

    print(f"Analyzing logs from: {log_dir}")
    print(f"Output will be saved to: {output_dir}")
    print()

    results = {}

    for fname in os.listdir(log_dir):
        if not fname.endswith('_gateway.log'):
            continue

        match = re.match(r'(shm-\w+)_m(\d+)_c(\d+)_r(\d+)_gateway\.log', fname)
        if not match:
            continue

        transport = match.group(1)
        matrix_size = int(match.group(2))
        concurrency = int(match.group(3))
        run_num = int(match.group(4))

        data = parse_gateway_log(os.path.join(log_dir, fname))
        if data:
            results[(transport, matrix_size, concurrency, run_num)] = data

    if not results:
        print("No valid data found in log directory")
        sys.exit(1)

    transports = sorted(set(k[0] for k in results))
    matrix_sizes = sorted(set(k[1] for k in results))
    concurrencies = sorted(set(k[2] for k in results))
    run_nums = sorted(set(k[3] for k in results))

    transports = sorted(transports, key=lambda t: (0 if t == 'shm-uintr' else 1, t))

    print(f"Found {len(results)} test cases")
    print(f"Transports: {transports}")
    print(f"Matrix sizes: {matrix_sizes}")
    print(f"Concurrences: {concurrencies}")
    print(f"Runs: {run_nums}")
    print()

    csv_filename = os.path.join(output_dir, "shm_latency_results.csv")
    print(f"Saving detailed results to: {csv_filename}")

    csv_headers = [
        'Transport', 'Matrix', 'Concurrency', 'Run',
        'SHM_Avg_us', 'SHM_Min_us', 'SHM_Max_us', 'SHM_Std_us',
        'SHM_P50_us', 'SHM_P75_us', 'SHM_P90_us', 'SHM_P95_us', 'SHM_P99_us',
        'SHM_Count'
    ]

    with open(csv_filename, 'w', newline='') as csvfile:
        writer = csv.DictWriter(csvfile, fieldnames=csv_headers)
        writer.writeheader()

        for (t, ms, cc, rn), data in sorted(results.items()):
            row = {
                'Transport': t,
                'Matrix': ms,
                'Concurrency': cc,
                'Run': rn,
                'SHM_Avg_us': data.get('shm_roundtrip_us', ''),
                'SHM_Min_us': data.get('shm_min_us', ''),
                'SHM_Max_us': data.get('shm_max_us', ''),
                'SHM_Std_us': data.get('shm_std_us', ''),
                'SHM_P50_us': data.get('shm_p50_us', ''),
                'SHM_P75_us': data.get('shm_p75_us', ''),
                'SHM_P90_us': data.get('shm_p90_us', ''),
                'SHM_P95_us': data.get('shm_p95_us', ''),
                'SHM_P99_us': data.get('shm_p99_us', ''),
                'SHM_Count': data.get('shm_count', ''),
            }
            writer.writerow(row)

    report_filename = os.path.join(output_dir, "shm_analysis_report.txt")
    print(f"Saving analysis report to: {report_filename}")

    with open(report_filename, 'w') as f:
        f.write("=" * 100 + "\n")
        f.write("SHM 往返延迟分析报告\n")
        f.write("=" * 100 + "\n")
        f.write(f"生成时间: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}\n")
        f.write(f"日志目录: {log_dir}\n")
        f.write(f"测试数量: {len(results)}\n")
        f.write(f"传输方式: {', '.join(transports)}\n")
        f.write("\n")

        f.write("=" * 100 + "\n")
        f.write("SHM 往返延迟详细数据\n")
        f.write("=" * 100 + "\n\n")

        header = f"{'传输方式':>12} {'矩阵':>8} {'并发':>6} {'轮':>4} {'平均(us)':>10} {'最小':>8} {'最大':>8} {'P50':>8} {'P95':>8} {'P99':>8} {'样本':>8}\n"
        f.write(header)
        f.write("-" * len(header) + "\n")

        for ms in matrix_sizes:
            for cc in concurrencies:
                for t in transports:
                    for rn in run_nums:
                        key = (t, ms, cc, rn)
                        if key in results:
                            d = results[key]
                            f.write(f"{t:>12} {ms:>8} {cc:>6} {rn:>4} "
                                    f"{d['shm_roundtrip_us']:>10.1f} {d['shm_min_us']:>8.0f} "
                                    f"{d['shm_max_us']:>8.0f} {d['shm_p50_us']:>8.0f} "
                                    f"{d['shm_p95_us']:>8.0f} {d['shm_p99_us']:>8.0f} "
                                    f"{d['shm_count']:>8}\n")
            f.write("\n")

        f.write("\n" + "=" * 100 + "\n")
        f.write("按传输方式分组的延迟统计\n")
        f.write("=" * 100 + "\n\n")

        for t in transports:
            f.write(f"\n传输方式: {t}\n")
            f.write("-" * 100 + "\n")
            f.write(f"{'矩阵':>8} {'并发':>6} {'轮':>4} {'平均(us)':>10} {'最小':>8} {'最大':>8} {'P50':>8} {'P99':>8} {'样本':>8}\n")
            f.write("-" * 100 + "\n")

            for ms in matrix_sizes:
                for cc in concurrencies:
                    for rn in run_nums:
                        key = (t, ms, cc, rn)
                        if key in results:
                            d = results[key]
                            f.write(f"{ms:>8} {cc:>6} {rn:>4} "
                                    f"{d['shm_roundtrip_us']:>10.1f} {d['shm_min_us']:>8.0f} "
                                    f"{d['shm_max_us']:>8.0f} {d['shm_p50_us']:>8.0f} "
                                    f"{d['shm_p99_us']:>8.0f} {d['shm_count']:>8}\n")
            f.write("\n")

        if len(transports) >= 2:
            f.write("\n" + "=" * 100 + "\n")
            f.write("传输方式SHM延迟对比（us，多轮平均）\n")
            f.write("=" * 100 + "\n\n")

            for ms in matrix_sizes:
                f.write(f"\n矩阵大小: {ms}x{ms}\n")
                f.write("-" * 100 + "\n")
                header = f"{'并发':>6}"
                for t in transports:
                    header += f" {t + '_Avg':>14}"
                for i in range(1, len(transports)):
                    header += f" {transports[i] + '/' + transports[0]:>12}"
                    if i >= 2:
                        header += f" {transports[i] + '/' + transports[i-1]:>12}"
                f.write(header + "\n")
                f.write("-" * len(header) + "\n")

                for cc in concurrencies:
                    values = []
                    has_values = False
                    for t in transports:
                        vals = []
                        for rn in run_nums:
                            key = (t, ms, cc, rn)
                            if key in results:
                                vals.append(results[key]['shm_roundtrip_us'])
                        if vals:
                            values.append(sum(vals) / len(vals))
                            has_values = True
                        else:
                            values.append(None)

                    if has_values:
                        line = f"{cc:>6}"
                        for v in values:
                            if v is not None:
                                line += f" {v:>14.1f}"
                            else:
                                line += f" {'N/A':>14}"

                        if len(values) >= 2 and values[0] and values[1]:
                            line += f" {values[1] / values[0]:>12.2f}x"
                        else:
                            line += f" {'N/A':>12}"

                        if len(values) >= 3:
                            if values[0] and values[2]:
                                line += f" {values[2] / values[0]:>12.2f}x"
                            else:
                                line += f" {'N/A':>12}"
                            if values[1] and values[2]:
                                line += f" {values[2] / values[1]:>12.2f}x"
                            else:
                                line += f" {'N/A':>12}"

                        f.write(line + "\n")
                f.write("\n")

    print()
    print("Analysis complete!")
    print(f"Results saved to:")
    print(f"  - {csv_filename}")
    print(f"  - {report_filename}")


if __name__ == '__main__':
    main()