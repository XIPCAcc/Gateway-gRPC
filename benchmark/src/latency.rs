use hdrhistogram::Histogram;
use parking_lot::Mutex;

/// Thread-safe latency collector
pub struct LatencyCollector {
    histogram: Histogram<u64>,
}

impl LatencyCollector {
    pub fn new() -> Self {
        // Histogram with microsecond precision, max 1 hour
        let histogram = Histogram::new_with_bounds(1, 3_600_000_000, 3)
            .expect("Failed to create histogram");

        Self { histogram }
    }

    pub fn record(&mut self, latency_us: u64) {
        // Saturate at max value rather than failing
        let _ = self.histogram.saturating_record(latency_us);
    }

    pub fn histogram(&self) -> &Histogram<u64> {
        &self.histogram
    }

    pub fn merge(&mut self, other: &LatencyCollector) {
        let _ = self.histogram.add(&other.histogram);
    }
}

impl Default for LatencyCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Latency percentile calculator
pub struct LatencyAnalyzer {
    latencies: Vec<f64>,
}

impl LatencyAnalyzer {
    pub fn new() -> Self {
        Self {
            latencies: Vec::new(),
        }
    }

    pub fn add(&mut self, latency_ms: f64) {
        self.latencies.push(latency_ms);
    }

    pub fn percentile(&self, p: f64) -> f64 {
        if self.latencies.is_empty() {
            return 0.0;
        }

        let mut sorted = self.latencies.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let index = ((p / 100.0) * (sorted.len() - 1) as f64) as usize;
        sorted[index.min(sorted.len() - 1)]
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
        if self.latencies.is_empty() {
            return 0.0;
        }
        self.latencies.iter().sum::<f64>() / self.latencies.len() as f64
    }

    pub fn std_dev(&self) -> f64 {
        if self.latencies.len() < 2 {
            return 0.0;
        }
        let mean = self.mean();
        let variance = self.latencies
            .iter()
            .map(|&x| (x - mean).powi(2))
            .sum::<f64>()
            / (self.latencies.len() - 1) as f64;
        variance.sqrt()
    }

    pub fn min(&self) -> f64 {
        self.latencies.iter().cloned().fold(f64::INFINITY, f64::min)
    }

    pub fn max(&self) -> f64 {
        self.latencies.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
    }

    pub fn count(&self) -> usize {
        self.latencies.len()
    }

    /// Generate a latency distribution report
    pub fn report(&self) -> LatencyReport {
        LatencyReport {
            count: self.count(),
            min_ms: self.min(),
            mean_ms: self.mean(),
            p50_ms: self.p50(),
            p90_ms: self.p90(),
            p95_ms: self.p95(),
            p99_ms: self.p99(),
            p999_ms: self.p999(),
            max_ms: self.max(),
            std_dev_ms: self.std_dev(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LatencyReport {
    pub count: usize,
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

impl std::fmt::Display for LatencyReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Latency Report ({} samples):", self.count)?;
        writeln!(f, "  Min:    {:.3} ms", self.min_ms)?;
        writeln!(f, "  Mean:   {:.3} ms", self.mean_ms)?;
        writeln!(f, "  P50:    {:.3} ms", self.p50_ms)?;
        writeln!(f, "  P90:    {:.3} ms", self.p90_ms)?;
        writeln!(f, "  P95:    {:.3} ms", self.p95_ms)?;
        writeln!(f, "  P99:    {:.3} ms", self.p99_ms)?;
        writeln!(f, "  P99.9:  {:.3} ms", self.p999_ms)?;
        writeln!(f, "  Max:    {:.3} ms", self.max_ms)?;
        writeln!(f, "  StdDev: {:.3} ms", self.std_dev_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_latency_analyzer() {
        let mut analyzer = LatencyAnalyzer::new();
        
        for i in 1..=100 {
            analyzer.add(i as f64);
        }

        assert_eq!(analyzer.p50(), 50.0);
        assert_eq!(analyzer.p90(), 90.0);
        assert_eq!(analyzer.p99(), 99.0);
    }
}
