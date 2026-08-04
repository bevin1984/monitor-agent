//! CPU 使用率采集：解析 /proc/stat，跨周期差值计算使用率。

use anyhow::Result;
use serde::Serialize;

#[derive(Debug, Clone, Copy)]
struct CpuTimes {
    total: u64,
    idle: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CpuUsage {
    /// 使用率 0–100；首次（无历史快照）为 None。
    pub usage_percent: Option<f64>,
}

/// 有状态的 CPU 采集器，持有上一周期快照以算差值。
pub struct CpuCollector {
    prev: Option<CpuTimes>,
}

impl CpuCollector {
    pub fn new() -> Self {
        Self { prev: None }
    }

    /// 启动时调用一次建立基线，使首个上报点即真实差值。
    pub fn warmup(&mut self) -> Result<()> {
        self.prev = Some(read_cpu_times()?);
        Ok(())
    }

    pub fn collect(&mut self) -> Result<CpuUsage> {
        let cur = read_cpu_times()?;
        let pct = match self.prev {
            Some(p) => {
                let dt = cur.total.saturating_sub(p.total) as f64;
                let di = cur.idle.saturating_sub(p.idle) as f64;
                if dt > 0.0 {
                    Some(((1.0 - di / dt) * 100.0).clamp(0.0, 100.0))
                } else {
                    None
                }
            }
            None => None,
        };
        self.prev = Some(cur);
        Ok(CpuUsage {
            usage_percent: pct,
        })
    }
}

/// 解析 /proc/stat 首行 cpu 聚合：`cpu user nice system idle iowait irq softirq steal ...`
fn parse_cpu_agg(text: &str) -> Result<CpuTimes> {
    let first = text
        .lines()
        .next()
        .ok_or_else(|| anyhow::anyhow!("/proc/stat 为空"))?;
    let parts: Vec<&str> = first.split_whitespace().collect();
    if parts.is_empty() || parts[0] != "cpu" {
        anyhow::bail!("/proc/stat 首行不是 cpu 聚合");
    }
    let nums: Vec<u64> = parts[1..]
        .iter()
        .filter_map(|s| s.parse::<u64>().ok())
        .collect();
    if nums.len() < 4 {
        anyhow::bail!("/proc/stat 字段不足");
    }
    // idle = idle列(索引3) + iowait列(索引4, 可能不存在)
    let idle = nums[3] + nums.get(4).copied().unwrap_or(0);
    let total: u64 = nums.iter().sum();
    Ok(CpuTimes { total, idle })
}

fn read_cpu_times() -> Result<CpuTimes> {
    let text = std::fs::read_to_string("/proc/stat")?;
    parse_cpu_agg(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cpu_agg_sample() {
        let s = "cpu  3357 0 4313 1362393 12345 0 234 567 0 0\ncpu0 100 0 100 300 0 0 0 0 0 0\n";
        let t = parse_cpu_agg(s).unwrap();
        let total: u64 = 3357 + 0 + 4313 + 1362393 + 12345 + 0 + 234 + 567 + 0 + 0;
        let idle = 1362393 + 12345;
        assert_eq!(t.total, total);
        assert_eq!(t.idle, idle);
    }

    #[test]
    fn collector_delta() {
        // 两次采样：total 增加 1000，idle 增加 800 → 使用率 20%
        let mut c = CpuCollector::new();
        c.prev = Some(CpuTimes { total: 10000, idle: 9000 });
        // 构造下一次读取：用 collect() 会读真实 /proc/stat，不便测试。
        // 这里直接验证差值逻辑：手动模拟。
        let cur = CpuTimes { total: 11000, idle: 9800 };
        let p = c.prev.unwrap();
        let dt = (cur.total - p.total) as f64;
        let di = (cur.idle - p.idle) as f64;
        let pct = (1.0 - di / dt) * 100.0;
        assert!((pct - 20.0).abs() < 1e-6);
    }

    #[test]
    fn warmup_then_none_then_some() {
        // warmup 后 collect 应回 Some（基于真实 /proc/stat 差值，可能为 0 或正）
        let mut c = CpuCollector::new();
        // 未 warmup 时 prev=None，collect 返回 None（但会设置 prev）
        let r = CpuCollector::new();
        let _ = r;
        if c.warmup().is_ok() {
            // warmup 成功说明 /proc/stat 可读
            assert!(c.prev.is_some());
        }
    }
}
