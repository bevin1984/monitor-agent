//! 系统负载采集：解析 /proc/loadavg。

use anyhow::Result;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct LoadMetrics {
    pub load_1: f64,
    pub load_5: f64,
    pub load_15: f64,
    /// 运行实体数（/proc/loadavg 第 4 字段 '/' 左侧）。
    pub running_procs: u64,
    /// 总实体数（'/' 右侧）。
    pub total_procs: u64,
}

/// 解析 /proc/loadavg：`0.05 0.03 0.01 2/123 4567`
pub fn parse_loadavg(text: &str) -> Result<LoadMetrics> {
    let parts: Vec<&str> = text.split_whitespace().collect();
    if parts.len() < 4 {
        anyhow::bail!("/proc/loadavg 字段不足");
    }
    let load_1 = parts[0].parse::<f64>()?;
    let load_5 = parts[1].parse::<f64>()?;
    let load_15 = parts[2].parse::<f64>()?;
    let (running, total) = parse_running_total(parts[3])?;
    Ok(LoadMetrics {
        load_1,
        load_5,
        load_15,
        running_procs: running,
        total_procs: total,
    })
}

fn parse_running_total(s: &str) -> Result<(u64, u64)> {
    let mut it = s.split('/');
    let r = it
        .next()
        .ok_or_else(|| anyhow::anyhow!("bad procs field"))?
        .parse::<u64>()?;
    let t = it
        .next()
        .ok_or_else(|| anyhow::anyhow!("bad procs field"))?
        .parse::<u64>()?;
    Ok((r, t))
}

pub fn read() -> Result<LoadMetrics> {
    let text = std::fs::read_to_string("/proc/loadavg")?;
    parse_loadavg(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sample() {
        let s = "0.84 0.44 0.20 2/461 36456\n";
        let m = parse_loadavg(s).unwrap();
        assert!((m.load_1 - 0.84).abs() < 1e-9);
        assert!((m.load_5 - 0.44).abs() < 1e-9);
        assert!((m.load_15 - 0.20).abs() < 1e-9);
        assert_eq!(m.running_procs, 2);
        assert_eq!(m.total_procs, 461);
    }

    #[test]
    fn read_from_proc() {
        if let Ok(m) = read() {
            assert!(m.load_1 >= 0.0);
            assert!(m.total_procs > 0);
        }
    }
}
