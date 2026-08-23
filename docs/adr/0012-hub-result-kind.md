# ADR-012：枢纽页 `result_kind=hub` 不计入内容型结果

- 状态：已接受
- 日期：2026-08-23

## 背景

词典/翻译门禁与词面相关性门禁（roadmap-result-quality.md P1–P3）之后，DuckDuckGo
HTTP SERP 仍会「高产通过」：8 条均为 `web`，标题里有「新闻/基金」，但 URL 是门户
首页或频道页（今日头条 `/`、12306 `/`、热榜 `/c/news`）。内容型数量与 overlap
都达标，降级链不会尝试百度。

站点级域名黑名单已否决（维护成本高、误伤面大）。

## 决策

- `ResultKind` 新增变体 `Hub`（schema v1 只增不改，序列化 `"hub"`）
- 识别只看 **URL 形态**：空路径、剥 locale/`index.html` 后为空、或剩余段全是通用
  频道名（`news`/`data`/`c` 等）；文章 id、文档扩展名、GitHub 仓库路径回退 `Web`
- **不做域名黑名单**
- `content_count` 只计 `Web`；枢纽占比高则视为不满意，走既有引擎降级
- 截断 `max_results` 前按 `Web` → `Hub` → 词典/翻译稳定重排

### 明确不做

- 本地语义打分、查询自动改写
- 搜索结果自动 `fetch`（硬约束 6）
- 把百度/Bing SERP 改成 HTTP 直抓（ADR-011）

## 后果

- **得到**：门户首页簇会降级到下一引擎；agent 可按 `result_kind` 过滤 `hub`
- **付出**：浅频道页可能被标 `hub`（误判代价 = 多试一个引擎或排序靠后）
- **测试**：extract 用实搜 URL 样本；app 枢纽 HTML → 降级 DuckDuckGo

## 备选

- 仅空路径算枢纽：挡不住 `/c/news`、`/hotnews/`，否决
- 域名黑名单：与既有非目标冲突，否决
