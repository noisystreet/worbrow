//! 测试用浏览器后端：返回预设 HTML，CI 无需真实浏览器（design.md §6.5 / §11）。
//!
//! `drivers::resolve(BrowserKind::Fake)` 返回带 [`SMOKE_HTML`] 的实例，使
//! `browser=fake`（CLI 不暴露，MCP `search` 工具可用）无需真实浏览器即可产出
//! 可解析的模拟结果；测试需要定制页面时用 [`FakeDriver::with_html`] 显式注入。

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use url::Url;

use crate::error::Error;
use crate::ports::BrowserDriver;

/// 冒烟预设结果页（DDG html 端点结构，含 3 条结果：3 ≥ 低产量阈值）。
pub const SMOKE_HTML: &str = r#"<!DOCTYPE html>
<html lang="zh">
<head><meta charset="utf-8"><title>DuckDuckGo Search</title></head>
<body>
  <div class="result">
    <h2 class="result__title"><a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Frust">Rust 程序设计语言（示例标题一）</a></h2>
    <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Frust">这是摘要一</a>
  </div>
  <div class="result">
    <h2 class="result__title"><a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fasync">异步运行时 对比（示例标题二）</a></h2>
    <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fasync">这是摘要二</a>
  </div>
  <div class="result">
    <h2 class="result__title"><a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.org%2Ftokio">Tokio 与 async-std 对比（示例标题三）</a></h2>
    <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.org%2Ftokio">这是摘要三</a>
  </div>
</body>
</html>
"#;

/// 固定返回预设 HTML 的假驱动。
#[derive(Debug, Default)]
pub struct FakeDriver {
    html: String,
}

impl FakeDriver {
    pub fn with_html(html: impl Into<String>) -> Self {
        Self { html: html.into() }
    }
}

#[async_trait]
impl BrowserDriver for FakeDriver {
    async fn navigate(&mut self, _url: Url) -> Result<(), Error> {
        Ok(())
    }

    async fn wait_for(&mut self, _selector: &str, _timeout: Duration) -> Result<(), Error> {
        Ok(())
    }

    async fn html(&self) -> Result<String, Error> {
        Ok(self.html.clone())
    }

    async fn eval(&mut self, _js: &str) -> Result<serde_json::Value, Error> {
        Ok(serde_json::Value::Null)
    }

    async fn screenshot(&mut self, _path: &Path) -> Result<(), Error> {
        Ok(())
    }
}
