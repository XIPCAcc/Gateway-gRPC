use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::interval;
use tracing::{debug, error, info};

/// System metrics collector using perf and pidstat
pub struct SystemMetricsCollector;

impl SystemMetricsCollector {
    /// Collect metrics using perf stat
    pub async fn collect_perf_stats(
        pid: u32,
        duration_secs: u64,
    ) -> Result<PerfMetrics> {
        info!("Collecting perf stats for PID {} for {}s", pid, duration_secs);

        let output = Command::new("perf")
            .args(&[
                "stat",
                "-p",
                &pid.to_string(),
                "-e",
                "syscalls:sys_enter,syscalls:sys_exit,context-switches,cpu-clock",
                "--",
                "sleep",
                &duration_secs.to_string(),
            ])
            .output()
            .await?;

        let stderr = String::from_utf8_lossy(&output.stderr);
        debug!("perf stat output:\n{}", stderr);

        // Parse perf output
        let metrics = Self::parse_perf_output(&stderr)?;
        Ok(metrics)
    }

    /// Collect metrics using pidstat
    pub async fn collect_pidstat(
        pid: u32,
        duration_secs: u64,
    ) -> Result<PidstatMetrics> {
        info!("Collecting pidstat for PID {} for {}s", pid, duration_secs);

        let mut child = Command::new("pidstat")
            .args(&[
                "-u", // CPU usage
                "-s", // Stack usage
                "-w", // Task switching
                "-p",
                &pid.to_string(),
                "1", // 1 second interval
                &duration_secs.to_string(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stdout = child.stdout.take().unwrap();
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        let mut cpu_user_samples = Vec::new();
        let mut cpu_system_samples = Vec::new();
        let mut ctx_switch_samples = Vec::new();

        while let Some(line) = lines.next_line().await? {
            if let Some(metrics) = Self::parse_pidstat_line(&line) {
                cpu_user_samples.push(metrics.cpu_user_percent);
                cpu_system_samples.push(metrics.cpu_system_percent);
                ctx_switch_samples.push(metrics.context_switches_per_sec);
            }
        }

        let status = child.wait().await?;
        if !status.success() {
            error!("pidstat exited with error");
        }

        Ok(PidstatMetrics {
            avg_cpu_user_percent: Self::average(&cpu_user_samples),
            avg_cpu_system_percent: Self::average(&cpu_system_samples),
            avg_context_switches_per_sec: Self::average(&ctx_switch_samples),
            max_cpu_user_percent: cpu_user_samples.iter().cloned().fold(0.0, f64::max),
            max_cpu_system_percent: cpu_system_samples.iter().cloned().fold(0.0, f64::max),
        })
    }

    fn parse_perf_output(output: &str) -> Result<PerfMetrics> {
        let mut syscalls = 0u64;
        let mut ctx_switches = 0u64;
        let mut cpu_clock = 0.0;

        for line in output.lines() {
            if line.contains("syscalls:sys_enter") {
                if let Some(val) = Self::extract_number(line) {
                    syscalls = val;
                }
            } else if line.contains("context-switches") {
                if let Some(val) = Self::extract_number(line) {
                    ctx_switches = val;
                }
            } else if line.contains("cpu-clock") {
                if let Some(val) = Self::extract_float(line) {
                    cpu_clock = val;
                }
            }
        }

        Ok(PerfMetrics {
            syscalls,
            context_switches: ctx_switches,
            cpu_clock_ms: cpu_clock,
        })
    }

    fn parse_pidstat_line(line: &str) -> Option<PidstatSample> {
        // pidstat output format:
        #![allow(dead_code)]
        // Average:   UID   PID  %usr %system  %guest   %wait    %CPU   CPU  Command
        let parts: Vec<&str> = line.split_whitespace().collect();
        
        if parts.len() < 8 || parts[0] != "Average:" {
            return None;
        }

        let cpu_user = parts.get(3)?.parse::<f64>().ok()?;
        let cpu_system = parts.get(4)?.parse::<f64>().ok()?;

        Some(PidstatSample {
            cpu_user_percent: cpu_user,
            cpu_system_percent: cpu_system,
            context_switches_per_sec: 0.0, // Would need -w flag parsing
        })
    }

    fn extract_number(line: &str) -> Option<u64> {
        line.split_whitespace()
            .next()
            .and_then(|s| s.replace(",", "").parse::<u64>().ok())
    }

    fn extract_float(line: &str) -> Option<f64> {
        line.split_whitespace()
            .next()
            .and_then(|s| s.parse::<f64>().ok())
    }

    fn average(samples: &[f64]) -> f64 {
        if samples.is_empty() {
            0.0
        } else {
            samples.iter().sum::<f64>() / samples.len() as f64
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerfMetrics {
    pub syscalls: u64,
    pub context_switches: u64,
    pub cpu_clock_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PidstatMetrics {
    pub avg_cpu_user_percent: f64,
    pub avg_cpu_system_percent: f64,
    pub avg_context_switches_per_sec: f64,
    pub max_cpu_user_percent: f64,
    pub max_cpu_system_percent: f64,
}

#[derive(Debug, Clone)]
struct PidstatSample {
    cpu_user_percent: f64,
    cpu_system_percent: f64,
    context_switches_per_sec: f64,
}

/// Real-time metrics monitor
pub struct MetricsMonitor {
    pid: u32,
}

impl MetricsMonitor {
    pub fn new(pid: u32) -> Self {
        Self { pid }
    }

    pub async fn start_monitoring(&self, interval_secs: u64) {
        let mut interval = interval(Duration::from_secs(interval_secs));

        loop {
            interval.tick().await;

            match Self::read_proc_stat(self.pid).await {
                Ok(stats) => {
                    debug!("Process stats: {:?}", stats);
                }
                Err(e) => {
                    error!("Failed to read process stats: {}", e);
                }
            }
        }
    }

    async fn read_proc_stat(pid: u32) -> Result<ProcessStats> {
        let path = format!("/proc/{}/stat", pid);
        let content = tokio::fs::read_to_string(&path).await?;
        
        // Parse /proc/PID/stat
        // Format: pid (comm) state ppid pgrp session tty_nr tpgid flags minflt cminflt majflt cmajflt utime stime cutime cstime ...
        let parts: Vec<&str> = content.split_whitespace().collect();
        
        if parts.len() < 14 {
            return Err(anyhow::anyhow!("Invalid /proc/stat format"));
        }

        let utime: u64 = parts[13].parse()?;
        let stime: u64 = parts[14].parse()?;

        Ok(ProcessStats {
            user_time_ticks: utime,
            system_time_ticks: stime,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessStats {
    pub user_time_ticks: u64,
    pub system_time_ticks: u64,
}

/// Combined system metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMetricsReport {
    pub timestamp: String,
    pub pid: u32,
    pub perf: Option<PerfMetrics>,
    pub pidstat: Option<PidstatMetrics>,
    pub process_stats: Option<ProcessStats>,
}

/// Export metrics to file
pub async fn export_metrics(metrics: &[SystemMetricsReport], path: &str) -> Result<()> {
    let json = serde_json::to_string_pretty(metrics)?;
    tokio::fs::write(path, json).await?;
    Ok(())
}
