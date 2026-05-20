#!/usr/bin/env python3
"""
可视化 SHM 往返延迟分析结果
"""

import sys
import os
import csv
import matplotlib.pyplot as plt
import numpy as np
from datetime import datetime

def load_csv(filepath):
    """加载 CSV 数据"""
    data = []
    with open(filepath, 'r') as f:
        reader = csv.DictReader(f)
        for row in reader:
            data.append({
                'transport': row['Transport'],
                'matrix': int(row['Matrix']),
                'concurrency': int(row['Concurrency']),
                'qps': float(row['QPS']) if row['QPS'] else None,
                'avg_lat_ms': float(row['AvgLat_ms']) if row['AvgLat_ms'] else None,
                'p99_lat_ms': float(row['P99Lat_ms']) if row['P99Lat_ms'] else None,
                'shm_avg_us': float(row['SHM_Avg_us']) if row['SHM_Avg_us'] else None,
                'shm_min_us': float(row['SHM_Min_us']) if row['SHM_Min_us'] else None,
                'shm_max_us': float(row['SHM_Max_us']) if row['SHM_Max_us'] else None,
                'shm_std_us': float(row['SHM_Std_us']) if row['SHM_Std_us'] else None,
                'shm_p50_us': float(row['SHM_P50_us']) if row['SHM_P50_us'] else None,
                'shm_p99_us': float(row['SHM_P99_us']) if row['SHM_P99_us'] else None,
                'shm_count': int(row['SHM_Count']) if row['SHM_Count'] else None,
                'shm_pct_lat': float(row['SHM_Pct_Lat']) if row['SHM_Pct_Lat'] else None,
            })
    return data

def plot_qps_vs_matrix(data, output_dir):
    """QPS vs Matrix Size"""
    transports = sorted(set(d['transport'] for d in data))
    matrix_sizes = sorted(set(d['matrix'] for d in data))
    concurrencies = sorted(set(d['concurrency'] for d in data))
    
    for conc in [16, 64, 256, 1024]:
        plt.figure(figsize=(12, 6))
        
        for t in transports:
            qps_values = []
            for ms in matrix_sizes:
                found = [d for d in data if d['transport'] == t and d['matrix'] == ms and d['concurrency'] == conc]
                if found and found[0]['qps']:
                    qps_values.append(found[0]['qps'])
                else:
                    qps_values.append(None)
            
            plt.plot(matrix_sizes, qps_values, marker='o', label=t, linewidth=2, markersize=8)
        
        plt.title(f'QPS vs Matrix Size (Concurrency={conc})', fontsize=14)
        plt.xlabel('Matrix Size', fontsize=12)
        plt.ylabel('QPS (req/s)', fontsize=12)
        plt.xscale('log', base=2)
        plt.xticks(matrix_sizes, [f'{m}x{m}' for m in matrix_sizes])
        plt.legend()
        plt.grid(True, linestyle='--', alpha=0.7)
        plt.tight_layout()
        plt.savefig(os.path.join(output_dir, f'qps_vs_matrix_c{conc}.png'), dpi=150, bbox_inches='tight')
        plt.close()

def plot_latency_vs_matrix(data, output_dir):
    """Average Latency vs Matrix Size"""
    transports = sorted(set(d['transport'] for d in data))
    matrix_sizes = sorted(set(d['matrix'] for d in data))
    concurrencies = sorted(set(d['concurrency'] for d in data))
    
    for conc in [16, 64, 256, 1024]:
        plt.figure(figsize=(12, 6))
        
        for t in transports:
            lat_values = []
            for ms in matrix_sizes:
                found = [d for d in data if d['transport'] == t and d['matrix'] == ms and d['concurrency'] == conc]
                if found and found[0]['avg_lat_ms']:
                    lat_values.append(found[0]['avg_lat_ms'])
                else:
                    lat_values.append(None)
            
            plt.plot(matrix_sizes, lat_values, marker='s', label=t, linewidth=2, markersize=8)
        
        plt.title(f'Average Latency vs Matrix Size (Concurrency={conc})', fontsize=14)
        plt.xlabel('Matrix Size', fontsize=12)
        plt.ylabel('Average Latency (ms)', fontsize=12)
        plt.xscale('log', base=2)
        plt.xticks(matrix_sizes, [f'{m}x{m}' for m in matrix_sizes])
        plt.yscale('log')
        plt.legend()
        plt.grid(True, linestyle='--', alpha=0.7)
        plt.tight_layout()
        plt.savefig(os.path.join(output_dir, f'latency_vs_matrix_c{conc}.png'), dpi=150, bbox_inches='tight')
        plt.close()

def plot_shm_latency_vs_matrix(data, output_dir):
    """SHM Roundtrip Latency vs Matrix Size"""
    transports = sorted(set(d['transport'] for d in data))
    matrix_sizes = sorted(set(d['matrix'] for d in data))
    concurrencies = sorted(set(d['concurrency'] for d in data))
    
    for conc in [16, 64, 256, 1024]:
        plt.figure(figsize=(12, 6))
        
        for t in transports:
            shm_values = []
            for ms in matrix_sizes:
                found = [d for d in data if d['transport'] == t and d['matrix'] == ms and d['concurrency'] == conc]
                if found and found[0]['shm_avg_us']:
                    shm_values.append(found[0]['shm_avg_us'] / 1000)  # Convert to ms
                else:
                    shm_values.append(None)
            
            plt.plot(matrix_sizes, shm_values, marker='^', label=t, linewidth=2, markersize=8)
        
        plt.title(f'SHM Roundtrip Latency vs Matrix Size (Concurrency={conc})', fontsize=14)
        plt.xlabel('Matrix Size', fontsize=12)
        plt.ylabel('SHM Roundtrip Latency (ms)', fontsize=12)
        plt.xscale('log', base=2)
        plt.xticks(matrix_sizes, [f'{m}x{m}' for m in matrix_sizes])
        plt.yscale('log')
        plt.legend()
        plt.grid(True, linestyle='--', alpha=0.7)
        plt.tight_layout()
        plt.savefig(os.path.join(output_dir, f'shm_latency_vs_matrix_c{conc}.png'), dpi=150, bbox_inches='tight')
        plt.close()

def plot_shm_pct_vs_matrix(data, output_dir):
    """SHM Latency Percentage vs Matrix Size"""
    transports = sorted(set(d['transport'] for d in data))
    matrix_sizes = sorted(set(d['matrix'] for d in data))
    
    plt.figure(figsize=(12, 6))
    
    for t in transports:
        pct_values = []
        for ms in matrix_sizes:
            found = [d for d in data if d['transport'] == t and d['matrix'] == ms]
            if found and any(d['shm_pct_lat'] for d in found):
                valid_pcts = [d['shm_pct_lat'] for d in found if d['shm_pct_lat']]
                pct_values.append(sum(valid_pcts) / len(valid_pcts))
            else:
                pct_values.append(None)
        
        plt.plot(matrix_sizes, pct_values, marker='o', label=t, linewidth=2, markersize=8)
    
    plt.title('SHM Latency Percentage of Total Latency', fontsize=14)
    plt.xlabel('Matrix Size', fontsize=12)
    plt.ylabel('SHM Latency Percentage (%)', fontsize=12)
    plt.xscale('log', base=2)
    plt.xticks(matrix_sizes, [f'{m}x{m}' for m in matrix_sizes])
    plt.ylim(0, 100)
    plt.legend()
    plt.grid(True, linestyle='--', alpha=0.7)
    plt.tight_layout()
    plt.savefig(os.path.join(output_dir, 'shm_pct_vs_matrix.png'), dpi=150, bbox_inches='tight')
    plt.close()

def plot_transport_comparison(data, output_dir):
    """Transport Comparison Bar Charts"""
    transports = sorted(set(d['transport'] for d in data))
    matrix_sizes = sorted(set(d['matrix'] for d in data))
    
    for conc in [16, 128, 1024]:
        fig, axes = plt.subplots(1, 2, figsize=(16, 6))
        
        x = np.arange(len(matrix_sizes))
        width = 0.25
        
        # QPS Comparison
        for i, t in enumerate(transports):
            qps_values = []
            for ms in matrix_sizes:
                found = [d for d in data if d['transport'] == t and d['matrix'] == ms and d['concurrency'] == conc]
                if found and found[0]['qps']:
                    qps_values.append(found[0]['qps'] / 1000)
                else:
                    qps_values.append(0)
            axes[0].bar(x + i*width, qps_values, width, label=t)
        
        axes[0].set_title(f'QPS Comparison (Concurrency={conc})', fontsize=12)
        axes[0].set_xlabel('Matrix Size', fontsize=10)
        axes[0].set_ylabel('QPS (K req/s)', fontsize=10)
        axes[0].set_xticks(x + width)
        axes[0].set_xticklabels([f'{m}x{m}' for m in matrix_sizes])
        axes[0].legend()
        axes[0].grid(True, linestyle='--', alpha=0.7)
        
        # SHM Latency Comparison
        for i, t in enumerate(transports):
            shm_values = []
            for ms in matrix_sizes:
                found = [d for d in data if d['transport'] == t and d['matrix'] == ms and d['concurrency'] == conc]
                if found and found[0]['shm_avg_us']:
                    shm_values.append(found[0]['shm_avg_us'] / 1000)
                else:
                    shm_values.append(0)
            axes[1].bar(x + i*width, shm_values, width, label=t)
        
        axes[1].set_title(f'SHM Roundtrip Latency Comparison (Concurrency={conc})', fontsize=12)
        axes[1].set_xlabel('Matrix Size', fontsize=10)
        axes[1].set_ylabel('SHM Latency (ms)', fontsize=10)
        axes[1].set_xticks(x + width)
        axes[1].set_xticklabels([f'{m}x{m}' for m in matrix_sizes])
        axes[1].legend()
        axes[1].grid(True, linestyle='--', alpha=0.7)
        
        plt.tight_layout()
        plt.savefig(os.path.join(output_dir, f'transport_comparison_c{conc}.png'), dpi=150, bbox_inches='tight')
        plt.close()

def plot_latency_distribution(data, output_dir):
    """Latency Distribution Boxplots"""
    transports = sorted(set(d['transport'] for d in data))
    matrix_sizes = [2, 16, 64, 256]
    
    for ms in matrix_sizes:
        plt.figure(figsize=(12, 6))
        all_data = []
        
        for t in transports:
            values = []
            for d in data:
                if d['transport'] == t and d['matrix'] == ms and d['shm_avg_us']:
                    values.append(d['shm_avg_us'] / 1000)
            all_data.append(values)
        
        bp = plt.boxplot(all_data, labels=transports, patch_artist=True)
        
        colors = ['#1f77b4', '#ff7f0e', '#2ca02c']
        for patch, color in zip(bp['boxes'], colors):
            patch.set_facecolor(color)
        
        plt.title(f'SHM Latency Distribution (Matrix={ms}x{ms})', fontsize=14)
        plt.ylabel('SHM Roundtrip Latency (ms)', fontsize=12)
        plt.grid(True, linestyle='--', alpha=0.7)
        plt.tight_layout()
        plt.savefig(os.path.join(output_dir, f'latency_distribution_m{ms}.png'), dpi=150, bbox_inches='tight')
        plt.close()

def plot_p99_vs_p50(data, output_dir):
    """P99 vs P50 Latency Comparison"""
    transports = sorted(set(d['transport'] for d in data))
    matrix_sizes = sorted(set(d['matrix'] for d in data))
    
    plt.figure(figsize=(12, 6))
    
    for t in transports:
        p50_values = []
        p99_values = []
        for ms in matrix_sizes:
            found = [d for d in data if d['transport'] == t and d['matrix'] == ms]
            if found:
                valid_p50 = [d['shm_p50_us'] for d in found if d['shm_p50_us']]
                valid_p99 = [d['shm_p99_us'] for d in found if d['shm_p99_us']]
                if valid_p50:
                    p50_values.append(sum(valid_p50) / len(valid_p50) / 1000)
                    p99_values.append(sum(valid_p99) / len(valid_p99) / 1000)
                else:
                    p50_values.append(None)
                    p99_values.append(None)
            else:
                p50_values.append(None)
                p99_values.append(None)
        
        plt.plot(matrix_sizes, p50_values, marker='o', label=f'{t} P50', linewidth=2, markersize=6)
        plt.plot(matrix_sizes, p99_values, marker='s', label=f'{t} P99', linewidth=2, markersize=6)
    
    plt.title('SHM Latency Percentiles (P50 vs P99)', fontsize=14)
    plt.xlabel('Matrix Size', fontsize=12)
    plt.ylabel('Latency (ms)', fontsize=12)
    plt.xscale('log', base=2)
    plt.xticks(matrix_sizes, [f'{m}x{m}' for m in matrix_sizes])
    plt.yscale('log')
    plt.legend()
    plt.grid(True, linestyle='--', alpha=0.7)
    plt.tight_layout()
    plt.savefig(os.path.join(output_dir, 'p99_vs_p50.png'), dpi=150, bbox_inches='tight')
    plt.close()

def main():
    if len(sys.argv) < 2:
        print("Usage: python3 scripts/visualize_shm_results.py <csv_file>")
        print("Example: python3 scripts/visualize_shm_results.py log/shm_analysis_20260518_134245/shm_latency_results.csv")
        sys.exit(1)
    
    csv_file = sys.argv[1]
    
    if not os.path.exists(csv_file):
        print(f"Error: File {csv_file} not found")
        sys.exit(1)
    
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    output_dir = os.path.join(os.path.dirname(csv_file), f'plots_{timestamp}')
    os.makedirs(output_dir, exist_ok=True)
    
    print(f"Loading data from: {csv_file}")
    data = load_csv(csv_file)
    print(f"Loaded {len(data)} records")
    
    print("\nGenerating plots...")
    
    plot_qps_vs_matrix(data, output_dir)
    print("  - QPS vs Matrix Size")
    
    plot_latency_vs_matrix(data, output_dir)
    print("  - Average Latency vs Matrix Size")
    
    plot_shm_latency_vs_matrix(data, output_dir)
    print("  - SHM Roundtrip Latency vs Matrix Size")
    
    plot_shm_pct_vs_matrix(data, output_dir)
    print("  - SHM Latency Percentage vs Matrix Size")
    
    plot_transport_comparison(data, output_dir)
    print("  - Transport Comparison Bar Charts")
    
    plot_latency_distribution(data, output_dir)
    print("  - Latency Distribution Boxplots")
    
    plot_p99_vs_p50(data, output_dir)
    print("  - P99 vs P50 Comparison")
    
    print(f"\nAll plots saved to: {output_dir}")

if __name__ == '__main__':
    main()
