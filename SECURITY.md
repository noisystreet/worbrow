# SECURITY.md

## 漏洞上报

项目处于开发阶段，尚未开放正式安全响应渠道。发现问题请先**不要公开**：

- 通过 GitHub 私有漏洞报告提交：<https://github.com/noisystreet/worbrow/security/advisories/new>
- 或在仓库 issue 中使用 `[SECURITY]` 前缀，不要贴完整漏洞细节

## 安全边界（本项目已做/未做）

已做：

- 目标域白名单：仅访问已注册搜索引擎的域名（防 SSRF 面，design.md §9）
- 结果 URL 只输出、不访问；标题/摘要做清洗（design.md §10.3）
- 退出码/错误 JSON 契约稳定，错误信息不泄漏内部路径细节

未做（设计取舍）：

- 不绕过验证码/反爬（只检测上报）
- 不承诺规避目标站点 ToS 风控

## 依赖安全

CI 运行 `cargo-deny`（漏洞 advisory deny 级别）。新增依赖前确认许可白名单（`deny.toml`）。
