//! 配置加载与校验。所有字段带默认值（向前兼容），支持环境变量覆盖。

use anyhow::{bail, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;

/// 顶层配置。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub agent: AgentConfig,
    pub network_client: NetworkClientConfig,
    pub retry_queue: RetryQueueConfig,
    pub collect: CollectConfig,
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub url: String,
    pub key: String,
    /// 携带 key 的请求头名称。
    pub key_header: String,
    /// 信任的企业 CA PEM 文件路径；留空则用内置公共 CA。
    pub ca_bundle_path: String,
    /// 应急跳过证书校验（不推荐）。
    pub tls_skip_verify: bool,
}
impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            url: "https://your-receiver.example.com/data/server".to_string(),
            key: "CHANGE_ME".to_string(),
            key_header: "X-Monitor-Key".to_string(),
            ca_bundle_path: String::new(),
            tls_skip_verify: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// 采集推送频率（分钟）。
    pub interval_minutes: u64,
    /// 服务器自定义标识；为空时回退 hostname。
    pub label: String,
}
impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            interval_minutes: 1,
            label: String::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct NetworkClientConfig {
    pub connect_timeout_secs: u64,
    pub request_timeout_secs: u64,
    pub max_retries: u32,
    pub retry_base_delay_ms: u64,
}
impl Default for NetworkClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout_secs: 5,
            request_timeout_secs: 15,
            max_retries: 3,
            retry_base_delay_ms: 500,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RetryQueueConfig {
    pub enabled: bool,
    pub max_items: usize,
    pub flush_batch_size: usize,
}
impl Default for RetryQueueConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_items: 60,
            flush_batch_size: 10,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CollectConfig {
    pub cpu: bool,
    pub memory: bool,
    pub load: bool,
    pub process: bool,
    pub connections: bool,
    pub host: bool,
    pub disks: DiskFilterConfig,
    pub disk_io: DiskIoFilterConfig,
    pub network: NetworkFilterConfig,
}
impl Default for CollectConfig {
    fn default() -> Self {
        Self {
            cpu: true,
            memory: true,
            load: true,
            process: true,
            connections: true,
            host: true,
            disks: DiskFilterConfig::default(),
            disk_io: DiskIoFilterConfig::default(),
            network: NetworkFilterConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DiskFilterConfig {
    pub enabled: bool,
    pub mountpoint_blocklist: Vec<String>,
    pub fstype_blocklist: Vec<String>,
}
impl Default for DiskFilterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mountpoint_blocklist: Vec::new(),
            fstype_blocklist: [
                "proc", "sysfs", "devtmpfs", "devfs", "tmpfs", "tmp", "overlay", "squashfs",
                "cgroup", "cgroup2", "mqueue", "hugetlbfs", "autofs", "rpc_pipefs",
                "fuse.gvfsd-fuse", "none",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DiskIoFilterConfig {
    pub enabled: bool,
    pub device_blocklist: Vec<String>,
}
impl Default for DiskIoFilterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            device_blocklist: vec!["loop*".to_string(), "ram*".to_string()],
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct NetworkFilterConfig {
    pub enabled: bool,
    pub interface_blocklist: Vec<String>,
}
impl Default for NetworkFilterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interface_blocklist: vec!["lo".to_string()],
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
}
impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "warn".to_string(),
        }
    }
}

impl Config {
    /// 从 TOML 文件加载；文件缺失则用默认值；随后用环境变量覆盖部分字段。
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let mut cfg = if path.as_ref().exists() {
            let text = std::fs::read_to_string(&path)?;
            toml::from_str::<Config>(&text)?
        } else {
            log::warn!(
                "配置文件 {} 不存在，使用默认值",
                path.as_ref().display()
            );
            Config::default()
        };
        cfg.apply_env();
        cfg.validate()?;
        Ok(cfg)
    }

    /// 环境变量覆盖（便于编排/容器注入）。优先级高于配置文件。
    fn apply_env(&mut self) {
        if let Ok(v) = std::env::var("MONITOR_AGENT_SERVER_URL") {
            if !v.is_empty() {
                self.server.url = v;
            }
        }
        if let Ok(v) = std::env::var("MONITOR_AGENT_KEY") {
            if !v.is_empty() {
                self.server.key = v;
            }
        }
        if let Ok(v) = std::env::var("MONITOR_AGENT_LABEL") {
            self.agent.label = v;
        }
        if let Ok(v) = std::env::var("MONITOR_AGENT_INTERVAL_MINUTES") {
            if let Ok(n) = v.parse::<u64>() {
                self.agent.interval_minutes = n;
            }
        }
    }

    /// 基础格式校验（不强制 key，key 校验由运行模式决定）。
    pub fn validate(&self) -> Result<()> {
        let url = self.server.url.trim();
        if url.is_empty() {
            bail!("server.url 不能为空");
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            bail!("server.url 必须以 http:// 或 https:// 开头");
        }
        if self.agent.interval_minutes == 0 {
            bail!("agent.interval_minutes 必须 >= 1");
        }
        if self.network_client.max_retries > 10 {
            bail!("network_client.max_retries 过大（需 <= 10）");
        }
        Ok(())
    }

    /// 编译采集过滤规则。
    pub fn compiled_filters(&self) -> Result<CompiledFilters> {
        CompiledFilters::from_config(&self.collect)
    }
}

/// 预编译的过滤规则集合。
#[derive(Debug)]
pub struct CompiledFilters {
    pub mountpoint_block: GlobSet,
    pub fstype_block: HashSet<String>,
    pub device_block: GlobSet,
    pub interface_block: GlobSet,
}

fn build_globset(patterns: &[String]) -> Result<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        b.add(Glob::new(p)?);
    }
    Ok(b.build()?)
}

impl CompiledFilters {
    pub fn from_config(c: &CollectConfig) -> Result<Self> {
        Ok(Self {
            mountpoint_block: build_globset(&c.disks.mountpoint_blocklist)?,
            fstype_block: c.disks.fstype_blocklist.iter().cloned().collect(),
            device_block: build_globset(&c.disk_io.device_blocklist)?,
            interface_block: build_globset(&c.network.interface_blocklist)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let c = Config::default();
        assert_eq!(c.server.key_header, "X-Monitor-Key");
        assert_eq!(c.server.key, "CHANGE_ME");
        assert_eq!(c.agent.interval_minutes, 1);
        assert!(c.retry_queue.enabled);
        assert!(c.collect.cpu);
        assert!(c.collect.disks.enabled);
        assert!(c.collect.network.interface_blocklist.contains(&"lo".to_string()));
        assert!(c.collect.disks.fstype_blocklist.contains(&"tmpfs".to_string()));
    }

    #[test]
    fn parse_overrides() {
        let toml = r#"
[server]
url = "https://example.com/api"
key = "abc123"

[agent]
interval_minutes = 5
label = "web-01"

[collect.network]
interface_blocklist = ["lo", "docker*", "veth*"]
"#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.server.url, "https://example.com/api");
        assert_eq!(c.server.key, "abc123");
        assert_eq!(c.agent.interval_minutes, 5);
        assert_eq!(c.agent.label, "web-01");
        assert_eq!(c.collect.network.interface_blocklist.len(), 3);
        // 未指定的字段保留默认
        assert!(c.collect.cpu);
    }

    #[test]
    fn validate_bad_url() {
        let mut c = Config::default();
        c.server.url = "ftp://x".into();
        assert!(c.validate().is_err());
        c.server.url = "".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn validate_zero_interval() {
        let mut c = Config::default();
        c.agent.interval_minutes = 0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn glob_compile_and_match() {
        let c = Config::default();
        let f = c.compiled_filters().unwrap();
        assert!(f.interface_block.is_match("lo"));
        assert!(!f.interface_block.is_match("eth0"));
        assert!(f.device_block.is_match("loop0"));
        assert!(!f.device_block.is_match("sda"));
        assert!(f.fstype_block.contains("tmpfs"));
    }
}
