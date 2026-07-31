//! 端口定义：浏览器后端与搜索引擎适配器的统一接口（design.md §6.4）。
//!
//! 依赖方向：本模块属于内层，adapters 实现这里定义的 trait，不反向依赖。

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use url::Url;

use crate::domain::{SearchQuery, SearchResult};
use crate::error::{EngineFailure, Error};

/// 浏览器后端统一接口。
///
/// 测试用 `FakeDriver`、生产用 CDP（Chrome/Edge）或 Marionette（Firefox）实现；
/// 协议差异全部封在各自文件内（design.md §6.5）。
#[async_trait]
pub trait BrowserDriver: Send + Sync {
    /// 导航到 URL 并等待首屏。
    async fn navigate(&mut self, url: Url) -> Result<(), Error>;
    /// 等待选择器匹配的元素出现，超时返回 `Error::Timeout`。
    async fn wait_for(&mut self, selector: &str, timeout: Duration) -> Result<(), Error>;
    /// 取当前页面 HTML。
    async fn html(&self) -> Result<String, Error>;
    /// 执行 JS（读取结构化数据、验证码判定等）。
    async fn eval(&mut self, js: &str) -> Result<serde_json::Value, Error>;
    /// 保存页面截图（调试）。
    async fn screenshot(&mut self, path: &Path) -> Result<(), Error>;
}

/// 搜索引擎适配器统一接口。
pub trait SearchProvider: Send + Sync {
    fn name(&self) -> &'static str;
    /// URL 直访模板（design.md ADR-003）。
    fn result_url(&self, q: &SearchQuery) -> Url;
    /// 结果容器选择器（等待加载用）。
    fn result_selector(&self) -> &'static str;
    /// 从页面 HTML 抽取结果；失败返回 `EngineFailure`（由 app 上报为 exit 4）。
    fn parse(&self, html: &str) -> Result<Vec<SearchResult>, EngineFailure>;
    /// 验证码特征词/选择器启发式。
    fn captcha_heuristics(&self) -> &[&'static str];
}
