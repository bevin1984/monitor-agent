//! 网络带宽与 IP 采集：/proc/net/dev 算速率 + libc::getifaddrs 取 IP。

use crate::config::CompiledFilters;
use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::Instant;

#[derive(Debug, Clone, Serialize)]
pub struct NetMetrics {
    pub interface: String,
    /// 该网卡全局 IPv4 地址（已过滤链路本地 169.254/16）；可能为空数组。
    pub ipv4: Vec<String>,
    /// 该网卡全局 IPv6 地址（已过滤链路本地 fe80::/10）；可能为空数组。
    pub ipv6: Vec<String>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
}

#[derive(Debug, Clone, Default)]
struct NetStat {
    rx_bytes: u64,
    tx_bytes: u64,
}

/// 有状态的网络采集器，持有上一周期快照以算速率。
pub struct NetworkCollector {
    prev: HashMap<String, (NetStat, Instant)>,
}

impl NetworkCollector {
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

    pub fn collect(&mut self, filters: &CompiledFilters) -> Vec<NetMetrics> {
        let now = Instant::now();
        let cur = match snapshot(filters) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("读取 /proc/net/dev 失败: {}", e);
                return Vec::new();
            }
        };
        let ips = list_interface_ips();
        let mut out = Vec::new();
        for (iface, cur_stat) in &cur {
            let (rbps, tbps) = match self.prev.get(iface) {
                Some((p, t)) => {
                    let dt = now.duration_since(*t).as_secs_f64();
                    if dt > 0.0 {
                        (
                            cur_stat.rx_bytes.saturating_sub(p.rx_bytes) as f64 / dt,
                            cur_stat.tx_bytes.saturating_sub(p.tx_bytes) as f64 / dt,
                        )
                    } else {
                        (0.0, 0.0)
                    }
                }
                None => (0.0, 0.0),
            };
            let (ipv4, ipv6) = ips.get(iface).cloned().unwrap_or_default();
            out.push(NetMetrics {
                interface: iface.clone(),
                ipv4,
                ipv6,
                rx_bytes: cur_stat.rx_bytes,
                tx_bytes: cur_stat.tx_bytes,
                rx_bytes_per_sec: rbps,
                tx_bytes_per_sec: tbps,
            });
        }
        self.prev = cur.into_iter().map(|(k, v)| (k, (v, now))).collect();
        out
    }
}

/// 解析 /proc/net/dev：跳过 2 行表头，cols[1]=rx_bytes, cols[9]=tx_bytes。
fn parse_net_dev(text: &str) -> Vec<(String, NetStat)> {
    let mut out = Vec::new();
    for line in text.lines().skip(2) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 10 {
            continue;
        }
        let iface = cols[0].trim_end_matches(':').to_string();
        let rx: u64 = cols[1].parse().unwrap_or(0);
        let tx: u64 = cols[9].parse().unwrap_or(0);
        out.push((iface, NetStat { rx_bytes: rx, tx_bytes: tx }));
    }
    out
}

fn snapshot(filters: &CompiledFilters) -> Result<HashMap<String, NetStat>> {
    let text = std::fs::read_to_string("/proc/net/dev")?;
    Ok(parse_net_dev(&text)
        .into_iter()
        .filter(|(iface, _)| !filters.interface_block.is_match(iface))
        .collect())
}

/// 通过 getifaddrs 获取各网卡 IPv4/IPv6 地址（过滤链路本地），返回 iface -> (ipv4, ipv6)。
fn list_interface_ips() -> HashMap<String, (Vec<String>, Vec<String>)> {
    use std::collections::HashSet;
    use std::ffi::CStr;

    let mut tmp: HashMap<String, (HashSet<String>, HashSet<String>)> = HashMap::new();
    let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs 成功时写入链表头到 ifap；失败返回非 0。
    if unsafe { libc::getifaddrs(&mut ifap) } != 0 || ifap.is_null() {
        return HashMap::new();
    }
    let mut cur = ifap;
    while !cur.is_null() {
        // SAFETY: cur 来自 getifaddrs 的有效链表节点，非空时可解引用。
        let ifa = unsafe { &*cur };
        let name = unsafe { CStr::from_ptr(ifa.ifa_name as *const libc::c_char) };
        let name = match name.to_str() {
            Ok(s) => s.to_string(),
            Err(_) => {
                cur = ifa.ifa_next;
                continue;
            }
        };
        if !ifa.ifa_addr.is_null() {
            // SAFETY: ifa_addr 指向有效 sockaddr（getifaddrs 保证）。
            let family = i32::from(unsafe { (*ifa.ifa_addr).sa_family });
            if family == libc::AF_INET {
                // SAFETY: family 已确认为 AF_INET，可按 sockaddr_in 解析。
                let sin = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in) };
                let ip = Ipv4Addr::from(sin.sin_addr.s_addr.to_ne_bytes());
                if !is_link_local_ipv4(&ip) {
                    tmp.entry(name.clone()).or_default().0.insert(ip.to_string());
                }
            } else if family == libc::AF_INET6 {
                // SAFETY: family 已确认为 AF_INET6，可按 sockaddr_in6 解析。
                let sin6 = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in6) };
                let ip = Ipv6Addr::from(sin6.sin6_addr.s6_addr);
                if !is_link_local_ipv6(&ip) {
                    tmp.entry(name.clone()).or_default().1.insert(ip.to_string());
                }
            }
        }
        cur = ifa.ifa_next;
    }
    // SAFETY: 释放 getifaddrs 分配的链表。
    unsafe { libc::freeifaddrs(ifap) };

    tmp.into_iter()
        .map(|(k, (v4, v6))| {
            let mut v4: Vec<String> = v4.into_iter().collect();
            v4.sort();
            let mut v6: Vec<String> = v6.into_iter().collect();
            v6.sort();
            (k, (v4, v6))
        })
        .collect()
}

fn is_link_local_ipv4(ip: &Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 169 && o[1] == 254
}

fn is_link_local_ipv6(ip: &Ipv6Addr) -> bool {
    let s = ip.segments();
    s[0] >= 0xfe80 && s[0] <= 0xfebf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn parse_net_dev_sample() {
        let s = "Inter-|   Receive                                                |  Transmit\n\
face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n\
    lo: 12345 100 0 0 0 0 0 0 12345 100 0 0 0 0 0 0\n\
  eth0: 67890 200 0 0 0 0 0 0 98765 300 0 0 0 0 0 0\n";
        let v = parse_net_dev(s);
        assert_eq!(v.len(), 2);
        let eth0 = v.iter().find(|(n, _)| n == "eth0").unwrap();
        assert_eq!(eth0.1.rx_bytes, 67890);
        assert_eq!(eth0.1.tx_bytes, 98765);
    }

    #[test]
    fn link_local_detection() {
        assert!(is_link_local_ipv4(&"169.254.1.1".parse().unwrap()));
        assert!(!is_link_local_ipv4(&"10.0.0.1".parse().unwrap()));
        assert!(is_link_local_ipv6(&"fe80::1".parse().unwrap()));
        assert!(!is_link_local_ipv6(&"2001:db8::1".parse().unwrap()));
    }

    #[test]
    fn collect_has_ip_fields() {
        let f = Config::default().compiled_filters().unwrap();
        let mut c = NetworkCollector::new();
        let _ = c.warmup(&f);
        let m = c.collect(&f);
        for n in &m {
            assert_ne!(n.interface, "lo");
            assert!(n.ipv4.iter().all(|s| s.contains('.')));
            assert!(n.ipv6.iter().all(|s| s.contains(':')));
        }
    }
}
