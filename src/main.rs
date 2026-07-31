//! 薄入口：解析 CLI → 初始化日志 → 执行用例 → 输出 JSON → 映射退出码（design.md §6.1）。

use std::process::ExitCode;

use clap::Parser;
use rplay_search::app::{self, Config};
use rplay_search::cli::{BrowserArg, Cli, Command, LogLevelArg};
use rplay_search::drivers::{self, BrowserKind};
use rplay_search::engines;
use rplay_search::error::Error;
use rplay_search::output;

fn main() -> ExitCode {
    // panic 兜底：任何 panic 转 exit 1（design.md §6.1）
    match std::panic::catch_unwind(run_cli) {
        Ok(code) => code,
        Err(_) => {
            eprintln!("内部错误: 发生 panic，请上报");
            ExitCode::from(1)
        }
    }
}

fn run_cli() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.log_level);

    // 子命令优先（无需搜索词）
    if let Some(cmd) = cli.command {
        return match cmd {
            Command::Doctor => doctor(),
            Command::List => list_engines(),
        };
    }

    let Some(query) = cli.query else {
        return finish(&Err(Error::Cli(
            "缺少搜索词（用法: search \"<query>\" 或 search doctor）".into(),
        )));
    };

    let config = Config {
        query,
        engine: cli.engine.name().to_string(),
        browser: match cli.browser {
            BrowserArg::Chrome => BrowserKind::Chrome,
            BrowserArg::Firefox => BrowserKind::Firefox,
        },
        max_results: cli.max_results,
        timeout: std::time::Duration::from_secs(cli.timeout),
        screenshot: cli.screenshot,
        dump_html: cli.dump_html,
        driver: None,
    };

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            return finish(&Err(Error::Internal(format!(
                "tokio runtime 初始化失败: {e}"
            ))));
        }
    };
    finish(&runtime.block_on(app::run(config)))
}

/// 输出契约收口：成功包/失败包 → stdout，退出码按 §7.2 映射。
fn finish(result: &Result<app::Outcome, Error>) -> ExitCode {
    match result {
        Ok(outcome) => {
            println!(
                "{}",
                output::success(&outcome.query, &outcome.results, &outcome.meta)
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            println!("{}", output::failure(err));
            ExitCode::from(err.exit_code() as u8)
        }
    }
}

/// `search doctor`：环境自检（design.md §10）。
fn doctor() -> ExitCode {
    println!("=== search doctor ===");
    println!("引擎注册表:");
    for name in engines::AVAILABLE {
        println!("  - {name}");
    }
    println!("浏览器后端:");
    println!("  - fake: 可用（测试）");
    println!("  - chrome (CDP): 待实现（V1）");
    match drivers::discovery::find_browser(BrowserKind::Firefox) {
        Ok(p) => println!(
            "  - firefox (Marionette): 已实现（V1），二进制: {}",
            p.display()
        ),
        Err(e) => println!("  - firefox (Marionette): 不可用 - {e}"),
    }
    println!("=== done ===");
    ExitCode::SUCCESS
}

/// `search list`：列出可用引擎。
fn list_engines() -> ExitCode {
    for name in engines::AVAILABLE {
        println!("{name}");
    }
    ExitCode::SUCCESS
}

/// 初始化 stderr 日志（默认 off，避免污染 stdout 契约）。
fn init_tracing(level: LogLevelArg) {
    let level = match level {
        LogLevelArg::Off => return,
        LogLevelArg::Error => "error",
        LogLevelArg::Warn => "warn",
        LogLevelArg::Info => "info",
        LogLevelArg::Debug => "debug",
        LogLevelArg::Trace => "trace",
    };
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::new(level))
        .init();
}
