//! 测试用浏览器后端：返回预设 HTML，CI 无需真实浏览器（design.md §6.5 / §11）。
//!
//! `drivers::resolve(BrowserKind::Fake)` 返回带 [`SMOKE_HTML`] 的实例，使
//! `browser=fake`（CLI 不暴露，MCP `web_search` 工具可用）无需真实浏览器即可产出
//! 可解析的模拟结果；测试需要定制页面时用 [`FakeDriver::with_html`] 显式注入。

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use url::Url;

use crate::error::Error;
use crate::ports::BrowserDriver;

/// 冒烟预设结果页（Bing 结构，含 3 条结果：3 ≥ 低产量阈值）。
pub const SMOKE_HTML: &str = include_str!("../../tests/fixtures/bing.html");

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
