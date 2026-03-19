use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use futures::future::join_all;
use hdrhistogram::Histogram;
use parking_lot::Mutex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tracing::{info, warn};

mod latency;
mod metrics;

use latency::LatencyCollector;
use metrics::SystemMetricsCollector;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// HTTP load test using reqwest
    Http {
        #[arg(short, long, default_value = "http://127.0.0.1:8080")]
        target: String,
        #[arg(short, long, default_value_t = 64)]
        connections: usize,
        #[arg(short, long, default_value_t = 1000)]
        duration_secs: u64,
        #[arg(short, long, default_value_t = 10000)]
        qps: usize,
        #[arg(long)]
        payload_size: Option<usize>,
    },
    /// gRPC load test using tonic
    Grpc {
        #[arg(short, long, default_value = "http://127.0.0.1:50051")]
        target: String,
        #[arg(short, long, default_value_t = 64)]
        connections: usize,
        #[arg(short, long, default_value_t = 1000)]
        duration_secs: u64,
        #[arg(short, long, default_value_t = 10000)]
        qps: usize,
    },
    /// Full benchmark suite
    Suite {
        #[arg(short, long, default_value = "http://127.0.0.1:8080")]
        gateway_url: String,
        #[arg(short, long, default_value = "http://127.0.0.1:50051")]
        backend_addr: String,
        #[arg(long, default_value_t = 10000)]
        max_qps: usize,
        #[arg(long, default_value_t = 10)]
        target_p99_ms: u64,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum TransportType {
    Tcp,
    Uds,
    Shm,
}

/// Benchmark results
#[derive(Debug, Serialize, Deserialize)]
pub struct BenchmarkResult {
    pub transport: String,
    pub connections: usize,
    pub target_qps: usize,
    pub actual_qps: f64,
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub latencies: LatencyStats,
    pub system_metrics: Option<SystemMetrics>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LatencyStats {
    pub min_ms: f64,
    pub mean_ms: f64,
    pub p50_ms: f64,
    pub p90_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub p999_ms: f64,
    pub max_ms: f64,
    pub std_dev_ms: f64,
}

impl LatencyStats {
    pub fn from_histogram(hist: &Histogram<u64>) -> Self {
        Self {
            min_ms: hist.min() as f64 / 1000.0,
            mean_ms: hist.mean() / 1000.0,
            p50_ms: hist.value_at_percentile(50.0) as f64 / 1000.0,
            p90_ms: hist.value_at_percentile(90.0) as f64 / 1000.0,
            p95_ms: hist.value_at_percentile(95.0) as f64 / 1000.0,
            p99_ms: hist.value_at_percentile(99.0) as f64 / 1000.0,
            p999_ms: hist.value_at_percentile(99.9) as f64 / 1000.0,
            max_ms: hist.max() as f64 / 1000.0,
            std_dev_ms: hist.stdev() / 1000.0,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SystemMetrics {
    pub cpu_user_percent: f64,
    pub cpu_kernel_percent: f64,
    pub syscalls_per_sec: f64,
    pub ctx_switches_per_sec: f64,
}

/// HTTP benchmark runner
async fn run_http_benchmark(
    target: &str,
    connections: usize,
    duration_secs: u64,
    target_qps: usize,
    payload_size: Option<usize>,
) -> Result<BenchmarkResult> {
    info!(
        "Starting HTTP benchmark: {} connections, {}s duration, {} QPS target",
        connections, duration_secs, target_qps
    );

    let client = Client::builder()
        .pool_max_idle_per_host(connections)
        .timeout(Duration::from_secs(30))
        .build()?;

    let latency_collector = Arc::new(Mutex::new(LatencyCollector::new()));
    let total_requests = Arc::new(AtomicU64::new(0));
    let successful_requests = Arc::new(AtomicU64::new(0));
    let failed_requests = Arc::new(AtomicU64::new(0));

    let payload = payload_size.map(|size| "x".repeat(size));

    let semaphore = Arc::new(Semaphore::new(connections));
    let start = Instant::now();
    let duration = Duration::from_secs(duration_secs);

    let qps_per_connection = target_qps / connections;
    let interval = Duration::from_secs_f64(1.0 / qps_per_connection as f64);

    let mut handles = vec![];

    for _ in 0..connections {
        let client = client.clone();
        let target = target.to_string();
        let latency_collector = latency_collector.clone();
        let total_requests = total_requests.clone();
        let successful_requests = successful_requests.clone();
        let failed_requests = failed_requests.clone();
        let semaphore = semaphore.clone();
        let payload = payload.clone();

        let handle = tokio::spawn(async move {
            let mut interval_timer = tokio::time::interval(interval);
            
            while start.elapsed() < duration {
                let _permit = semaphore.acquire().await.unwrap();
                interval_timer.tick().await;

                let req_start = Instant::now();
                total_requests.fetch_add(1, Ordering::Relaxed);

                let request_builder = client.post(&target);
                let request = if let Some(ref p) = payload {
                    request_builder.body(p.clone())
                } else {
                    request_builder
                };

                match request.send().await {
                    Ok(response) => {
                        let latency_us = req_start.elapsed().as_micros() as u64;
                        latency_collector.lock().record(latency_us);

                        if response.status().is_success() {
                            successful_requests.fetch_add(1, Ordering::Relaxed);
                        } else {
                            failed_requests.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Err(e) => {
                        warn!("Request failed: {}", e);
                        failed_requests.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        });

        handles.push(handle);
    }

    join_all(handles).await;

    let elapsed = start.elapsed();
    let total = total_requests.load(Ordering::Relaxed);
    let successful = successful_requests.load(Ordering::Relaxed);
    let failed = failed_requests.load(Ordering::Relaxed);
    let actual_qps = total as f64 / elapsed.as_secs_f64();

    let latencies = {
        let collector = latency_collector.lock();
        LatencyStats::from_histogram(collector.histogram())
    };

    info!("HTTP benchmark completed:");
    info!("  Total requests: {}", total);
    info!("  Successful: {}", successful);
    info!("  Failed: {}", failed);
    info!("  Actual QPS: {:.2}", actual_qps);
    info!("  P50 latency: {:.3}ms", latencies.p50_ms);
    info!("  P99 latency: {:.3}ms", latencies.p99_ms);

    Ok(BenchmarkResult {
        transport: "http".to_string(),
        connections,
        target_qps,
        actual_qps,
        total_requests: total,
        successful_requests: successful,
        failed_requests: failed,
        latencies,
        system_metrics: None,
    })
}

/// gRPC benchmark runner
async fn run_grpc_benchmark(
    target: &str,
    connections: usize,
    duration_secs: u64,
    target_qps: usize,
) -> Result<BenchmarkResult> {
    info!(
        "Starting gRPC benchmark: {} connections, {}s duration, {} QPS target",
        connections, duration_secs, target_qps
    );

    use proto::echo_service_client::EchoServiceClient;
    use proto::EchoRequest;
    use tonic::transport::Endpoint;

    let latency_collector = Arc::new(Mutex::new(LatencyCollector::new()));
    let total_requests = Arc::new(AtomicU64::new(0));
    let successful_requests = Arc::new(AtomicU64::new(0));
    let failed_requests = Arc::new(AtomicU64::new(0));

    let endpoint = Endpoint::from_shared(target.to_string())?
        .timeout(Duration::from_secs(30));

    let semaphore = Arc::new(Semaphore::new(connections));
    let start = Instant::now();
    let duration = Duration::from_secs(duration_secs);

    let qps_per_connection = target_qps / connections;
    let interval = Duration::from_secs_f64(1.0 / qps_per_connection as f64);

    let mut handles = vec![];

    for _ in 0..connections {
        let endpoint = endpoint.clone();
        let latency_collector = latency_collector.clone();
        let total_requests = total_requests.clone();
        let successful_requests = successful_requests.clone();
        let failed_requests = failed_requests.clone();
        let semaphore = semaphore.clone();

        let handle = tokio::spawn(async move {
            let channel = endpoint.connect().await.unwrap();
            let mut client = EchoServiceClient::new(channel);
            let mut interval_timer = tokio::time::interval(interval);

            while start.elapsed() < duration {
                let _permit = semaphore.acquire().await.unwrap();
                interval_timer.tick().await;

                let req_start = Instant::now();
                total_requests.fetch_add(1, Ordering::Relaxed);

                let request = EchoRequest {
                    message: "benchmark".to_string(),
                    timestamp_ns: req_start.elapsed().as_nanos() as i64,
                    payload: vec![0u8; 100],
                };

                match client.echo(tonic::Request::new(request)).await {
                    Ok(_) => {
                        let latency_us = req_start.elapsed().as_micros() as u64;
                        latency_collector.lock().record(latency_us);
                        successful_requests.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => {
                        warn!("gRPC request failed: {}", e);
                        failed_requests.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        });

        handles.push(handle);
    }

    join_all(handles).await;

    let elapsed = start.elapsed();
    let total = total_requests.load(Ordering::Relaxed);
    let successful = successful_requests.load(Ordering::Relaxed);
    let failed = failed_requests.load(Ordering::Relaxed);
    let actual_qps = total as f64 / elapsed.as_secs_f64();

    let latencies = {
        let collector = latency_collector.lock();
        LatencyStats::from_histogram(collector.histogram())
    };

    info!("gRPC benchmark completed:");
    info!("  Total requests: {}", total);
    info!("  Successful: {}", successful);
    info!("  Failed: {}", failed);
    info!("  Actual QPS: {:.2}", actual_qps);
    info!("  P50 latency: {:.3}ms", latencies.p50_ms);
    info!("  P99 latency: {:.3}ms", latencies.p99_ms);

    Ok(BenchmarkResult {
        transport: "grpc".to_string(),
        connections,
        target_qps,
        actual_qps,
        total_requests: total,
        successful_requests: successful,
        failed_requests: failed,
        latencies,
        system_metrics: None,
    })
}

/// Run full benchmark suite
async fn run_benchmark_suite(
    gateway_url: &str,
    _backend_addr: &str,
    max_qps: usize,
    target_p99_ms: u64,
) -> Result<Vec<BenchmarkResult>> {
    let mut results = Vec::new();

    // Test different connection counts
    let connection_counts = vec![64, 256, 1024];

    for connections in connection_counts {
        info!("\n=== Testing with {} connections ===", connections);

        // Binary search for max QPS with P99 < target
        let mut low = 100usize;
        let mut high = max_qps;
        let mut best_result = None;

        while low <= high {
            let mid = (low + high) / 2;
            info!("  Testing QPS: {}", mid);

            let result = run_http_benchmark(
                gateway_url,
                connections,
                30, // 30 seconds per test
                mid,
                None,
            )
            .await?;

            if result.latencies.p99_ms <= target_p99_ms as f64 {
                best_result = Some(result);
                low = mid + 1;
            } else {
                if mid > 100 {
                    high = mid - 1;
                } else {
                    break;
                }
            }
        }

        if let Some(result) = best_result {
            results.push(result);
        }
    }

    // Save results
    let output = serde_json::to_string_pretty(&results)?;
    tokio::fs::write("benchmark_results.json", output).await?;

    info!("\n=== Benchmark Suite Complete ===");
    info!("Results saved to benchmark_results.json");

    Ok(results)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    match args.command {
        Commands::Http {
            target,
            connections,
            duration_secs,
            qps,
            payload_size,
        } => {
            let result = run_http_benchmark(&target, connections, duration_secs, qps, payload_size).await?;
            println!("\n{}", serde_json::to_string_pretty(&result)?);
        }
        Commands::Grpc {
            target,
            connections,
            duration_secs,
            qps,
        } => {
            let result = run_grpc_benchmark(&target, connections, duration_secs, qps).await?;
            println!("\n{}", serde_json::to_string_pretty(&result)?);
        }
        Commands::Suite {
            gateway_url,
            backend_addr,
            max_qps,
            target_p99_ms,
        } => {
            run_benchmark_suite(&gateway_url, &backend_addr, max_qps, target_p99_ms).await?;
        }
    }

    Ok(())
}
