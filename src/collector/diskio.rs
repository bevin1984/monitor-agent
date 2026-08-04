//! 磁盘读写采集：/proc/diskstats 顶层块设备，跨周期算 read/write bytes/s 与 iops。

use crate::config::CompiledFilters;
use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

const SECTOR: u64 = 512;

#[derive(Debug, Clone, Serialize)]
pub struct DiskIoMetrics {
    pub device: String,
    pub read_bytes_per_sec: f64,
    pub write_bytes_per_sec: f64,
    pub read_iops: f64,
    pub write_iops: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiskStat {
    pub reads_completed: u64,
    pub sectors_read: u64,
    pub writes_completed: u64,
    pub sectors_written: u64,
}

/// 有状态的 disk IO 采集器，持有上一周期快照（含时间戳）以算速率。
pub struct DiskIoCollector {
    prev: HashMap<String, (DiskStat, Instant)>,
}

impl DiskIoCollector {
    pub fn new() -> Self {
        Self {
            prev: HashMap::new(),
        }
    }

    pub fn warmup(&mut self, filters: &CompiledFilters) -> Result<()> {
        let now = Instant::now();
        self.prev = snapshot(filters)?
            .into_iter()
            .map(|(k, v)| (k, (v, now)))
            .collect();
        Ok(())
    }

    pub fn collect(&mut self, filters: &CompiledFilters) -> Vec<DiskIoMetrics> {
        let now = Instant::now();
        let cur = match snapshot(filters) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("读取 diskstats 失败: {}", e);
                return Vec::new();
            }
        };
        let mut out = Vec::new();
        for (dev, cur_stat) in &cur {
            if let Some((p_stat, p_when)) = self.prev.get(dev) {
                let dt = now.duration_since(*p_when).as_secs_f64();
                if dt <= 0.0 {
                    continue;
                }
                let dr = cur_stat.sectors_read.saturating_sub(p_stat.sectors_read) as f64
                    * SECTOR as f64
                    / dt;
                let dw = cur_stat.sectors_written.saturating_sub(p_stat.sectors_written) as f64
                    * SECTOR as f64
                    / dt;
                let rr = cur_stat.reads_completed.saturating_sub(p_stat.reads_completed) as f64 / dt;
                let ww = cur_stat.writes_completed.saturating_sub(p_stat.writes_completed) as f64
                    / dt;
                out.push(DiskIoMetrics {
                    device: dev.clone(),
                    read_bytes_per_sec: dr,
                    write_bytes_per_sec: dw,
                    read_iops: rr,
                    write_iops: ww,
                });
            }
        }
        self.prev = cur.into_iter().map(|(k, v)| (k, (v, now))).collect();
        out
    }
}

/// 解析 /proc/diskstats：major minor name reads reads_merged sectors_read ...
pub fn parse_diskstats(text: &str) -> Vec<(String, DiskStat)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 11 {
            continue;
        }
        let name = parts[2].to_string();
        let stat = DiskStat {
            reads_completed: parts[3].parse().unwrap_or(0),
            sectors_read: parts[5].parse().unwrap_or(0),
            writes_completed: parts[7].parse().unwrap_or(0),
            sectors_written: parts[9].parse().unwrap_or(0),
        };
        out.push((name, stat));
    }
    out
}

/// 是否为顶层块设备（/sys/block/<name> 存在）。排除分区（sda1/nvme0n1p1 等）。
fn is_top_level(name: &str) -> bool {
    Path::new("/sys/block").join(name).exists()
}

/// 读取当前各顶层设备的累计统计（不含时间戳）。
fn snapshot(filters: &CompiledFilters) -> Result<HashMap<String, DiskStat>> {
    let text = std::fs::read_to_string("/proc/diskstats")?;
    Ok(parse_diskstats(&text)
        .into_iter()
        .filter(|(name, _)| is_top_level(name))
        .filter(|(name, _)| !filters.device_block.is_match(name))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn parse_diskstats_sample() {
        let s = "   8       0 sda 100 5 2000 500 50 2 1000 200 0 100 0 0 0 0\n   8       1 sda1 10 0 100 50 5 0 50 10 0 50 0 0 0 0\n";
        let v = parse_diskstats(s);
        assert_eq!(v.len(), 2);
        let sda = v.iter().find(|(n, _)| n == "sda").unwrap();
        assert_eq!(sda.1.reads_completed, 100);
        assert_eq!(sda.1.sectors_read, 2000);
        assert_eq!(sda.1.writes_completed, 50);
        assert_eq!(sda.1.sectors_written, 1000);
    }

    #[test]
    fn collector_collect_runs() {
        let f = Config::default().compiled_filters().unwrap();
        let mut c = DiskIoCollector::new();
        let _ = c.warmup(&f);
        let _ = c.collect(&f);
    }

    #[test]
    fn rate_math() {
        let prev = DiskStat::default();
        let cur = DiskStat {
            reads_completed: 10,
            sectors_read: 1024,
            writes_completed: 5,
            sectors_written: 512,
        };
        let dt = 1.0_f64;
        let dr = (cur.sectors_read - prev.sectors_read) as f64 * SECTOR as f64 / dt;
        assert!((dr - (1024.0 * 512.0)).abs() < 1e-6);
    }
}
