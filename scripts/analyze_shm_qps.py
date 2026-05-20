#!/usr/bin/env python3
"""
专门分析 QPS 吞吐量数据的脚本（不涉及 SHM 延迟）
从 wrk 日志中提取 QPS 数据，保存结果到文件
"""

import sys
import os
import re
import csv
from datetime import datetime


def parse_wrk_log(filepath):
    if not os.path.exists(filepath):
        return None

    with open(filepath) as f:
        content = f.read()

    if "unable to connect" in content or "Connection refused" in content:
        return None

    qps_match = re.search(r'Requests/sec:\s+([\d.]+)', content)
    lat_match = re.search(r'Latency\s+([\d.]+)(us|ms|s)', content)

    if not qps_match:
        return None

    def to_us(val, unit):
        val = float(val)
        if unit == 'ms':
            return val * 1000
        elif unit == 's':
            return val * 1000000
        return val

    result = {'qps': float(qps_match.group(1))}
    if lat_match:
        result['avg_lat_us'] = to_us(lat_match.group(1), lat_match.group(2))

    return result


def main():
    if len(sys.argv) < 2:
        print("Usage: python3 scripts/analyze_qps.py <log_dir>")
        print("Example: python3 scripts/analyze_qps.py log/qps_matrix_20260522-120000")
        print("         python3 scripts/analyze_qps.py log/matrix_20260521-220745")
        sys.exit(1)

    log_dir = sys.argv[1]

    if not os.path.exists(log_dir):
        print(f"Error: Log directory {log_dir} not found")
        sys.exit(1)

    output_dir = os.path.join(log_dir, "qps_analysis")
    os.makedirs(output_dir, exist_ok=True)

    print(f"Analyzing logs from: {log_dir}")
    print(f"Output will be saved to: {output_dir}")
    print()

    results = {}

    for fname in os.listdir(log_dir):
        if not fname.endswith('.log') or fname.endswith('_gateway.log'):
            continue

        match = re.match(r'(shm-\w+)_m(\d+)_c(\d+)_r(\d+)\.log', fname)
        if match:
            transport = match.group(1)
            matrix_size = int(match.group(2))
            concurrency = int(match.group(3))
            run_num = int(match.group(4))
        else:
            match = re.match(r'(uintr)_m(\d+)_c(\d+)_r(\d+)\.log', fname)
            if match:
                transport = 'shm-uintr'
                matrix_size = int(match.group(2))
                concurrency = int(match.group(3))
                run_num = int(match.group(4))
            else:
                continue

        data = parse_wrk_log(os.path.join(log_dir, fname))
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

    csv_filename = os.path.join(output_dir, "qps_results.csv")
    print(f"Saving detailed results to: {csv_filename}")

    csv_headers = [
        'Transport', 'Matrix', 'Concurrency', 'Run',
        'QPS', 'AvgLat_us'
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
                'QPS': data.get('qps', ''),
                'AvgLat_us': data.get('avg_lat_us', ''),
            }
            writer.writerow(row)

    report_filename = os.path.join(output_dir, "qps_analysis_report.txt")
    print(f"Saving analysis report to: {report_filename}")

    with open(report_filename, 'w') as f:
        f.write("=" * 100 + "\n")
        f.write("QPS 性能分析报告\n")
        f.write("=" * 100 + "\n")
        f.write(f"生成时间: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}\n")
        f.write(f"日志目录: {log_dir}\n")
        f.write(f"测试数量: {len(results)}\n")
        f.write(f"传输方式: {', '.join(transports)}\n")
        f.write("\n")

        f.write("=" * 100 + "\n")
        f.write("QPS 详细数据\n")
        f.write("=" * 100 + "\n\n")

        header = f"{'传输方式':>12} {'矩阵':>8} {'并发':>6} {'轮':>4} {'QPS':>12} {'平均延迟(us)':>14}\n"
        f.write(header)
        f.write("-" * len(header) + "\n")

        for ms in matrix_sizes:
            for cc in concurrencies:
                for t in transports:
                    for rn in run_nums:
                        key = (t, ms, cc, rn)
                        if key in results:
                            d = results[key]
                            qps_str = f"{d['qps']:,.1f}" if d.get('qps') is not None else 'N/A'
                            avg_lat_str = f"{d['avg_lat_us']:.0f}" if d.get('avg_lat_us') is not None else 'N/A'
                            f.write(f"{t:>12} {ms:>8} {cc:>6} {rn:>4} "
                                    f"{qps_str:>12} {avg_lat_str:>14}\n")
            f.write("\n")

        f.write("\n" + "=" * 100 + "\n")
        f.write("按传输方式分组的 QPS 统计\n")
        f.write("=" * 100 + "\n\n")

        for t in transports:
            f.write(f"\n传输方式: {t}\n")
            f.write("-" * 100 + "\n")
            f.write(f"{'矩阵':>8} {'并发':>6} {'轮':>4} {'QPS':>12} {'平均延迟(us)':>14}\n")
            f.write("-" * 100 + "\n")

            for ms in matrix_sizes:
                for cc in concurrencies:
                    for rn in run_nums:
                        key = (t, ms, cc, rn)
                        if key in results:
                            d = results[key]
                            qps_str = f"{d['qps']:,.1f}" if d.get('qps') is not None else 'N/A'
                            avg_lat_str = f"{d['avg_lat_us']:.0f}" if d.get('avg_lat_us') is not None else 'N/A'
                            f.write(f"{ms:>8} {cc:>6} {rn:>4} "
                                    f"{qps_str:>12} {avg_lat_str:>14}\n")
            f.write("\n")

        if len(transports) >= 2:
            f.write("\n" + "=" * 100 + "\n")
            f.write("传输方式 QPS 对比（多轮平均）\n")
            f.write("=" * 100 + "\n\n")

            for ms in matrix_sizes:
                f.write(f"\n矩阵大小: {ms}x{ms}\n")
                f.write("-" * 100 + "\n")
                header = f"{'并发':>6}"
                for t in transports:
                    header += f" {t + '_QPS':>14}"
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
                                vals.append(results[key]['qps'])
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