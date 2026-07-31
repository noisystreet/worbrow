//! 测试用浏览器后端：返回预设 HTML，CI 无需真实浏览器（design.md §6.5 / §11）。

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use url::Url;

use crate::error::Error;
use crate::ports::BrowserDriver;

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
