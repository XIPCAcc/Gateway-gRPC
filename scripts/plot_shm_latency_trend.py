#!/usr/bin/env python3
"""
从 gateway 日志中提取前 N 个 SHM 延迟数据，对比三种传输方式的延迟趋势。
用法:
    python3 scripts/plot_shm_latency_trend.py <log_dir> <matrix_size> <concurrency> [--first N]
    python3 scripts/plot_shm_latency_trend.py log/matrix_20260521-220745 2 32 --first 1000
"""

import sys
import os
import re
import glob
import argparse
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
import numpy as np


def extract_latencies(filepath, first_n=None):
    if not os.path.exists(filepath):
        return None

    latencies = []
    with open(filepath) as f:
        for line in f:
            m = re.search(r'SHM:\s*(\d+)us', line)
            if m:
                latencies.append(int(m.group(1)))
                if first_n and len(latencies) >= first_n:
                    break
    return latencies if latencies else None


def main():
    parser = argparse.ArgumentParser(description='Plot SHM latency trend across transports')
    parser.add_argument('log_dir', help='Log directory path')
    parser.add_argument('matrix_size', type=int, help='Matrix size')
    parser.add_argument('concurrency', type=int, help='Concurrency')
    parser.add_argument('--first', type=int, default=1000, help='Number of samples to plot (default: 1000)')
    args = parser.parse_args()

    log_dir = args.log_dir
    ms = args.matrix_size
    cc = args.concurrency
    first_n = args.first

    transports = ['shm-uintr', 'shm-eventfd', 'shm-uds']
    colors = {'shm-uintr': '#e74c3c', 'shm-eventfd': '#3498db', 'shm-uds': '#2ecc71'}
    labels = {'shm-uintr': 'UINTR', 'shm-eventfd': 'eventfd (epoll)', 'shm-uds': 'UDS'}

    data = {}
    for t in transports:
        pattern = os.path.join(log_dir, f'{t}_m{ms}_c{cc}_r*_gateway.log')
        all_lats = []
        for fpath in sorted(glob.glob(pattern)):
            lats = extract_latencies(fpath, first_n)
            if lats:
                all_lats.extend(lats)
        if all_lats:
            data[t] = all_lats
            print(f'{t}: {len(all_lats)} samples (from {len(glob.glob(pattern))} run(s)), '
                  f'avg={sum(all_lats)/len(all_lats):.1f}us, '
                  f'min={min(all_lats)}us, max={max(all_lats)}us')
        else:
            print(f'{t}: NO DATA')

    if not data:
        print('Error: No data found!')
        sys.exit(1)

    fig, axes = plt.subplots(2, 2, figsize=(18, 12))
    fig.suptitle(f'SHM Roundtrip Latency: {ms}×{ms} Matrix, {cc} Connections\n'
                 f'(First {first_n} Requests Per Transport)',
                 fontsize=15, fontweight='bold')

    # 1. 主图：延迟趋势
    ax1 = axes[0][0]
    for t in transports:
        if t in data:
            x = np.arange(1, min(len(data[t]), first_n) + 1)
            y = data[t][:first_n]
            ax1.plot(x, y, color=colors[t], label=labels[t], linewidth=0.6, alpha=0.85)

    ax1.set_xlabel('Request Sequence', fontsize=11)
    ax1.set_ylabel('SHM Roundtrip Latency (us)', fontsize=11)
    ax1.set_title('Latency Trend (Line)', fontsize=13)
    ax1.legend(fontsize=10, loc='upper right')
    ax1.grid(True, alpha=0.3)

    # 2. 滑动平均对比
    ax2 = axes[0][1]
    window = max(10, first_n // 50)
    for t in transports:
        if t in data:
            y = data[t][:first_n]
            if len(y) >= window:
                smoothed = np.convolve(y, np.ones(window)/window, mode='valid')
                ax2.plot(np.arange(window, len(y)+1), smoothed,
                         color=colors[t], label=f'{labels[t]} (win={window})', linewidth=1.2)

    ax2.set_xlabel('Request Sequence', fontsize=11)
    ax2.set_ylabel('Smoothed SHM Latency (us)', fontsize=11)
    ax2.set_title(f'Latency Trend (Moving Average, window={window})', fontsize=13)
    ax2.legend(fontsize=10, loc='upper right')
    ax2.grid(True, alpha=0.3)

    # 3. 箱线图
    ax3 = axes[1][0]
    box_data = []
    box_labels = []
    box_colors = []
    for t in transports:
        if t in data:
            box_data.append(data[t][:first_n])
            box_labels.append(labels[t])
            box_colors.append(colors[t])

    bp = ax3.boxplot(box_data, labels=box_labels, patch_artist=True, widths=0.5)
    for patch, color in zip(bp['boxes'], box_colors):
        patch.set_facecolor(color)
        patch.set_alpha(0.5)
    ax3.set_ylabel('SHM Roundtrip Latency (us)', fontsize=11)
    ax3.set_title('Latency Distribution (Boxplot)', fontsize=13)
    ax3.grid(True, alpha=0.3, axis='y')

    for i, vals in enumerate(box_data):
        avg_v = sum(vals) / len(vals)
        ax3.text(i + 1, max(vals) * 1.02, f'avg={avg_v:.0f}us',
                 ha='center', fontsize=9, fontweight='bold')

    # 4. CDF 累积分布
    ax4 = axes[1][1]
    for t in transports:
        if t in data:
            sorted_vals = sorted(data[t][:first_n])
            cdf_y = np.arange(1, len(sorted_vals) + 1) / len(sorted_vals)
            ax4.plot(sorted_vals, cdf_y, color=colors[t], label=labels[t], linewidth=1.5)

    ax4.set_xlabel('SHM Roundtrip Latency (us)', fontsize=11)
    ax4.set_ylabel('Cumulative Probability', fontsize=11)
    ax4.set_title('Cumulative Distribution (CDF)', fontsize=13)
    ax4.legend(fontsize=10, loc='lower right')
    ax4.grid(True, alpha=0.3)

    plt.tight_layout()

    output_path = os.path.join(log_dir, f'shm_latency_trend_m{ms}_c{cc}.png')
    plt.savefig(output_path, dpi=150, bbox_inches='tight')
    print(f'\nChart saved to: {output_path}')

    # 统计对比表
    print()
    print(f"{'Transport':>14} {'Samples':>8} {'Avg(us)':>10} {'Min':>8} {'Max':>8} "
          f"{'P50':>8} {'P90':>8} {'P95':>8} {'P99':>8} {'Std':>8}")
    print('-' * 90)
    for t in transports:
        if t in data:
            vals = sorted(data[t][:first_n])
            n = len(vals)
            avg = sum(vals) / n
            std = (sum((x - avg) ** 2 for x in vals) / n) ** 0.5
            print(f'{labels[t]:>14} {n:>8} {avg:>10.1f} {vals[0]:>8} {vals[-1]:>8} '
                  f'{vals[int(n*0.50)]:>8} {vals[int(n*0.90)]:>8} '
                  f'{vals[int(n*0.95)]:>8} {vals[int(n*0.99)]:>8} {std:>8.1f}')

    if 'shm-uintr' in data and 'shm-eventfd' in data:
        uintr_avg = sum(data['shm-uintr'][:first_n]) / len(data['shm-uintr'][:first_n])
        efd_avg = sum(data['shm-eventfd'][:first_n]) / len(data['shm-eventfd'][:first_n])
        print(f'\nUINTR vs eventfd: {uintr_avg/efd_avg:.2f}x '
              f'({"faster" if uintr_avg < efd_avg else "slower"}) '
              f'(UINTR={uintr_avg:.1f}us, eventfd={efd_avg:.1f}us)')


if __name__ == '__main__':
    main()