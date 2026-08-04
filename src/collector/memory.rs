//! 内存采集：解析 /proc/meminfo。used = MemTotal - MemAvailable（更准）。

use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;

const KB: u64 = 1024;

#[derive(Debug, Clone, Serialize)]
pub struct MemoryMetrics {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub used_percent: f64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
}

/// 解析 /proc/meminfo 文本。
pub fn parse_meminfo(text: &str) -> Result<MemoryMetrics> {
    let map = parse_kv(text);
    let total_kb = *map.get("MemTotal").unwrap_or(&0);
    // MemAvailable（内核 3.14+）更准；缺失则回退 MemFree+Buffers+Cached
    let avail_kb = match map.get("MemAvailable") {
        Some(&v) => v,
        None => {
            let f = *map.get("MemFree").unwrap_or(&0);
            let b = *map.get("Buffers").unwrap_or(&0);
            let c = *map.get("Cached").unwrap_or(&0);
            f + b + c
        }
    };
    let total = total_kb * KB;
    let available = avail_kb * KB;
    let used = total.saturating_sub(available);
    let used_percent = if total > 0 {
        used as f64 / total as f64 * 100.0
    } else {
        0.0
    };
    let swap_total_kb = *map.get("SwapTotal").unwrap_or(&0);
    let swap_free_kb = *map.get("SwapFree").unwrap_or(&0);
    let swap_total = swap_total_kb * KB;
    let swap_used = swap_total.saturating_sub(swap_free_kb * KB);
    Ok(MemoryMetrics {
        total_bytes: total,
        used_bytes: used,
        available_bytes: available,
        used_percent,
        swap_total_bytes: swap_total,
        swap_used_bytes: swap_used,
    })
}

/// 解析形如 "MemTotal:       16333752 kB" 的键值行。
fn parse_kv(text: &str) -> HashMap<&str, u64> {
    let mut m = HashMap::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let Some(key) = it.next() else { continue };
        let key = key.trim_end_matches(':');
        let Some(val) = it.next() else { continue };
        if let Ok(n) = val.parse::<u64>() {
            m.insert(key, n);
        }
    }
    m
}

/// 从 /proc/meminfo 实时读取。
pub fn read() -> Result<MemoryMetrics> {
    let text = std::fs::read_to_string("/proc/meminfo")?;
    parse_meminfo(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sample() {
        let s = "MemTotal:       16333752 kB\n\
                 MemFree:         2410084 kB\n\
                 MemAvailable:    8765432 kB\n\
                 Buffers:         1234567 kB\n\
                 Cached:          4567890 kB\n\
                 SwapTotal:       2097148 kB\n\
                 SwapFree:        2097148 kB\n";
        let m = parse_meminfo(s).unwrap();
        assert_eq!(m.total_bytes, 16333752 * 1024);
        assert_eq!(m.available_bytes, 8765432 * 1024);
        assert_eq!(m.used_bytes, (16333752 - 8765432) * 1024);
        let expect_pct = (16333752 - 8765432) as f64 / 16333752.0 * 100.0;
        assert!((m.used_percent - expect_pct).abs() < 1e-6);
        assert_eq!(m.swap_total_bytes, 2097148 * 1024);
        assert_eq!(m.swap_used_bytes, 0);
    }

    #[test]
    fn parse_no_memavailable_fallback() {
        let s = "MemTotal:       10000 kB\n\
                 MemFree:         2000 kB\n\
                 Buffers:         1000 kB\n\
                 Cached:          1000 kB\n";
        let m = parse_meminfo(s).unwrap();
        assert_eq!(m.available_bytes, 4000 * 1024);
        assert_eq!(m.used_bytes, 6000 * 1024);
        assert_eq!(m.swap_total_bytes, 0);
    }

    #[test]
    fn read_from_proc() {
        // 实机验证（CI 的 Linux 环境应存在 /proc/meminfo）
        if let Ok(m) = read() {
            assert!(m.total_bytes > 0);
        }
    }
}
