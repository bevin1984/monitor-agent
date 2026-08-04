//! 上报模块：ureq + rustls 推送 JSON，含超时重试与内存队列补传。

use crate::collector::MetricSnapshot;
use crate::config::Config;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use std::collections::VecDeque;
use std::io::BufReader;
use std::sync::Arc;
use std::time::Duration;

pub struct Reporter {
    agent: ureq::Agent,
    url: String,
    key: String,
    key_header: String,
    max_retries: u32,
    base_delay: Duration,
    queue: VecDeque<String>,
    max_items: usize,
    flush_batch: usize,
}

impl Reporter {
    pub fn new(cfg: &Config) -> anyhow::Result<Self> {
        let agent = build_agent(cfg)?;
        Ok(Self {
            agent,
            url: cfg.server.url.clone(),
            key: cfg.server.key.clone(),
            key_header: cfg.server.key_header.clone(),
            max_retries: cfg.network_client.max_retries,
            base_delay: Duration::from_millis(cfg.network_client.retry_base_delay_ms),
            queue: VecDeque::new(),
            max_items: if cfg.retry_queue.enabled {
                cfg.retry_queue.max_items
            } else {
                0
            },
            flush_batch: cfg.retry_queue.flush_batch_size,
        })
    }

    /// 发送快照：失败且启用队列则入队等待补传。
    pub fn send(&mut self, snapshot: &MetricSnapshot) {
        let payload = match serde_json::to_string(snapshot) {
            Ok(s) => s,
            Err(e) => {
                log::error!("序列化快照失败: {e}");
                return;
            }
        };
        if self.post_with_retry(&payload) {
            self.drain_queue();
        } else if self.max_items > 0 {
            if self.queue.len() >= self.max_items {
                self.queue.pop_front();
            }
            self.queue.push_back(payload);
            log::warn!("上报失败已入队补传（队列长度 {}）", self.queue.len());
        }
    }

    /// 当前队列长度（自监控/测试用）。
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    fn post_with_retry(&self, payload: &str) -> bool {
        let mut delay = self.base_delay;
        for attempt in 0..=self.max_retries {
            match post_once(&self.agent, &self.url, &self.key_header, &self.key, payload) {
                PostOutcome::Ok => return true,
                PostOutcome::NoRetry(msg) => {
                    log::error!("上报失败（不重试）: {msg}");
                    return false;
                }
                PostOutcome::Retry(msg) => {
                    if attempt < self.max_retries {
                        log::warn!("上报失败（第{}次，重试）: {msg}", attempt + 1);
                        std::thread::sleep(delay);
                        delay = delay.saturating_mul(2);
                    } else {
                        log::error!("上报失败（重试耗尽）: {msg}");
                    }
                }
            }
        }
        false
    }

    fn drain_queue(&mut self) {
        let n = self.flush_batch.min(self.queue.len());
        for _ in 0..n {
            let payload = match self.queue.front() {
                Some(p) => p.clone(),
                None => break,
            };
            if matches!(
                post_once(&self.agent, &self.url, &self.key_header, &self.key, &payload),
                PostOutcome::Ok
            ) {
                self.queue.pop_front();
            } else {
                break;
            }
        }
    }
}

enum PostOutcome {
    Ok,
    Retry(String),
    NoRetry(String),
}

/// 纯函数：按 HTTP 状态码分类（便于单元测试）。
fn classify_http_status(code: u16) -> PostOutcome {
    match code {
        200..=299 => PostOutcome::Ok,
        400..=499 => PostOutcome::NoRetry(format!("HTTP {code}")),
        _ => PostOutcome::Retry(format!("HTTP {code}")),
    }
}

fn post_once(
    agent: &ureq::Agent,
    url: &str,
    key_header: &str,
    key: &str,
    payload: &str,
) -> PostOutcome {
    match agent
        .post(url)
        .set(key_header, key)
        .set("Content-Type", "application/json")
        .send_string(payload)
    {
        Ok(_) => PostOutcome::Ok,
        Err(ureq::Error::Status(code, _)) => classify_http_status(code),
        Err(ureq::Error::Transport(t)) => PostOutcome::Retry(format!("transport: {t}")),
    }
}

fn build_agent(cfg: &Config) -> anyhow::Result<ureq::Agent> {
    let mut builder = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(cfg.network_client.connect_timeout_secs))
        .timeout_read(Duration::from_secs(cfg.network_client.request_timeout_secs));

    if cfg.server.tls_skip_verify {
        let tls = ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerifier))
        .with_no_client_auth();
        builder = builder.tls_config(Arc::new(tls));
        log::warn!("已启用 tls_skip_verify（跳过证书校验，仅应急使用）");
    } else if !cfg.server.ca_bundle_path.is_empty() {
        let roots = load_ca_bundle(&cfg.server.ca_bundle_path)?;
        let tls = ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
        builder = builder.tls_config(Arc::new(tls));
    }
    // 否则用 ureq 默认（rustls + webpki-roots）
    Ok(builder.build())
}

fn load_ca_bundle(path: &str) -> anyhow::Result<RootCertStore> {
    use rustls::pki_types::pem::PemObject;
    let f = std::fs::File::open(path)?;
    let mut reader = BufReader::new(f);
    let mut roots = RootCertStore::empty();
    let mut added = 0;
    for cert in CertificateDer::pem_reader_iter(&mut reader) {
        roots.add(cert?)?;
        added += 1;
    }
    if added == 0 {
        anyhow::bail!("CA 文件 {path} 未解析出任何证书");
    }
    Ok(roots)
}

const SUPPORTED_SCHEMES: &[SignatureScheme] = &[
    SignatureScheme::RSA_PKCS1_SHA256,
    SignatureScheme::RSA_PKCS1_SHA384,
    SignatureScheme::RSA_PKCS1_SHA512,
    SignatureScheme::ECDSA_NISTP256_SHA256,
    SignatureScheme::ECDSA_NISTP384_SHA384,
    SignatureScheme::ECDSA_NISTP521_SHA512,
    SignatureScheme::RSA_PSS_SHA256,
    SignatureScheme::RSA_PSS_SHA384,
    SignatureScheme::RSA_PSS_SHA512,
    SignatureScheme::ED25519,
];

/// 应急用：跳过所有证书校验（仅 tls_skip_verify=true 时使用）。
#[derive(Debug)]
struct NoVerifier;

impl ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        SUPPORTED_SCHEMES.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_status() {
        assert!(matches!(classify_http_status(200), PostOutcome::Ok));
        assert!(matches!(classify_http_status(204), PostOutcome::Ok));
        assert!(matches!(classify_http_status(401), PostOutcome::NoRetry(_)));
        assert!(matches!(classify_http_status(404), PostOutcome::NoRetry(_)));
        assert!(matches!(classify_http_status(500), PostOutcome::Retry(_)));
        assert!(matches!(classify_http_status(502), PostOutcome::Retry(_)));
    }

    #[test]
    fn build_default_agent() {
        let cfg = Config::default();
        assert!(Reporter::new(&cfg).is_ok(), "默认配置应能构建 reporter");
    }
}
