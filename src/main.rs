//! 薄入口：解析 CLI → 初始化日志 → 执行用例 → 输出 JSON → 映射退出码（design.md §6.1）。

use std::process::ExitCode;

use clap::Parser;
use worbrow::BrowserKind;
use worbrow::app::{self, Config};
use worbrow::engines;
use worbrow::error::Error;
use worbrow::output;

use cli::{Cli, Command, LogLevelArg};

/// CLI 参数定义（bin 私有；lib 不暴露 clap 细节，公开面为库 API，见 ADR-006）。
mod cli;

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

    let config = Config::new(query, cli.engine, cli.browser.to_kind())
        .with_max_results(cli.max_results)
        .with_timeout(std::time::Duration::from_secs(cli.timeout))
        .with_screenshot(cli.screenshot)
        .with_dump_html(cli.dump_html)
        .with_lang(cli.lang)
        .with_region(cli.region)
        .with_pages(cli.pages);

    // 同步入口：内部管理 tokio runtime（CLI 保持薄封装）
    finish(&app::search(config), json)
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
    let report = app::DoctorReport::collect();
    println!("引擎注册表:");
    for name in &report.engines {
        println!("  - {name}");
    }
    println!("浏览器后端:");
    for backend in &report.backends {
        match backend.kind {
            BrowserKind::Fake => println!("  - fake: 可用（测试）"),
            BrowserKind::Chrome | BrowserKind::Firefox => {
                let label = match backend.kind {
                    BrowserKind::Chrome => "chrome (CDP)",
                    _ => "firefox (Marionette)",
                };
                match (&backend.binary, backend.major_version) {
                    (Some(p), Some(v)) => println!(
                        "  - {label}: 已实现（V1），二进制: {}（主版本 {v}）",
                        p.display()
                    ),
                    (Some(p), None) => {
                        println!("  - {label}: 已实现（V1），二进制: {}", p.display())
                    }
                    (None, _) => println!(
                        "  - {label}: 不可用 - {}",
                        backend.error.as_deref().unwrap_or("未知原因")
                    ),
                }
            }
        }
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
