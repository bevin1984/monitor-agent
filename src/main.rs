#![allow(dead_code)]

mod collector;
mod config;
mod reporter;
mod scheduler;

use clap::Parser;
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(version, about = "Lightweight server resource monitoring agent")]
struct Args {
    /// 配置文件路径
    #[arg(long, default_value = "/etc/monitor-agent/config.toml")]
    config: String,
    /// 采集一次并把 JSON 打印到 stdout（不推送），用于调试与对接
    #[arg(long)]
    once: bool,
    /// 提高日志级别（-v info，-vv debug，-vvv trace）
    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    verbose: u8,
}

fn main() -> ExitCode {
    let args = Args::parse();
    init_log(args.verbose);
    if args.once {
        run_once(&args.config)
    } else {
        run_daemon(&args.config)
    }
}

fn init_log(verbose: u8) {
    let level = match verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    let env = env_logger::Env::default().default_filter_or(level);
    let _ = env_logger::Builder::from_env(env)
        .format_timestamp_secs()
        .try_init();
}

fn run_once(config_path: &str) -> ExitCode {
    let cfg = match config::Config::load(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("加载配置失败: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    let mut collector = match collector::Collector::new(&cfg) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("初始化采集器失败: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    let snapshot = collector.collect();
    match serde_json::to_string_pretty(&snapshot) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("序列化失败: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run_daemon(config_path: &str) -> ExitCode {
    let cfg = match config::Config::load(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("加载配置失败: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    // 启动校验：常驻模式必须配置真实 key
    let key = cfg.server.key.trim();
    if key.is_empty() || key == "CHANGE_ME" {
        eprintln!(
            "server.key 未设置（仍为 CHANGE_ME 或空），拒绝运行。\
             请在配置文件或环境变量 MONITOR_AGENT_KEY 中设置。"
        );
        return ExitCode::FAILURE;
    }
    match scheduler::run(cfg) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("运行失败: {e:#}");
            ExitCode::FAILURE
        }
    }
}
