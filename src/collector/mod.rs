//! 系统指标采集子模块与顶层快照整合。

pub mod connections;
pub mod cpu;
pub mod disk;
pub mod diskio;
pub mod host;
pub mod load;
pub mod memory;
pub mod network;
pub mod process;

use crate::config::{CollectConfig, CompiledFilters, Config};
use serde::Serialize;

/// 顶层 JSON 快照（推送到接收端的完整负载）。
#[derive(Debug, Serialize)]
pub struct MetricSnapshot {
    pub schema_version: u32,
    pub agent_version: String,
    /// unix 秒（UTC）。
    pub ts: u64,
    /// 服务器自定义标识；为空时回退 hostname。
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<host::HostMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<CpuSection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<memory::MemoryMetrics>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disks: Vec<disk::DiskMetrics>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disk_io: Vec<diskio::DiskIoMetrics>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<network::NetMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tcp: Option<connections::TcpMetrics>,
}

/// CPU 区段：使用率 + 负载 + 进程数。
#[derive(Debug, Serialize)]
pub struct CpuSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_percent: Option<f64>,
    pub load_1: f64,
    pub load_5: f64,
    pub load_15: f64,
    pub running_procs: u64,
    pub total_procs: u64,
    pub process_count: u64,
}

/// 采集器，持有有状态采集器（CPU/磁盘IO/网络）的跨周期快照。
pub struct Collector {
    filters: CompiledFilters,
    cfg: CollectConfig,
    label: String,
    cpu: cpu::CpuCollector,
    disk_io: diskio::DiskIoCollector,
    network: network::NetworkCollector,
}

impl Collector {
    pub fn new(cfg: &Config) -> anyhow::Result<Self> {
        Ok(Self {
            filters: cfg.compiled_filters()?,
            cfg: cfg.collect.clone(),
            label: cfg.agent.label.clone(),
            cpu: cpu::CpuCollector::new(),
            disk_io: diskio::DiskIoCollector::new(),
            network: network::NetworkCollector::new(),
        })
    }

    /// 启动时建立差值类指标基线（常驻模式调用；--once 不调用）。
    pub fn warmup(&mut self) -> anyhow::Result<()> {
        self.cpu.warmup()?;
        if self.cfg.disk_io.enabled {
            self.disk_io.warmup(&self.filters)?;
        }
        if self.cfg.network.enabled {
            self.network.warmup(&self.filters)?;
        }
        Ok(())
    }

    pub fn collect(&mut self) -> MetricSnapshot {
        let ts = unix_now();
        let label = if self.label.is_empty() {
            read_hostname()
        } else {
            self.label.clone()
        };
        let mut s = MetricSnapshot {
            schema_version: 1,
            agent_version: env!("CARGO_PKG_VERSION").to_string(),
            ts,
            label,
            host: None,
            cpu: None,
            memory: None,
            disks: Vec::new(),
            disk_io: Vec::new(),
            networks: Vec::new(),
            tcp: None,
        };

        if self.cfg.host {
            if let Ok(h) = host::read() {
                s.host = Some(h);
            }
        }

        if self.cfg.cpu || self.cfg.load || self.cfg.process {
            let usage = if self.cfg.cpu {
                self.cpu.collect().ok().and_then(|u| u.usage_percent)
            } else {
                None
            };
            let load = if self.cfg.load { load::read().ok() } else { None };
            let proc = if self.cfg.process { process::read().ok() } else { None };
            s.cpu = Some(CpuSection {
                usage_percent: usage,
                load_1: load.as_ref().map(|l| l.load_1).unwrap_or(0.0),
                load_5: load.as_ref().map(|l| l.load_5).unwrap_or(0.0),
                load_15: load.as_ref().map(|l| l.load_15).unwrap_or(0.0),
                running_procs: load.as_ref().map(|l| l.running_procs).unwrap_or(0),
                total_procs: load.as_ref().map(|l| l.total_procs).unwrap_or(0),
                process_count: proc.map(|p| p.process_count).unwrap_or(0),
            });
        }

        if self.cfg.memory {
            if let Ok(m) = memory::read() {
                s.memory = Some(m);
            }
        }
        if self.cfg.disks.enabled {
            s.disks = disk::read(&self.filters);
        }
        if self.cfg.disk_io.enabled {
            s.disk_io = self.disk_io.collect(&self.filters);
        }
        if self.cfg.network.enabled {
            s.networks = self.network.collect(&self.filters);
        }
        if self.cfg.connections {
            if let Ok(t) = connections::read() {
                s.tcp = Some(t);
            }
        }
        s
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_once_snapshot() {
        // --once 等价：不 warmup，直接 collect
        let cfg = Config::default();
        let mut c = Collector::new(&cfg).unwrap();
        let s = c.collect();
        assert_eq!(s.schema_version, 1);
        assert!(!s.agent_version.is_empty());
        assert!(s.ts > 0);
        // 未 warmup：disk_io 为空、cpu.usage 为 None
        assert!(s.disk_io.is_empty());
        if let Some(cpu) = &s.cpu {
            assert!(cpu.usage_percent.is_none());
        }
        assert!(s.host.is_some());
        assert!(s.memory.is_some());
    }

    #[test]
    fn label_fallback_hostname() {
        let cfg = Config::default();
        let mut c = Collector::new(&cfg).unwrap();
        let s = c.collect();
        assert!(!s.label.is_empty()); // label 空 → 回退 hostname
    }
}
