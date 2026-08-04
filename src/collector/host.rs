//! 主机元数据采集：hostname / machine-id / product_uuid / os-release / kernel / cpuinfo / uptime。

use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize)]
pub struct HostMetrics {
    pub hostname: String,
    /// 机器唯一标识（首选），克隆镜像场景需注意去重。
    pub machine_id: String,
    /// DMI 产品 UUID（root 可读，作为 machine-id 重复时的辅助）。
    pub product_uuid: String,
    pub os: String,
    pub os_id: String,
    pub os_version: String,
    pub kernel: String,
    pub arch: String,
    pub cpu_model: String,
    pub cpu_cores: u64,
    pub uptime_secs: u64,
}

pub fn read() -> Result<HostMetrics> {
    let hostname = read_first_line("/proc/sys/kernel/hostname").unwrap_or_default();
    let machine_id = read_first_line("/etc/machine-id")
        .or_else(|_| read_first_line("/var/lib/dbus/machine-id"))
        .unwrap_or_default();
    let product_uuid = read_first_line("/sys/class/dmi/id/product_uuid").unwrap_or_default();
    let (os, os_id, os_version) = parse_os_release().unwrap_or_default();
    let kernel = read_first_line("/proc/sys/kernel/osrelease").unwrap_or_default();
    let arch = std::env::consts::ARCH.to_string();
    let (cpu_model, cpu_cores) = parse_cpuinfo().unwrap_or((String::new(), 0));
    let uptime_secs = parse_uptime().unwrap_or(0);
    Ok(HostMetrics {
        hostname,
        machine_id,
        product_uuid,
        os,
        os_id,
        os_version,
        kernel,
        arch,
        cpu_model,
        cpu_cores,
        uptime_secs,
    })
}

fn read_first_line(path: &str) -> Result<String> {
    let s = std::fs::read_to_string(path)?;
    Ok(s.trim().to_string())
}

fn parse_os_release() -> Result<(String, String, String)> {
    let text = std::fs::read_to_string("/etc/os-release")?;
    let map = parse_osrelease_kv(&text);
    Ok((
        map.get("PRETTY_NAME").cloned().unwrap_or_default(),
        map.get("ID").cloned().unwrap_or_default(),
        map.get("VERSION_ID").cloned().unwrap_or_default(),
    ))
}

/// 解析 os-release 的 `KEY=value`（去掉值的引号）。
pub fn parse_osrelease_kv(text: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim().trim_matches('"').to_string();
            m.insert(k.trim().to_string(), v);
        }
    }
    m
}

/// 解析 /proc/cpuinfo：首个 model name + processor 行数（逻辑核数）。
pub fn parse_cpuinfo() -> Result<(String, u64)> {
    let text = std::fs::read_to_string("/proc/cpuinfo")?;
    Ok(parse_cpuinfo_text(&text))
}

pub fn parse_cpuinfo_text(text: &str) -> (String, u64) {
    let mut model = String::new();
    let mut cores = 0u64;
    for line in text.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim();
            let v = v.trim();
            if k == "model name" && model.is_empty() {
                model = v.to_string();
            }
            if k == "processor" {
                cores += 1;
            }
        }
    }
    (model, cores)
}

pub fn parse_uptime() -> Result<u64> {
    let text = std::fs::read_to_string("/proc/uptime")?;
    parse_uptime_text(&text)
}

pub fn parse_uptime_text(text: &str) -> Result<u64> {
    let first = text
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow::anyhow!("/proc/uptime 为空"))?;
    let secs: f64 = first.parse()?;
    Ok(secs as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_osrelease_sample() {
        let s = "PRETTY_NAME=\"Anolis OS 8.10\"\nID=anolis\nVERSION_ID=\"8.10\"\n";
        let m = parse_osrelease_kv(s);
        assert_eq!(m.get("PRETTY_NAME").unwrap(), "Anolis OS 8.10");
        assert_eq!(m.get("ID").unwrap(), "anolis");
        assert_eq!(m.get("VERSION_ID").unwrap(), "8.10");
    }

    #[test]
    fn parse_cpuinfo_sample() {
        let s = "processor\t: 0\nmodel name\t: Intel(R) Xeon(R) CPU @ 2.70GHz\nprocessor\t: 1\nmodel name\t: Intel(R) Xeon(R) CPU @ 2.70GHz\n";
        let (model, cores) = parse_cpuinfo_text(s);
        assert_eq!(model, "Intel(R) Xeon(R) CPU @ 2.70GHz");
        assert_eq!(cores, 2);
    }

    #[test]
    fn parse_uptime_sample() {
        let s = "1816988.60 72627289.64\n";
        assert_eq!(parse_uptime_text(s).unwrap(), 1816988);
    }

    #[test]
    fn read_from_proc() {
        if let Ok(h) = read() {
            assert!(!h.hostname.is_empty());
            assert!(h.uptime_secs > 0 || h.uptime_secs == 0); // 仅验证不 panic
            assert!(!h.kernel.is_empty());
        }
    }
}
