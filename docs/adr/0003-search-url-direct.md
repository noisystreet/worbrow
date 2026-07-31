# ADR-003：搜索方式 = URL 直访优先，交互原语备用

- 状态：已接受
- 日期：2026-07-31

## 背景

在搜索引擎上发起搜索有两种方式：直接构造搜索结果页 URL，或"填框 + 回车"模拟表单交互。

## 决策

- v1 一律 **URL 直访**：通用搜索引擎（Bing/DuckDuckGo/百度）都支持
  `https://<host>/search?q=<query>`
- V3 引入站内搜索时再扩展 `BrowserDriver` trait（`fill_input` / `click` / `press_key`
  原语），当前不预留空方法
- 查询词编码用 `url::form_urlencoded`；引擎 URL 模板集中在各引擎适配器

## 后果

- **得到**：实现简单、跨引擎行为一致、失败点少（少 2~3 个易碎步骤：选择器、焦点、按键事件）
- **付出**：个别引擎（Google）对纯 URL 直访的风控更严
