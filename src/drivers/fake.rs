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
///
/// `eval` 模拟真实浏览器语义：`document.readyState` 返回 `complete`（fetch 等待加载
/// 立即通过）、`location.href` 返回最近导航 URL（fetch `final_url`）；其余 JS 返回 Null。
#[derive(Debug, Default)]
pub struct FakeDriver {
    html: String,
    /// 最近一次 navigate 的 URL（供 `location.href` eval）。
    current_url: Option<String>,
}

impl FakeDriver {
    pub fn with_html(html: impl Into<String>) -> Self {
        Self {
            html: html.into(),
            current_url: None,
        }
    }
}

#[async_trait]
impl BrowserDriver for FakeDriver {
    async fn navigate(&mut self, url: Url) -> Result<(), Error> {
        self.current_url = Some(url.to_string());
        Ok(())
    }

    async fn wait_for(&mut self, _selector: &str, _timeout: Duration) -> Result<(), Error> {
        Ok(())
    }

    async fn html(&self) -> Result<String, Error> {
        Ok(self.html.clone())
    }

    async fn eval(&mut self, js: &str) -> Result<serde_json::Value, Error> {
        if js.contains("readyState") {
            return Ok(serde_json::Value::String("complete".into()));
        }
        if js.contains("location.href") {
            return Ok(serde_json::Value::String(
                self.current_url.clone().unwrap_or_default(),
            ));
        }
        Ok(serde_json::Value::Null)
    }

    async fn screenshot(&mut self, _path: &Path) -> Result<(), Error> {
        Ok(())
    }
}
