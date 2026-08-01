//! 基本搜索示例：同步入口 + builder 配置。
//!
//! 运行：`cargo run --example basic_search`
//!
//! 说明：使用 fake 浏览器后端（`BrowserKind::Fake`，无需真实浏览器），
//! 展示库 API 的典型用法；生产环境将 `Fake` 换为 `Firefox`/`Chrome` 即可。

use worbrow::{BrowserKind, Config, search};

fn main() -> Result<(), worbrow::Error> {
    let config = Config::new("rust async runtime", "bing", BrowserKind::Fake).with_max_results(5);
    let outcome = search(config)?;

    for r in &outcome.results {
        println!("{}. {}\n   {}\n   {}", r.rank, r.title, r.url, r.snippet);
    }
    println!(
        "engine: {}  results: {}  elapsed: {}ms",
        outcome.meta.engine, outcome.meta.result_count, outcome.meta.elapsed_ms
    );
    Ok(())
}
