use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use parking_lot::Mutex;
use prometheus::{
    register_counter, register_gauge, register_histogram, Counter, Gauge, Histogram,
};

/// Latency percentiles tracker
pub struct LatencyHistogram {
    buckets: Vec<AtomicU64>,
    bucket_bounds: Vec<f64>,
    total: AtomicU64,
    sum: AtomicU64,
}

impl LatencyHistogram {
    pub fn new() -> Self {
        // Exponential buckets from 1us to 10s
        let bucket_bounds: Vec<f64> = std::iter::successors(Some(1e-6), |&x| {
            if x < 10.0 {
                Some(x * 2.0)
            } else {
                None
            }
        })
        .collect();

        let buckets = bucket_bounds.iter().map(|_| AtomicU64::new(0)).collect();

        Self {
            buckets,
            bucket_bounds,
            total: AtomicU64::new(0),
            sum: AtomicU64::new(0),
        }
    }

    pub fn observe(&self, latency_secs: f64) {
        self.total.fetch_add(1, Ordering::Relaxed);
        self.sum.fetch_add((latency_secs * 1e9) as u64, Ordering::Relaxed);

        for (i, bound) in self.bucket_bounds.iter().enumerate() {
            if latency_secs <= *bound {
                self.buckets[i].fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
    }

    pub fn percentile(&self, p: f64) -> f64 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }

        let target = (total as f64 * p / 100.0) as u64;
        let mut cumulative = 0u64;

        for (i, bucket) in self.buckets.iter().enumerate() {
            let count = bucket.load(Ordering::Relaxed);
            cumulative += count;
            if cumulative >= target {
                return self.bucket_bounds[i];
            }
        }

        self.bucket_bounds.last().copied().unwrap_or(0.0)
    }

    pub fn p50(&self) -> f64 {
        self.percentile(50.0)
    }

    pub fn p90(&self) -> f64 {
        self.percentile(90.0)
    }

    pub fn p95(&self) -> f64 {
        self.percentile(95.0)
    }

    pub fn p99(&self) -> f64 {
        self.percentile(99.0)
    }

    pub fn p999(&self) -> f64 {
        self.percentile(99.9)
    }

    pub fn mean(&self) -> f64 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        let sum = self.sum.load(Ordering::Relaxed);
        (sum as f64 / total as f64) / 1e9
    }
}

/// Connection-level metrics
pub struct ConnectionMetrics {
    pub requests_total: AtomicU64,
    pub requests_active: AtomicU64,
    pub latency: LatencyHistogram,
    pub errors_total: AtomicU64,
    pub bytes_sent: AtomicU64,
    pub bytes_received: AtomicU64,
}

impl ConnectionMetrics {
    pub fn new() -> Self {
        Self {
            requests_total: AtomicU64::new(0),
            requests_active: AtomicU64::new(0),
            latency: LatencyHistogram::new(),
            errors_total: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
        }
    }
}

/// Global gateway metrics
pub struct Metrics {
    pub connections: DashMap<String, Arc<ConnectionMetrics>>,
    pub global_latency: LatencyHistogram,
    pub start_time: Instant,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            connections: DashMap::new(),
            global_latency: LatencyHistogram::new(),
            start_time: Instant::now(),
        }
    }

    pub fn get_or_create_connection(&self, id: &str) -> Arc<ConnectionMetrics> {
        self.connections
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(ConnectionMetrics::new()))
            .clone()
    }

    pub fn record_request(&self, latency_secs: f64) {
        self.global_latency.observe(latency_secs);
    }

    pub fn report(&self) -> MetricsReport {
        MetricsReport {
            uptime_secs: self.start_time.elapsed().as_secs(),
            total_connections: self.connections.len(),
            global_p50: self.global_latency.p50() * 1000.0, // Convert to ms
            global_p90: self.global_latency.p90() * 1000.0,
            global_p95: self.global_latency.p95() * 1000.0,
            global_p99: self.global_latency.p99() * 1000.0,
            global_p999: self.global_latency.p999() * 1000.0,
            global_mean: self.global_latency.mean() * 1000.0,
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct MetricsReport {
    pub uptime_secs: u64,
    pub total_connections: usize,
    pub global_p50: f64,
    pub global_p90: f64,
    pub global_p95: f64,
    pub global_p99: f64,
    pub global_p999: f64,
    pub global_mean: f64,
}

/// Prometheus-compatible metrics exporter
pub struct PrometheusExporter;

impl PrometheusExporter {
    pub fn export(metrics: &Metrics) -> String {
        let mut output = String::new();

        // Global latency percentiles
        output.push_str("# HELP gateway_latency_p50_ms P50 latency in milliseconds\n");
        output.push_str("# TYPE gateway_latency_p50_ms gauge\n");
        output.push_str(&format!(
            "gateway_latency_p50_ms {}\n",
            metrics.global_latency.p50() * 1000.0
        ));

        output.push_str("# HELP gateway_latency_p99_ms P99 latency in milliseconds\n");
        output.push_str("# TYPE gateway_latency_p99_ms gauge\n");
        output.push_str(&format!(
            "gateway_latency_p99_ms {}\n",
            metrics.global_latency.p99() * 1000.0
        ));

        output.push_str("# HELP gateway_connections_total Total number of connections\n");
        output.push_str("# TYPE gateway_connections_total gauge\n");
        output.push_str(&format!(
            "gateway_connections_total {}\n",
            metrics.connections.len()
        ));

        output
    }
}
