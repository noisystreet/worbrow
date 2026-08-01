# worbrow 结果质量信号与降级链增强规划

> 读者：项目维护者 / 贡献者。状态：**P1/P2 已落地（2026-08，P1 见 [roadmap.md](roadmap.md) ✅ 标记）**，随实施更新；落地决策按 AGENTS.md 记 ADR。
> 本文聚焦「搜索结果质量自检」，架构权威见 [design.md](design.md)，主功能路线见 [roadmap.md](roadmap.md)。

## 1. 背景与现状

真实案例（CrabMate 会话导出 `chat_export_20260801_135514.md`）暴露两类搜索质量问题：

| 查询 | 结果数量 | 可用结果 | 现象 |
|---|---|---|---|
| `Rust 教程 入门 学习资源 推荐`（中文） | 10 | 8 | 混入 2 条 Rust 游戏《腐蚀》结果（多义未消歧） |
| `best Rust tutorials 2024 learn Rust`（英文） | 10 | **0** | 10 条全为 "best" 词典释义（iciba/百度百科/剑桥词典） |
| `learn Rust book rustlings The Rust Programming Language tutorial`（英文） | 10 | 少量 | 再次被 "learn" 词典结果淹没（爱词霸/剑桥/欧路词典） |

根因分三层：

1. **引擎意图误判**：Bing 对含常见英文词（`best`/`learn`）的查询做"词典释义"意图判定，
   返回大量词典/翻译站点结果；中文市场（mkt）下该权重更高
2. **agent 查询词构造**：把 `best`/`learn` 等高噪声词堆进查询稀释信号（可文档化规避）
3. **降级链盲区（本规划核心）**：当前降级判定只看**数量**（[app.rs](../src/app.rs)
   `satisfied = results.len() >= LOW_YIELD_THRESHOLD`），10 条"高产低质"结果不会触发降级，
   这类失败永远走不到 DuckDuckGo

对比方案：`site:` 等查询约束（见 [roadmap-search-params.md](roadmap-search-params.md)）需要
agent **事先知道答案在哪个站**，污染发生时无从指定，不通用。本规划是引擎侧**自动识别
结果类型、自检质量**，对所有查询与引擎生效，不依赖 agent 输入。

## 2. 目标与非目标

### 目标

1. **结果类型识别**：解析层按 URL 特征标记每条结果为 `web` / `dictionary` / `translation`
   等类型（跨引擎通用，非域名黑名单）
2. **质量降级信号**：降级判定从"数量 ≥ 阈值"升级为"**内容型结果**数 ≥ 阈值"，高产低质
   自动尝试下一引擎
3. **契约可见性**（可选）：`result_kind` 暴露到输出，agent 可自行过滤词典/翻译噪声

### 非目标（明确不做）

- **站点级域名黑名单**：与 `site:` 同属"枚举具体站点"，维护成本高且误伤面大；特征库只含
  URL 路径模式（~10 条），正常内容页路径几乎不含这些词
- **语义相关性打分**：本地无 LM，靠 URL 特征 + 统计信号，不做内容语义判断
- **查询自动改写**：检测常见词并改写不可靠（词典意图权重因 mkt/时段浮动），留给 agent 侧
- **多引擎并行交叉验证**：成本翻倍且 DDG 对常见词同样可能被污染；顺序降级链 + 质量信号已够
- **绕过验证码/反爬**：遵守 AGENTS.md 安全红线，只检测上报

## 3. 方向与优先级

### P1：结果类型识别（`result_kind`）—— ✅ 已完成（2026-08）

| 项 | 内容 |
|---|---|
| 现状 | 解析层只产出 rank/title/url/snippet + `is_ad`/`url_resolved`，无"结果类型"概念 |
| 目标 | 每条结果标记类型：`web`（内容页，默认）/ `dictionary` / `translation`；识别失败回退 `web` |
| 改动点 | ① [extract.rs](../src/extract.rs) 新增 `result_kind(url) -> ResultKind`：按 URL 路径/主机特征模式匹配（如路径含 `word`/`dict`/`dictionary`/`danci`/`translate`/`翻译`/`词典` 等，~10 个模式，跨引擎共享）；② [domain.rs](../src/domain.rs) `SearchResult` 新增 `result_kind: ResultKind`（受控枚举，`serde` 序列化为字符串） |
| 契约影响 | 结果对象新增字段（schema v1 **只增不改**，允许）；`ResultKind` 枚举新增变体为非破坏性（未知类型回退 `web`） |
| 验证 | 特征库单测（真实污染 URL 样本：iciba `word?w=`、cambridge 词典路径、eudic `dicts/en/`、ichacha、fanyi.so）；引擎 fixture 断言类型标注 |
| 风险 | 特征误判：正常内容页路径偶含 `word`/`dict`（如 wordpress）→ 回退 `web` 兜底 + 特征模式加边界（路径段精确匹配，非子串） |

### P1：质量降级信号（app 层判定升级）—— ✅ 已完成（2026-08）

| 项 | 内容 |
|---|---|
| 现状 | [app.rs](../src/app.rs) 降级判定只看数量：`satisfied = results.len() >= max_results \|\| >= LOW_YIELD_THRESHOLD`；高产低质（10 条全词典）不降级 |
| 目标 | satisfied 条件改为"**内容型**结果数"（`result_kind == web`）≥ 阈值；词典/翻译类不计入；低质走既有降级链（保留候选继续尝试，全失败返回稳定错误码） |
| 改动点 | ① [app.rs](../src/app.rs)：`search_one` 返回值或 `SearchResult` 上统计 `web` 结果数，替换降级判定中的 `results.len()`；② 低质时 `meta.low_yield = true` 语义保持（结果可用但质量差 → 成功包 + 降级标志，不升级为失败） |
| 契约影响 | `meta.low_yield` 语义扩展（"数量低"→"内容型结果不足"），字段不变；错误码不变 |
| 验证 | 集成测试：fixture 全词典结果 → 触发降级（engine_tried 断言）、首引擎 8 内容型结果不降级（回归）、全低质保留最高产候选 |
| 风险 | 阈值与现有 `LOW_YIELD_THRESHOLD = 3` 对齐，不新增配置项（保持简单）；误判代价仅"多试一个引擎" |

### P2：质量信号扩展 —— ✅ 已完成（2026-08）

| 项 | 内容 |
|---|---|
| 现状 | 类型识别 + 数量阈值覆盖词典/翻译污染；多义混入（如 Rust 游戏）无法靠 URL 特征区分 |
| 目标 | ① 结果集**类型同质化**检测（如 web 占比 < 50% → 强低质信号）；② 同域名去重（重排时合并，防单一来源刷屏） |
| 改动点 | ① [app.rs](../src/app.rs) 降级判定叠加占比阈值；② 翻页聚合去重逻辑扩展域名级 |
| 契约影响 | 无新增字段（内部信号）；去重改变结果集需注意 `meta` 计数语义 |
| 验证 | fixture 构造同质化/同域名样本断言降级触发 |
| 风险 | 占比阈值调参（误伤多义但仍有价值的结果）；实施前用真实污染样本校准 |

## 4. 实施顺序

结果类型识别（`result_kind`）→ 质量降级信号 →（P2 同质化/同域名去重）

- 每步独立可验证、可回退（特征库单测 + 集成测试）
- 完成后同步 `doctor`（如需要）、README、CHANGELOG、[roadmap.md](roadmap.md) 主文档标记 ✅

## 5. 契约与约束（贯穿全程）

- `schema_version` 只增不改；`result_kind` 为结果对象**新增字段**（允许），无破坏性变更
- `meta.low_yield` / 错误码语义冻结（0/2/3/4/124/1）
- 特征库以**真实污染 URL 样本**为验收基准（Bing/DDG 词典结果形态稳定，跨引擎共享特征）
- 识别失败一律回退 `web`（尽力语义，不因特征误判丢结果）

## 6. 开放决策

1. **`result_kind` 是否暴露 schema**：暴露（agent 可自行过滤，只增不改允许）vs 仅内部统计
   （schema 零变化）→ 倾向暴露，agent 侧可用
2. **特征库形态**：URL 路径/主机**模式**（~10 条，跨引擎）vs 域名黑名单（易维护但枚举化）
   → 倾向模式匹配 + 回退 `web`；实施前用真实样本校准边界（wordpress 类误判）
3. **内容型阈值**：与 `LOW_YIELD_THRESHOLD = 3` 对齐（不新增配置）vs 独立阈值 → 倾向对齐
4. **多义混入（Rust 游戏类）**：URL 特征无法区分 → 倾向留给 agent 查询词消歧
   （README 搜索词建议章节），引擎不做内容语义判断
