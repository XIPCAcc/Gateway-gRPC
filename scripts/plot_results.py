#!/usr/bin/env python3
import matplotlib.pyplot as plt
import numpy as np

# Test results
transports = ['TCP', 'UDS', 'SHM']
qps = [18210.83, 19142.38, 476.37]
avg_latency = [3.50, 3.32, 145.77]
p99_latency = [5.87, 5.37, 526.35]

# Create figure with subplots
fig, axes = plt.subplots(1, 3, figsize=(15, 5))

# QPS comparison
colors = ['#3498db', '#2ecc71', '#e74c3c']
bars1 = axes[0].bar(transports, qps, color=colors, alpha=0.8)
axes[0].set_ylabel('QPS (requests/second)', fontsize=12)
axes[0].set_title('Throughput Comparison', fontsize=14, fontweight='bold')
axes[0].set_ylim(0, max(qps) * 1.1)
for bar, value in zip(bars1, qps):
    height = bar.get_height()
    axes[0].text(bar.get_x() + bar.get_width()/2., height,
                f'{value:,.0f}',
                ha='center', va='bottom', fontsize=11, fontweight='bold')

# Average latency comparison
bars2 = axes[1].bar(transports, avg_latency, color=colors, alpha=0.8)
axes[1].set_ylabel('Latency (ms)', fontsize=12)
axes[1].set_title('Average Latency Comparison', fontsize=14, fontweight='bold')
axes[1].set_ylim(0, max(avg_latency) * 1.1)
for bar, value in zip(bars2, avg_latency):
    height = bar.get_height()
    axes[1].text(bar.get_x() + bar.get_width()/2., height,
                f'{value:.2f}ms',
                ha='center', va='bottom', fontsize=11, fontweight='bold')

# P99 latency comparison
bars3 = axes[2].bar(transports, p99_latency, color=colors, alpha=0.8)
axes[2].set_ylabel('Latency (ms)', fontsize=12)
axes[2].set_title('P99 Latency Comparison', fontsize=14, fontweight='bold')
axes[2].set_ylim(0, max(p99_latency) * 1.1)
for bar, value in zip(bars3, p99_latency):
    height = bar.get_height()
    axes[2].text(bar.get_x() + bar.get_width()/2., height,
                f'{value:.2f}ms',
                ha='center', va='bottom', fontsize=11, fontweight='bold')

# Add grid
for ax in axes:
    ax.grid(axis='y', alpha=0.3, linestyle='--')
    ax.set_xlabel('Transport Type', fontsize=12)

plt.tight_layout()
plt.savefig('benchmark_results/performance_comparison.png', dpi=300, bbox_inches='tight')
print("Performance comparison chart saved to benchmark_results/performance_comparison.png")

# Create a summary table
fig, ax = plt.subplots(figsize=(10, 4))
ax.axis('tight')
ax.axis('off')

table_data = [
    ['Transport', 'QPS', 'Avg Latency', 'P99 Latency', 'Relative Performance'],
    ['TCP', '18,210.83', '3.50ms', '5.87ms', '95.1%'],
    ['UDS', '19,142.38', '3.32ms', '5.37ms', '100% (Best)'],
    ['SHM', '476.37', '145.77ms', '526.35ms', '2.5%'],
]

table = ax.table(cellText=table_data, cellLoc='center', loc='center',
                colWidths=[0.15, 0.2, 0.2, 0.2, 0.25])
table.auto_set_font_size(False)
table.set_fontsize(11)
table.scale(1.2, 2)

# Style header row
for i in range(5):
    table[(0, i)].set_facecolor('#3498db')
    table[(0, i)].set_text_props(weight='bold', color='white')

# Style data rows
for i in range(1, 4):
    for j in range(5):
        if i == 2:  # UDS row (best performance)
            table[(i, j)].set_facecolor('#d5f4e6')
        elif i == 3:  # SHM row (worst performance)
            table[(i, j)].set_facecolor('#ffe6e6')
        else:  # TCP row
            table[(i, j)].set_facecolor('#f0f0f0')

plt.title('Performance Summary Table', fontsize=16, fontweight='bold', pad=20)
plt.savefig('benchmark_results/performance_table.png', dpi=300, bbox_inches='tight')
print("Performance table saved to benchmark_results/performance_table.png")

print("\n=== Performance Summary ===")
print(f"Best Transport: UDS (19,142 QPS)")
print(f"TCP vs UDS: -5.1% QPS")
print(f"SHM vs UDS: -97.5% QPS (needs redesign)")
