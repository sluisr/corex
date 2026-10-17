use std::time::Instant;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::NodeConfig;
use crate::ssh::SshExecutor;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeTelemetry {
    pub node_id: String,
    pub is_online: bool,
    pub latency_ms: Option<u64>,
    pub hostname: Option<String>,
    pub os_name: Option<String>,
    pub uptime_str: Option<String>,
    pub load_avg: Option<String>,
    #[serde(default)]
    pub cpu_cores: Option<u32>,
    pub memory_used_mb: Option<u64>,
    pub memory_total_mb: Option<u64>,
    pub disk_usage_pct: Option<u8>,
    pub last_checked: DateTime<Utc>,
    pub error_message: Option<String>,
}

impl NodeTelemetry {
    pub fn offline(node_id: impl Into<String>, err: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            is_online: false,
            latency_ms: None,
            hostname: None,
            os_name: None,
            uptime_str: None,
            load_avg: None,
            cpu_cores: None,
            memory_used_mb: None,
            memory_total_mb: None,
            disk_usage_pct: None,
            last_checked: Utc::now(),
            error_message: Some(err.into()),
        }
    }

    pub fn memory_percent(&self) -> Option<u8> {
        let used = self.memory_used_mb? as f64;
        let total = self.memory_total_mb? as f64;
        if total > 0.0 {
            Some(((used / total) * 100.0).clamp(0.0, 100.0) as u8)
        } else {
            None
        }
    }
}

pub struct TelemetryCollector;

impl TelemetryCollector {
    /// Probe script executed on the remote VPS to collect all metrics in a single round-trip.
    pub const PROBE_SCRIPT: &'static str = "export LC_ALL=C LANG=C; echo '===CX_HOST==='; (hostname 2>/dev/null || uname -n 2>/dev/null || true); echo '===CX_UNAME==='; (uname -sr 2>/dev/null || true); echo '===CX_UPTIME==='; (uptime -p 2>/dev/null || uptime 2>/dev/null || true); echo '===CX_LOAD==='; (cat /proc/loadavg 2>/dev/null || sysctl -n vm.loadavg 2>/dev/null || true); echo '===CX_CORES==='; (nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || true); echo '===CX_FREE==='; (free -m 2>/dev/null || true); echo '===CX_DF==='; (df -P / 2>/dev/null | tail -n 1 || df -h / 2>/dev/null | tail -n 1 || true); echo '===CX_END==='";

    /// Fetches telemetry from a remote node and parses the metrics.
    pub async fn fetch(node: &NodeConfig, timeout_secs: u64) -> NodeTelemetry {
        let start = Instant::now();
        let res = SshExecutor::execute(node, Self::PROBE_SCRIPT, false, timeout_secs).await;

        match res {
            Ok(output) if output.success => {
                let latency_ms = start.elapsed().as_millis() as u64;
                Self::parse_probe_output(&node.id, latency_ms, &output.stdout)
            }
            Ok(output) => {
                NodeTelemetry::offline(&node.id, format!("Exit code {}: {}", output.exit_code, output.stderr.trim()))
            }
            Err(e) => NodeTelemetry::offline(&node.id, e.to_string()),
        }
    }

    pub fn parse_probe_output(node_id: &str, latency_ms: u64, stdout: &str) -> NodeTelemetry {
        let mut telemetry = NodeTelemetry {
            node_id: node_id.to_string(),
            is_online: true,
            latency_ms: Some(latency_ms),
            hostname: None,
            os_name: None,
            uptime_str: None,
            load_avg: None,
            cpu_cores: None,
            memory_used_mb: None,
            memory_total_mb: None,
            disk_usage_pct: None,
            last_checked: Utc::now(),
            error_message: None,
        };

        if stdout.contains("===CX_") {
            let mut section = "";
            for raw_line in stdout.lines() {
                let line = raw_line.trim();
                match line {
                    "===CX_HOST===" => {
                        section = "HOST";
                        continue;
                    }
                    "===CX_UNAME===" => {
                        section = "UNAME";
                        continue;
                    }
                    "===CX_UPTIME===" => {
                        section = "UPTIME";
                        continue;
                    }
                    "===CX_LOAD===" => {
                        section = "LOAD";
                        continue;
                    }
                    "===CX_CORES===" => {
                        section = "CORES";
                        continue;
                    }
                    "===CX_FREE===" => {
                        section = "FREE";
                        continue;
                    }
                    "===CX_DF===" => {
                        section = "DF";
                        continue;
                    }
                    "===CX_END===" => break,
                    _ => {}
                }

                if line.is_empty() {
                    continue;
                }

                match section {
                    "HOST" => {
                        if telemetry.hostname.is_none() {
                            telemetry.hostname = Some(line.to_string());
                        }
                    }
                    "UNAME" => {
                        if telemetry.os_name.is_none() {
                            telemetry.os_name = Some(line.to_string());
                        }
                    }
                    "UPTIME" => {
                        if telemetry.uptime_str.is_none() {
                            let clean_up = line.trim().trim_start_matches("up ").trim();
                            if !clean_up.is_empty() {
                                telemetry.uptime_str = Some(clean_up.to_string());
                            }
                        }
                    }
                    "LOAD" => {
                        if telemetry.load_avg.is_none() {
                            let clean_load = line.trim_matches(|c| c == '{' || c == '}').trim();
                            let parts: Vec<&str> = clean_load.split_whitespace().collect();
                            if parts.len() >= 3 {
                                let p0 = parts[0].trim_end_matches(',');
                                let p1 = parts[1].trim_end_matches(',');
                                let p2 = parts[2].trim_end_matches(',');
                                telemetry.load_avg = Some(format!("{}, {}, {}", p0, p1, p2));
                            }
                        }
                    }
                    "CORES" => {
                        if telemetry.cpu_cores.is_none() {
                            if let Ok(cores) = line.parse::<u32>() {
                                if cores > 0 {
                                    telemetry.cpu_cores = Some(cores);
                                }
                            }
                        }
                    }
                    "FREE" => {
                        if line.starts_with("Mem:") {
                            let parts: Vec<&str> = line.split_whitespace().collect();
                            if parts.len() >= 3 {
                                telemetry.memory_total_mb = parts[1].parse().ok();
                                telemetry.memory_used_mb = parts[2].parse().ok();
                            }
                        }
                    }
                    "DF" => {
                        if telemetry.disk_usage_pct.is_none() && !line.starts_with("Filesystem") {
                            for token in line.split_whitespace() {
                                if let Some(pct_str) = token.strip_suffix('%') {
                                    if let Ok(pct) = pct_str.parse::<u8>() {
                                        telemetry.disk_usage_pct = Some(pct);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }

            return telemetry;
        }

        // Legacy fallback for `---CX_TELEMETRY---`
        let lines: Vec<&str> = stdout.lines().collect();
        let mut idx = 0;

        while idx < lines.len() && !lines[idx].contains("---CX_TELEMETRY---") {
            idx += 1;
        }
        idx += 1;

        if idx < lines.len() {
            let h = lines[idx].trim();
            if !h.is_empty() {
                telemetry.hostname = Some(h.to_string());
            }
            idx += 1;
        }
        if idx < lines.len() {
            let os = lines[idx].trim();
            if !os.is_empty() {
                telemetry.os_name = Some(os.to_string());
            }
            idx += 1;
        }
        if idx < lines.len() {
            let clean_up = lines[idx].trim().trim_start_matches("up ").trim();
            if !clean_up.is_empty() {
                telemetry.uptime_str = Some(clean_up.to_string());
            }
            idx += 1;
        }
        if idx < lines.len() {
            let load_line = lines[idx].trim();
            let parts: Vec<&str> = load_line.split_whitespace().collect();
            if parts.len() >= 3 {
                telemetry.load_avg = Some(format!("{}, {}, {}", parts[0], parts[1], parts[2]));
            }
            idx += 1;
        }

        while idx < lines.len() {
            let l = lines[idx].trim();
            if l.starts_with("Mem:") {
                let parts: Vec<&str> = l.split_whitespace().collect();
                if parts.len() >= 3 {
                    telemetry.memory_total_mb = parts[1].parse().ok();
                    telemetry.memory_used_mb = parts[2].parse().ok();
                }
            } else if !l.starts_with("Filesystem") && !l.starts_with("---") {
                for token in l.split_whitespace() {
                    if let Some(pct_str) = token.strip_suffix('%') {
                        if let Ok(pct) = pct_str.parse::<u8>() {
                            telemetry.disk_usage_pct = Some(pct);
                            break;
                        }
                    }
                }
            }
            idx += 1;
        }

        telemetry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_probe_output_linux_standard() {
        let sample = "\
===CX_HOST===
prod-web-01
===CX_UNAME===
Linux 6.8.0-45-generic
===CX_UPTIME===
up 2 weeks, 3 days, 4 hours, 12 minutes
===CX_LOAD===
0.42 0.31 0.25 1/412 9821
===CX_CORES===
8
===CX_FREE===
               total        used        free      shared  buff/cache   available
Mem:           16000        4000        8000         200        4000       11500
Swap:           2048           0        2048
===CX_DF===
/dev/sda1       50264772 21111204  26575188  45% /
===CX_END===
";
        let t = TelemetryCollector::parse_probe_output("web-01", 24, sample);
        assert!(t.is_online);
        assert_eq!(t.latency_ms, Some(24));
        assert_eq!(t.hostname.as_deref(), Some("prod-web-01"));
        assert_eq!(t.os_name.as_deref(), Some("Linux 6.8.0-45-generic"));
        assert_eq!(t.uptime_str.as_deref(), Some("2 weeks, 3 days, 4 hours, 12 minutes"));
        assert_eq!(t.load_avg.as_deref(), Some("0.42, 0.31, 0.25"));
        assert_eq!(t.cpu_cores, Some(8));
        assert_eq!(t.memory_total_mb, Some(16000));
        assert_eq!(t.memory_used_mb, Some(4000));
        assert_eq!(t.memory_percent(), Some(25));
        assert_eq!(t.disk_usage_pct, Some(45));
    }

    #[test]
    fn test_parse_probe_output_docker_overlay_and_missing_hostname() {
        let sample = "\
===CX_HOST===
===CX_UNAME===
Linux 6.1.0-docker
===CX_UPTIME===
up 5 hours, 10 minutes
===CX_LOAD===
{ 1.15 0.90 0.75 }
===CX_CORES===
4
===CX_FREE===
Mem:            8192        2048        6144           0           0        6144
===CX_DF===
overlay         102400000 73728000 28672000  72% /
===CX_END===
";
        let t = TelemetryCollector::parse_probe_output("container-01", 11, sample);
        assert!(t.hostname.is_none());
        assert_eq!(t.os_name.as_deref(), Some("Linux 6.1.0-docker"));
        assert_eq!(t.uptime_str.as_deref(), Some("5 hours, 10 minutes"));
        assert_eq!(t.load_avg.as_deref(), Some("1.15, 0.90, 0.75"));
        assert_eq!(t.cpu_cores, Some(4));
        assert_eq!(t.disk_usage_pct, Some(72));
    }
}
