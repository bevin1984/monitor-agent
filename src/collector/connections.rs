//! TCP 连接数采集：/proc/net/tcp(+tcp6) 统计 established/time_wait/listen/total。

use anyhow::Result;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, Default)]
pub struct TcpMetrics {
    pub established: u64,
    pub time_wait: u64,
    pub listen: u64,
    pub total: u64,
}

/// 解析 /proc/net/tcp 或 /proc/net/tcp6 文本（跳过表头，第 4 列为十六进制状态）。
/// 01=ESTABLISHED, 06=TIME_WAIT, 0A=LISTEN。
pub fn parse_tcp(text: &str) -> TcpMetrics {
    let mut m = TcpMetrics::default();
    for line in text.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 4 {
            continue;
        }
        let st = cols[3];
        m.total += 1;
        match st {
            "01" => m.established += 1,
            "06" => m.time_wait += 1,
            "0A" => m.listen += 1,
            _ => {}
        }
    }
    m
}

/// 合并 IPv4(/proc/net/tcp) 与 IPv6(/proc/net/tcp6)。
pub fn read() -> Result<TcpMetrics> {
    let mut m = TcpMetrics::default();
    for path in ["/proc/net/tcp", "/proc/net/tcp6"] {
        if let Ok(text) = std::fs::read_to_string(path) {
            let one = parse_tcp(&text);
            m.established += one.established;
            m.time_wait += one.time_wait;
            m.listen += one.listen;
            m.total += one.total;
        }
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tcp_sample() {
        let s = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 0100007F:0016 00000000:0000 0A 0 0 00:00000000 0 0 0 0 1234 1 0 100 0 0 10 0\n\
   1: 0100007F:D4E2 0100007F:831F 01 0 0 00:00000000 0 0 0 0 1234 1 0 20 4 30 10 -1\n\
   2: 0100007F:1234 0100007F:5678 06 0 0 00:00000000 0 0 0 0 1234 1 0 20 4 30 10 -1\n";
        let m = parse_tcp(s);
        assert_eq!(m.listen, 1);
        assert_eq!(m.established, 1);
        assert_eq!(m.time_wait, 1);
        assert_eq!(m.total, 3);
    }

    #[test]
    fn read_from_proc() {
        if let Ok(m) = read() {
            // total 含所有状态，必不少于 established 子集
            assert!(m.total >= m.established);
        }
    }
}
