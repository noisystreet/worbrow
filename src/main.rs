//! 薄入口：解析 CLI → 初始化日志 → 执行用例 → 输出 JSON → 映射退出码（design.md §6.1）。

use std::process::ExitCode;

use clap::Parser;
use worbrow::app::{self, Config};
use worbrow::cli::{BrowserArg, Cli, Command, LogLevelArg};
use worbrow::drivers::{self, BrowserKind};
use worbrow::engines;
use worbrow::error::Error;
use worbrow::output;

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
            #[cfg(feature = "mcp")]
            Command::Mcp { idle_timeout } => mcp_main(idle_timeout),
        };
    }

    let json = cli.json;

    let Some(query) = cli.query else {
        return finish(
            &Err(Error::Cli(
                "缺少搜索词（用法: worbrow \"<query>\" 或 worbrow doctor）".into(),
            )),
            json,
        );
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
            return finish(
                &Err(Error::Internal(format!("tokio runtime 初始化失败: {e}"))),
                json,
            );
        }
    };
    finish(&runtime.block_on(app::run(config)), json)
}

/// 输出收口：`--json` → 契约包；否则人读文本。退出码按 §7.2 映射。
fn finish(result: &Result<app::Outcome, Error>, json: bool) -> ExitCode {
    match result {
        Ok(outcome) => {
            let body = if json {
                output::success(&outcome.query, &outcome.results, &outcome.meta)
            } else {
                output::success_text(&outcome.query, &outcome.results, &outcome.meta)
            };
            println!("{body}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            let body = if json {
                output::failure(err)
            } else {
                output::failure_text(err)
            };
            println!("{body}");
            ExitCode::from(err.exit_code() as u8)
        }
    }
}

/// `worbrow doctor`：环境自检（design.md §10）。
fn doctor() -> ExitCode {
    println!("=== worbrow doctor ===");
    println!("引擎注册表:");
    for name in engines::AVAILABLE {
        println!("  - {name}");
    }
    println!("浏览器后端:");
    println!("  - fake: 可用（测试）");
    match drivers::discovery::find_browser(BrowserKind::Chrome) {
        Ok(p) => {
            let version = drivers::discovery::browser_major_version(&p)
                .map(|v| v.to_string())
                .unwrap_or_else(|| "未知".into());
            println!(
                "  - chrome (CDP): 已实现（V1），二进制: {}（主版本 {version}）",
                p.display()
            );
        }
        Err(e) => println!("  - chrome (CDP): 不可用 - {e}"),
    }
    match drivers::discovery::find_browser(BrowserKind::Firefox) {
        Ok(p) => {
            let version = drivers::discovery::browser_major_version(&p)
                .map(|v| v.to_string())
                .unwrap_or_else(|| "未知".into());
            println!(
                "  - firefox (Marionette): 已实现（V1），二进制: {}（主版本 {version}）",
                p.display()
            );
        }
        Err(e) => println!("  - firefox (Marionette): 不可用 - {e}"),
    }
    println!("=== done ===");
    ExitCode::SUCCESS
}

/// `worbrow list`：列出可用引擎。
fn list_engines() -> ExitCode {
    for name in engines::AVAILABLE {
        println!("{name}");
    }
    ExitCode::SUCCESS
}

/// `worbrow mcp`：以 MCP stdio server 形态运行（docs/adr/0005-mcp-stdio-server.md）。
///
/// 与普通搜索不同：stdout 是 MCP JSON-RPC 通道，**不**走 `finish()` 输出契约包；
/// 工具结果经 MCP `tools/call` 响应返回。错误仅写 stderr + exit 1。
#[cfg(feature = "mcp")]
fn mcp_main(idle_timeout: u64) -> ExitCode {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("tokio runtime 初始化失败: {e}");
            return ExitCode::from(1);
        }
    };
    // 0 = 禁用空闲超时（保持"等客户端断开"语义）
    let idle = (idle_timeout > 0).then(|| std::time::Duration::from_secs(idle_timeout));
    match runtime.block_on(worbrow::mcp::serve_stdio(idle)) {
        Ok(()) => {
            // 正常结束（空闲超时/客户端 EOF）。必须显式 exit：tokio::io::Stdin 的内部
            // 阻塞读线程在管道无数据时永不退出，若走 main 正常返回会挂在 runtime drop
            // 上导致进程残留（实测 timeout 后仍存活）。
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("MCP server 退出: {e}");
            ExitCode::from(1)
        }
    }
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
