#!/usr/bin/env python3
import matplotlib.pyplot as plt
import numpy as np

# 测试数据
concurrency = [64, 256, 1024]
tcp_qps = [4580.86, 5279.96, 5648.40]
uds_qps = [4847.84, 5414.95, 6145.74]

tcp_latency = [13.99, 48.50, 181.20]
uds_latency = [13.20, 47.27, 165.96]

tcp_stdev = [4.29, 16.26, 55.03]
uds_stdev = [3.79, 13.71, 48.51]

# 创建图表
fig, axes = plt.subplots(2, 2, figsize=(14, 10))
fig.suptitle('TCP vs UDS Performance Comparison', fontsize=16, fontweight='bold')

# 1. QPS对比
ax1 = axes[0, 0]
x = np.arange(len(concurrency))
width = 0.35

bars1 = ax1.bar(x - width/2, tcp_qps, width, label='TCP', color='#3498db', alpha=0.8)
bars2 = ax1.bar(x + width/2, uds_qps, width, label='UDS', color='#e74c3c', alpha=0.8)

ax1.set_xlabel('Concurrency', fontsize=12)
ax1.set_ylabel('QPS (Requests/sec)', fontsize=12)
ax1.set_title('Throughput Comparison', fontsize=14, fontweight='bold')
ax1.set_xticks(x)
ax1.set_xticklabels(concurrency)
ax1.legend()
ax1.grid(axis='y', alpha=0.3)

# 添加数值标签
for bars in [bars1, bars2]:
    for bar in bars:
        height = bar.get_height()
        ax1.annotate(f'{height:.0f}',
                    xy=(bar.get_x() + bar.get_width() / 2, height),
                    xytext=(0, 3),
                    textcoords="offset points",
                    ha='center', va='bottom', fontsize=9)

# 2. 延迟对比
ax2 = axes[0, 1]
bars3 = ax2.bar(x - width/2, tcp_latency, width, label='TCP', color='#3498db', alpha=0.8)
bars4 = ax2.bar(x + width/2, uds_latency, width, label='UDS', color='#e74c3c', alpha=0.8)

ax2.set_xlabel('Concurrency', fontsize=12)
ax2.set_ylabel('Average Latency (ms)', fontsize=12)
ax2.set_title('Latency Comparison', fontsize=14, fontweight='bold')
ax2.set_xticks(x)
ax2.set_xticklabels(concurrency)
ax2.legend()
ax2.grid(axis='y', alpha=0.3)

# 添加数值标签
for bars in [bars3, bars4]:
    for bar in bars:
        height = bar.get_height()
        ax2.annotate(f'{height:.1f}',
                    xy=(bar.get_x() + bar.get_width() / 2, height),
                    xytext=(0, 3),
                    textcoords="offset points",
                    ha='center', va='bottom', fontsize=9)

# 3. QPS提升百分比
ax3 = axes[1, 0]
qps_improvement = [(uds - tcp) / tcp * 100 for tcp, uds in zip(tcp_qps, uds_qps)]
bars5 = ax3.bar(concurrency, qps_improvement, color='#2ecc71', alpha=0.8)

ax3.set_xlabel('Concurrency', fontsize=12)
ax3.set_ylabel('QPS Improvement (%)', fontsize=12)
ax3.set_title('UDS QPS Improvement over TCP', fontsize=14, fontweight='bold')
ax3.grid(axis='y', alpha=0.3)
ax3.axhline(y=0, color='black', linestyle='-', linewidth=0.5)

# 添加数值标签
for bar in bars5:
    height = bar.get_height()
    ax3.annotate(f'{height:.1f}%',
                xy=(bar.get_x() + bar.get_width() / 2, height),
                xytext=(0, 3),
                textcoords="offset points",
                ha='center', va='bottom', fontsize=10, fontweight='bold')

# 4. 延迟降低百分比
ax4 = axes[1, 1]
latency_improvement = [(tcp - uds) / tcp * 100 for tcp, uds in zip(tcp_latency, uds_latency)]
bars6 = ax4.bar(concurrency, latency_improvement, color='#9b59b6', alpha=0.8)

ax4.set_xlabel('Concurrency', fontsize=12)
ax4.set_ylabel('Latency Reduction (%)', fontsize=12)
ax4.set_title('UDS Latency Reduction over TCP', fontsize=14, fontweight='bold')
ax4.grid(axis='y', alpha=0.3)
ax4.axhline(y=0, color='black', linestyle='-', linewidth=0.5)

# 添加数值标签
for bar in bars6:
    height = bar.get_height()
    ax4.annotate(f'{height:.1f}%',
                xy=(bar.get_x() + bar.get_width() / 2, height),
                xytext=(0, 3),
                textcoords="offset points",
                ha='center', va='bottom', fontsize=10, fontweight='bold')

plt.tight_layout()
plt.savefig('/home/zwp/gateway-gRPC/benchmark_results/tcp_vs_uds_comparison.png', dpi=300, bbox_inches='tight')
print("图表已保存到: benchmark_results/tcp_vs_uds_comparison.png")

# 创建性能对比表格
print("\n=== 性能对比表格 ===")
print("\nQPS对比:")
print(f"{'并发数':<10} {'TCP':<15} {'UDS':<15} {'提升':<10}")
print("-" * 50)
for i, c in enumerate(concurrency):
    improvement = (uds_qps[i] - tcp_qps[i]) / tcp_qps[i] * 100
    print(f"{c:<10} {tcp_qps[i]:<15.2f} {uds_qps[i]:<15.2f} {improvement:>6.1f}%")

print("\n延迟对比 (ms):")
print(f"{'并发数':<10} {'TCP':<15} {'UDS':<15} {'降低':<10}")
print("-" * 50)
for i, c in enumerate(concurrency):
    reduction = (tcp_latency[i] - uds_latency[i]) / tcp_latency[i] * 100
    print(f"{c:<10} {tcp_latency[i]:<15.2f} {uds_latency[i]:<15.2f} {reduction:>6.1f}%")

print("\n延迟标准差对比 (ms):")
print(f"{'并发数':<10} {'TCP':<15} {'UDS':<15} {'降低':<10}")
print("-" * 50)
for i, c in enumerate(concurrency):
    reduction = (tcp_stdev[i] - uds_stdev[i]) / tcp_stdev[i] * 100
    print(f"{c:<10} {tcp_stdev[i]:<15.2f} {uds_stdev[i]:<15.2f} {reduction:>6.1f}%")
