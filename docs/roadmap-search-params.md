# worbrow 搜索参数补全规划（时间过滤 / 安全搜索 / 站点过滤）

> 读者：项目维护者 / 贡献者。状态：**规划草案**，随实施更新；落地决策按 AGENTS.md 记 ADR。
> 本文聚焦「搜索参数补全」，架构权威见 [design.md](design.md)，主功能路线见 [roadmap.md](roadmap.md)。

## 1. 背景与现状

同类开源项目对比（SearXNG / Brave Search MCP / googler / ddgr）显示：**时间过滤、安全搜索、
站点过滤是通用搜索工具的标准能力**，而 worbrow 目前缺失：

| 能力 | worbrow | SearXNG | Brave MCP | googler/ddgr |
|---|---|---|---|---|
| 时间过滤 | ❌ | ✅ day/week/month/year | ✅ freshness | ✅ -t d/w/m/y |
| 安全搜索 | ❌ | ✅ 0/1/2 | ✅ off/moderate/strict | ✅ --unsafe |
| 站点过滤 site: | ❌ | ✅ 语法 | — | ✅ -w |
| 文件类型 filetype: | ❌ | ✅ 语法 | — | ✅ |

已有基础：搜索参数增强 P1（`lang`/`region`/`pages` 翻页聚合，[roadmap.md](roadmap.md) ✅）确立了
「引擎适配器读 `SearchQuery` → 拼 URL 参数」的扩展模式，本规划四个参数沿用同一模式，纯增量。

## 2. 目标与非目标

### 目标

1. **时间过滤**（`--freshness day|week|month|year`）：agent 查时效内容（最新资讯/文档版本）
2. **安全搜索**（`--safesearch off|moderate|strict`）：agent 工作环境内容过滤
3. **站点过滤**（`--site <domain>`）：agent 定向查 docs.rs / github.com 等
4. **文件类型过滤**（`--filetype <ext>`）：agent 查 PDF 论文 / MD 文档 / JSON 配置样例

### 非目标（明确不做）

- **垂直搜索**（images/videos/news）：与「通用搜索 JSON 契约」定位不符（roadmap.md 非目标）
- **内容抓取**（Brave `llm_context` / Exa `get_contents`）：违反安全红线「不自动访问搜索结果
  第三方 URL（只输出）」（AGENTS.md 硬约束 6）
- **精确日期范围**（`--from/--to`，googler 支持）：复杂度高、收益低，先做相对时间
- **多引擎并行聚合**（元搜索，SearXNG 核心）：顺序降级链已满足容错，并行聚合契约语义变化大，
  另行评估

## 3. 方向与优先级

### P1：时间过滤（freshness）

| 项 | 内容 |
|---|---|
| 现状 | 无法限定结果时效；agent 查"最新"需自行过滤 |
| 目标 | `--freshness day\|week\|month\|year`（`None` = 不限时间，保持现行为） |
| 改动点 | ① [domain.rs](../src/domain.rs) `SearchQuery` 新增 `freshness: Option<Freshness>`（受控枚举，`Freshness::as_engine_param` 映射引擎参数）；② [bing.rs](../src/engines/bing.rs)：`qft=+filterui:age-lt<sec>`（day=86400 / week=604800 / month≈2592000 / year≈31536000）；③ [duckduckgo.rs](../src/engines/duckduckgo.rs)：`df=d\|w\|m\|y`；④ [cli.rs](../src/cli.rs)/[mcp.rs](../src/mcp.rs) 新增参数（`serde(default)` 向后兼容） |
| 契约影响 | 请求参数新增（CLI/MCP schema 只增）；**输出 schema v1 无变化**（结果即过滤后，meta 不回显） |
| 验证 | URL 模板单测（bing/ddg 各档位）；CLI/MCP 参数解析测试 |
| 风险 | Bing `qft` 时间参数可能改版 → 实施前实网验证 + fixture 同步（引擎改版纪律） |

### P1：安全搜索（safesearch）

| 项 | 内容 |
|---|---|
| 现状 | 无内容过滤；工作环境/合规场景不可用 |
| 目标 | `--safesearch off\|moderate\|strict`（`None` = 引擎默认，保持现行为） |
| 改动点 | ① [domain.rs](../src/domain.rs) `SearchQuery` 新增 `safesearch: Option<SafesearchLevel>`（三级枚举）；② [bing.rs](../src/engines/bing.rs)：`adlt=off\|strict`（Bing 仅两级，moderate 映射 strict）；③ [duckduckgo.rs](../src/engines/duckduckgo.rs)：`kp=-1\|1\|2`；④ CLI/MCP 参数 |
| 契约影响 | 请求参数新增（只增）；输出 schema 无变化 |
| 验证 | URL 模板单测（bing/ddg 三级映射）；参数解析测试 |
| 风险 | 低（参数成熟稳定） |

### P1：站点与文件类型过滤（site / filetype）

| 项 | 内容 |
|---|---|
| 现状 | agent 定向查站点/文件类型需手工在 query 拼 `site:` / `filetype:` |
| 目标 | `--site <domain>` 与 `--filetype <ext>`（单值；`None` = 不限定） |
| 改动点 | ① [domain.rs](../src/domain.rs) `SearchQuery` 新增 `site: Option<String>` 与 `filetype: Option<String>`；② 引擎适配器在发送 query 时追加 `site:<domain>` / `filetype:<ext>`（`{text} site:{site} filetype:{ft}`，query 级语法 Bing/DDG 均原生支持，零引擎适配改动）；③ CLI/MCP 参数 |
| 契约影响 | 请求参数新增（只增）；输出 `query` 字段**保留原始**（agent 已从参数知晓过滤条件，避免契约噪音） |
| 验证 | URL 模板单测（query 追加 `site:`/`filetype:`）；参数解析测试 |
| 风险 | 低（query 级语法，引擎天然支持；值不校验，非法值即空结果，符合"尽力"语义） |

## 4. 实施顺序

时间过滤 → 安全搜索 → 站点与文件类型过滤

- 每步独立可验证、可回退（URL 单测 + 参数解析测试）
- 完成后同步 `doctor`（如需要）、README、CHANGELOG、[roadmap.md](roadmap.md) 主文档标记 ✅

## 5. 契约与约束（贯穿全程）

- `schema_version` 只增不改；本组改动均为**请求参数**新增，输出 schema v1 完全无变化
- 退出码语义冻结（0/2/3/4/124/1）
- 引擎 URL 参数以**实网验证**为准（Bing 改版风险），失败走 `EngineFailure`（exit 4）不上报 schema
- 新增参数默认值保持现行为（`None`/引擎默认），对既有调用方零影响

## 6. 开放决策

1. **Bing 时间过滤参数形态**：`qft=+filterui:age-lt<sec>` 与 `filters=ex1:"ez5_…"` 两种实现
   并存 → 实施前实网验证取稳定者
2. **safesearch 三级 vs 两级**：Bing 无 moderate → 倾向三级对齐生态（moderate 映射 Bing strict）；
   也可退化为两级（off/strict）降低映射复杂度
3. **site/filetype 多值**：`--site a --site b`（`site:(a OR b)`）→ 倾向 V1 单值（简单、够用），
   多值留后续
4. **freshness 是否回显 `meta`**：倾向不加（结果即过滤后，meta 保持最小）；如 agent 需要审计
   可后续加（schema 只增不改允许）
