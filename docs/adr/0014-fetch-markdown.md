# ADR-014：fetch 正文 Markdown 输出（`--format markdown`）

- 状态：已接受
- 日期：2026-08-31

## 背景

`fetch_page` / `worbrow fetch` 只返回清洗后的**纯文本**（`extract_main_text`，ADR-009）。
agent 生态的文档/网页投喂标准格式是 **Markdown**（Firecrawl、Jina Reader、fetch MCP、
Crawl4AI 均默认输出 markdown）：保留标题层级、链接、列表与代码块结构，模型按结构
消费而非逐段猜测。纯文本丢失了这些信号，agent 做"引用来源链接""按标题定位章节"
类任务时需要二次解析。

## 决策

- 新增 **`--format text|markdown`**（CLI fetch 子命令）+ `FetchConfig::with_format`
  （lib，`FetchTextFormat` 枚举）+ MCP `fetch_page` 的 `format` 参数（缺省 `text`）
- **提取器按格式分流**：`text` → 既有 `extract_main_text`；`markdown` → 新增
  `extract_markdown`（`extract.rs`，scraper 手写转换，**不引入新依赖**）
- **Markdown 转换范围**（尽力语义，`article`/`main` 回退 `body`、跳过噪音容器，与
  纯文本同一容器策略）：`h1-h6` → `#` 标题、`a` → `[text](href)`（锚点/空链接降级为
  纯文本）、`ul/ol/li` → `- ` 列表（嵌套缩进）、`blockquote` → `> `、`pre` → ``` ``` ```、
  `strong/em/code/img/br` → 行内标记；**表格扁平化为行内文本**（放弃结构、保留内容）
- **契约**：schema v1 零变化（`FetchedPage.text` 按格式承载内容，`chars`/`truncated`
  语义不变；`format` 为请求参数）。退出码不变
- 非法 `format` 值：CLI 在 clap 解析期拒绝（exit 2）；MCP 工具级 `isError`

### 明确不做

- 引入 `html2md` 等第三方转换库（项目刚做过依赖裁剪；scraper 手写 ~150 行覆盖常用
  结构，代价可控且无传递依赖风险）
- 表格结构化（`|` 管线）渲染：复杂对齐/合并单元格场景收益低，先扁平化
- 让 `search` 结果也出 markdown（搜索结果已是结构化 DTO，无此需求）
- 改变 `extract_main_text` 的默认行为（`text` 仍是默认，向后兼容）

## 后果

- **得到**：agent 直接拿到结构化 markdown（标题/链接/列表/代码块），生态对齐
  Firecrawl/Jina 等主流抓取工具；无需新依赖
- **付出**：转换器是"尽力"实现，非常规页面（复杂表格/嵌套样式）结构会降级为
  行内文本；`chars` 统计的是 markdown 字符数（比纯文本多出标记字符）
- **测试**：`extract_markdown` 单测（结构保留/噪音剥离/截断/空页）+ app 集成
  （format=markdown 经 FakeDriver）+ MCP 集成（成功包含链接结构、非法 format isError）+
  CLI（非法 format exit 2）
- 真机验证：`worbrow fetch --format markdown <url>` 本地比对输出（CI 外）

## 备选

- 引入 `html2md`：少写代码但新增依赖树、与既有 scraper 双解析并存，否决
- 基于 `extract_main_text` 的文本后处理（正则恢复结构）：不可靠，否决
- 默认改为 markdown：破坏既有 `text` 消费方（schema 未变但内容形态变），否决
