//! 进程数采集：数 /proc 下纯数字目录（即进程数）。

use anyhow::Result;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ProcessMetrics {
    pub process_count: u64,
}

/// 统计 /proc 下纯数字子目录数量（每个对应一个进程）。
pub fn count_processes() -> Result<u64> {
    let entries = std::fs::read_dir("/proc")?;
    let mut n = 0u64;
    for e in entries.flatten() {
        let is_pid = e
            .file_name()
            .to_str()
            .map(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or(false);
        if is_pid {
            n += 1;
        }
    }
    Ok(n)
}

pub fn read() -> Result<ProcessMetrics> {
    Ok(ProcessMetrics {
        process_count: count_processes()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_from_proc() {
        // 实机：至少有 init 进程
        if let Ok(m) = read() {
            assert!(m.process_count > 0);
        }
    }
}
