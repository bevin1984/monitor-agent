//! 磁盘空间采集：/proc/mounts 过滤伪文件系统 + libc::statvfs。

use crate::config::CompiledFilters;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct DiskMetrics {
    pub device: String,
    pub mountpoint: String,
    pub fstype: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    pub used_percent: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEntry {
    pub device: String,
    pub mountpoint: String,
    pub fstype: String,
}

/// 解析 /proc/mounts（device mountpoint fstype opts dump pass）。
pub fn parse_mounts(text: &str) -> Vec<MountEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }
        out.push(MountEntry {
            device: unescape_octal(parts[0]),
            mountpoint: unescape_octal(parts[1]),
            fstype: parts[2].to_string(),
        });
    }
    out
}

/// /proc/mounts 中空格/特殊字符以 \ooo 八进制转义（如 \040 = 空格）。
pub fn unescape_octal(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 3 < chars.len() {
            let seq: String = chars[i + 1..i + 4].iter().collect();
            if let Ok(n) = u32::from_str_radix(&seq, 8) {
                if let Some(c) = char::from_u32(n) {
                    out.push(c);
                    i += 4;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

struct DiskUsage {
    total: u64,
    bfree: u64,
    avail: u64,
}

fn statvfs(path: &str) -> std::io::Result<DiskUsage> {
    let c = std::ffi::CString::new(path)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: path 为有效 C 字符串，s 为零初始化输出缓冲。
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let fr = s.f_frsize as u64;
    Ok(DiskUsage {
        total: s.f_blocks as u64 * fr,
        bfree: s.f_bfree as u64 * fr,
        avail: s.f_bavail as u64 * fr,
    })
}

/// 采集磁盘空间：过滤后对每个挂载点 statvfs。
pub fn collect(mounts_text: &str, filters: &CompiledFilters) -> Vec<DiskMetrics> {
    let entries = parse_mounts(mounts_text);
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for e in entries {
        if !e.device.starts_with("/dev/") {
            continue;
        }
        if filters.fstype_block.contains(&e.fstype) {
            continue;
        }
        if filters.mountpoint_block.is_match(&e.mountpoint) {
            continue;
        }
        // 同设备多次挂载（bind）只取第一个
        if !seen.insert(e.device.clone()) {
            continue;
        }
        match statvfs(&e.mountpoint) {
            Ok(u) => {
                let total = u.total;
                let used = total.saturating_sub(u.bfree);
                let pct = if total > 0 {
                    used as f64 / total as f64 * 100.0
                } else {
                    0.0
                };
                out.push(DiskMetrics {
                    device: e.device,
                    mountpoint: e.mountpoint,
                    fstype: e.fstype,
                    total_bytes: total,
                    used_bytes: used,
                    free_bytes: u.avail,
                    used_percent: pct,
                });
            }
            Err(err) => {
                log::warn!("statvfs {} 失败: {}", e.mountpoint, err);
            }
        }
    }
    out
}

pub fn read(filters: &CompiledFilters) -> Vec<DiskMetrics> {
    match std::fs::read_to_string("/proc/mounts") {
        Ok(text) => collect(&text, filters),
        Err(err) => {
            log::error!("读取 /proc/mounts 失败: {}", err);
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn parse_mounts_sample() {
        let s = "/dev/sda1 / ext4 rw,relatime 0 0\nproc /proc proc rw 0 0\ntmpfs /run tmpfs rw 0 0\n";
        let m = parse_mounts(s);
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].device, "/dev/sda1");
        assert_eq!(m[0].mountpoint, "/");
        assert_eq!(m[0].fstype, "ext4");
    }

    #[test]
    fn unescape_octal_space() {
        assert_eq!(unescape_octal("/tmp\\040dir"), "/tmp dir");
        assert_eq!(unescape_octal("/normal"), "/normal");
    }

    #[test]
    fn read_filters_pseudo_fs() {
        // 默认过滤 tmpfs/proc 等；过滤后剩余项均为真实块设备
        let f = Config::default().compiled_filters().unwrap();
        let disks = read(&f);
        for d in &disks {
            assert!(d.device.starts_with("/dev/"));
            assert!(!f.fstype_block.contains(&d.fstype));
        }
    }
}
