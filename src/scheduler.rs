//! 调度循环：周期采集 + 上报，SIGTERM/SIGINT 优雅退出。

use crate::collector::Collector;
use crate::config::Config;
use crate::reporter::Reporter;
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::flag;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub fn run(cfg: Config) -> anyhow::Result<()> {
    let mut collector = Collector::new(&cfg)?;
    let mut reporter = Reporter::new(&cfg)?;

    let stop = Arc::new(AtomicBool::new(false));
    flag::register(SIGTERM, Arc::clone(&stop))?;
    flag::register(SIGINT, Arc::clone(&stop))?;

    collector.warmup()?;
    log::info!(
        "monitor-agent 启动，采集间隔 {} 分钟，上报至 {}",
        cfg.agent.interval_minutes,
        cfg.server.url
    );

    let interval = Duration::from_secs(cfg.agent.interval_minutes * 60);
    loop {
        sleep_interruptible(&interval, &stop);
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let snapshot = collector.collect();
        reporter.send(&snapshot);
    }
    log::info!("monitor-agent 收到退出信号，已停止");
    Ok(())
}

/// 分段 sleep，每秒检查停止标志，收到信号即返回。
fn sleep_interruptible(d: &Duration, stop: &AtomicBool) {
    let step = Duration::from_secs(1);
    let mut remaining = *d;
    while remaining > Duration::ZERO {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let s = step.min(remaining);
        std::thread::sleep(s);
        remaining = remaining.saturating_sub(s);
    }
}
