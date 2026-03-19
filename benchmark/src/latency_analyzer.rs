use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use serde::{Deserialize, Serialize};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    input: PathBuf,
    
    #[arg(short, long, default_value = "latency_report.json")]
    output: PathBuf,
    
    #[arg(long)]
    compare: Option<PathBuf>,
    
    #[arg(long)]
    plot: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BenchmarkResult {
    transport: String,
    connections: usize,
    target_qps: usize,
    actual_qps: f64,
    total_requests: u64,
    successful_requests: u64,
    failed_requests: u64,
    latencies: LatencyStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LatencyStats {
    min_ms: f64,
    mean_ms: f64,
    p50_ms: f64,
    p90_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    p999_ms: f64,
    max_ms: f64,
    std_dev_ms: f64,
}

#[derive(Debug, Serialize, Deserialize)]
struct ComparisonReport {
    baseline: Vec<BenchmarkResult>,
    comparison: Option<Vec<BenchmarkResult>>,
    summary: ComparisonSummary,
}

#[derive(Debug, Serialize, Deserialize)]
struct ComparisonSummary {
    improvements: HashMap<String, f64>,
    regressions: HashMap<String, f64>,
}

fn analyze_results(results: &[BenchmarkResult]) -> String {
    let mut report = String::new();
    
    report.push_str("\n=== Latency Analysis Report ===\n\n");
    
    // Group by transport type
    let mut by_transport: HashMap<&str, Vec<&BenchmarkResult>> = HashMap::new();
    for r in results {
        by_transport.entry(&r.transport).or_default().push(r);
    }
    
    for (transport, results) in by_transport {
        report.push_str(&format!("Transport: {}\n", transport));
        report.push_str("-".repeat(50).as_str());
        report.push('\n');
        
        // Sort by connections
        let mut sorted = results.clone();
        sorted.sort_by_key(|r| r.connections);
        
        for r in sorted {
            report.push_str(&format!(
                "Connections: {:4} | QPS: {:8.0} | P50: {:6.3}ms | P99: {:6.3}ms | P99.9: {:6.3}ms\n",
                r.connections,
                r.actual_qps,
                r.latencies.p50_ms,
                r.latencies.p99_ms,
                r.latencies.p999_ms
            ));
        }
        report.push('\n');
    }
    
    // Find best configuration
    if let Some(best) = results.iter().max_by(|a, b| {
        let a_score = a.actual_qps / (a.latencies.p99_ms + 1.0);
        let b_score = b.actual_qps / (b.latencies.p99_ms + 1.0);
        a_score.partial_cmp(&b_score).unwrap()
    }) {
        report.push_str("\n=== Best Configuration ===\n");
        report.push_str(&format!("Transport: {}\n", best.transport));
        report.push_str(&format!("Connections: {}\n", best.connections));
        report.push_str(&format!("QPS: {:.0}\n", best.actual_qps));
        report.push_str(&format!("P99 Latency: {:.3}ms\n", best.latencies.p99_ms));
    }
    
    report
}

fn compare_results(baseline: &[BenchmarkResult], comparison: &[BenchmarkResult]) -> ComparisonSummary {
    let mut improvements = HashMap::new();
    let mut regressions = HashMap::new();
    
    for b in baseline {
        for c in comparison {
            if b.transport == c.transport && b.connections == c.connections {
                let qps_change = (c.actual_qps - b.actual_qps) / b.actual_qps * 100.0;
                let p99_change = (b.latencies.p99_ms - c.latencies.p99_ms) / b.latencies.p99_ms * 100.0;
                
                let key = format!("{}-{}conn", b.transport, b.connections);
                
                if qps_change > 5.0 || p99_change > 5.0 {
                    improvements.insert(key.clone(), qps_change);
                } else if qps_change < -5.0 || p99_change < -5.0 {
                    regressions.insert(key, qps_change);
                }
            }
        }
    }
    
    ComparisonSummary {
        improvements,
        regressions,
    }
}

fn generate_csv(results: &[BenchmarkResult]) -> String {
    let mut csv = String::from("transport,connections,target_qps,actual_qps,total_requests,success_rate,min_ms,mean_ms,p50_ms,p90_ms,p95_ms,p99_ms,p999_ms,max_ms,std_dev_ms\n");
    
    for r in results {
        let success_rate = if r.total_requests > 0 {
            r.successful_requests as f64 / r.total_requests as f64 * 100.0
        } else {
            0.0
        };
        
        csv.push_str(&format!(
            "{},{},{},{},{},{:.2},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3}\n",
            r.transport,
            r.connections,
            r.target_qps,
            r.actual_qps,
            r.total_requests,
            success_rate,
            r.latencies.min_ms,
            r.latencies.mean_ms,
            r.latencies.p50_ms,
            r.latencies.p90_ms,
            r.latencies.p95_ms,
            r.latencies.p99_ms,
            r.latencies.p999_ms,
            r.latencies.max_ms,
            r.latencies.std_dev_ms
        ));
    }
    
    csv
}

fn main() -> Result<()> {
    let args = Args::parse();
    
    // Read input file
    let content = fs::read_to_string(&args.input)?;
    let results: Vec<BenchmarkResult> = serde_json::from_str(&content)?;
    
    // Generate analysis
    let report = analyze_results(&results);
    println!("{}", report);
    
    // Generate CSV
    let csv = generate_csv(&results);
    let csv_path = args.output.with_extension("csv");
    fs::write(&csv_path, csv)?;
    println!("CSV report saved to: {}", csv_path.display());
    
    // Compare if provided
    if let Some(compare_path) = args.compare {
        let compare_content = fs::read_to_string(&compare_path)?;
        let compare_data: Vec<BenchmarkResult> = serde_json::from_str(&compare_content)?;
        
        let summary = compare_results(&results, &compare_data);
        
        println!("\n=== Comparison Results ===");
        println!("\nImprovements:");
        for (key, value) in &summary.improvements {
            println!("  {}: {:.1}%", key, value);
        }
        
        println!("\nRegressions:");
        for (key, value) in &summary.regressions {
            println!("  {}: {:.1}%", key, value);
        }
        
        let report = ComparisonReport {
            baseline: results,
            comparison: Some(compare_data),
            summary,
        };
        
        let output = serde_json::to_string_pretty(&report)?;
        fs::write(&args.output, output)?;
    } else {
        // Just save the analysis
        let output = serde_json::to_string_pretty(&results)?;
        fs::write(&args.output, output)?;
    }
    
    println!("\nReport saved to: {}", args.output.display());
    
    Ok(())
}
