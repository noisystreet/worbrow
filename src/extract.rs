//! 链接归一化与文本清洗（design.md §6.5 公共工具，供各引擎适配器复用）。

use base64::Engine as _;
use scraper::{Html, Selector};
use url::Url;

use crate::domain::ResultKind;

/// 结果类型特征库：URL 路径/主机模式 → `ResultKind`（roadmap-result-quality.md）。
///
/// 词典/翻译污染（如 Bing 对 `best`/`learn` 查询返回的释义页）识别失败一律回退
/// `Web`（尽力语义，不因误判丢结果）。路径段**精确匹配**（非子串），避免
/// wordpress 类正常站点误判；主机模式用**前缀**（`fanyi.`/`translate.`），避免
/// 普通域名中偶含特征词（如 notfanyiso.example.com）被误判。
pub fn result_kind(raw: &str) -> ResultKind {
    let Ok(url) = Url::parse(raw) else {
        return ResultKind::Web;
    };
    let host = url.host_str().unwrap_or_default();
    // 路径段（去空段），供精确匹配
    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|seg| !seg.is_empty()).collect())
        .unwrap_or_default();

    // 翻译站：host 前缀 fanyi.（fanyi.baidu.com / fanyi.so）或 translate.
    //（translate.google.com / translate.yandex.com），或路径段为 translate*/fanyi
    if host.starts_with("fanyi.")
        || host.starts_with("translate.")
        || segments
            .iter()
            .any(|seg| matches!(*seg, "translate" | "translation" | "fanyi"))
    {
        return ResultKind::Translation;
    }
    // 词典站：host 以 dictionary. 开头（dictionary.cambridge.org），或路径段为
    // dict*/word/danci（iciba `word`、eudic `dicts` 等）
    if host.starts_with("dictionary.")
        || segments
            .iter()
            .any(|seg| matches!(*seg, "dict" | "dicts" | "dictionary" | "word" | "danci"))
    {
        return ResultKind::Dictionary;
    }
    ResultKind::Web
}

/// 清洗标题/摘要：剥离控制字符、折叠多余空白（HTML 实体已由 scraper 解码）。
pub fn clean_text(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 归一化结果链接：补齐协议相对链接、展开跳转参数（uddg / Bing ck/a）、去 fragment。
///
/// 返回 `(url, resolved)`：`resolved=true` 表示发生了跳转链展开（uddg/ck-a 解码成功），
/// `url` 已尽力解为真实目标；`false` 表示原样返回（含 ck/a 解码失败保持链式 URL），
/// 供 `SearchResult.url_resolved` 标记。
pub fn normalize_url(raw: &str) -> (String, bool) {
    // DuckDuckGo html 版使用协议相对链接
    let raw = if let Some(rest) = raw.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        raw.to_string()
    };
    let Ok(mut url) = Url::parse(&raw) else {
        return (raw, false);
    };
    // 展开 DDG 跳转参数 uddg（真实目标）
    if let Some((_, target)) = url
        .query_pairs()
        .find(|(k, _)| k == "uddg")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
    {
        let (target, _) = normalize_url(&target);
        return (target, true);
    }
    // 展开 Bing 点击追踪链 ck/a（解码失败保持原样）
    if let Some(expanded) = expand_bing_ck(&url) {
        return (expanded, true);
    }
    url.set_fragment(None);
    (url.to_string(), false)
}

/// 展开 Bing 点击追踪链（`www.bing.com/ck/a`）：`u` 参数为 base64url（可能带 `a1`
/// 前缀）编码的目标 URL。解码失败（无 `u`/非法 base64/非 http(s)）返回 `None`。
fn expand_bing_ck(url: &Url) -> Option<String> {
    if !matches!(url.host_str(), Some("www.bing.com" | "bing.com")) || url.path() != "/ck/a" {
        return None;
    }
    let u = url.query_pairs().find(|(k, _)| k == "u")?.1.into_owned();
    // URL-safe base64 字符集（`-`/`_`）+ 可能省略 padding → 统一为标准 base64 再解码
    let mut b64 = u.strip_prefix("a1").unwrap_or(&u).to_string();
    b64 = b64.replace('-', "+").replace('_', "/");
    b64.push_str(&"=".repeat((4 - b64.len() % 4) % 4));
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.as_bytes())
        .ok()?;
    let target = String::from_utf8(bytes).ok()?;
    let mut parsed = Url::parse(&target).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    parsed.set_fragment(None);
    Some(parsed.to_string())
}

/// 提取 URL 的来源域名与 https 标志（供 `SearchResult.domain/https` 填充；
/// 非法 URL 返回空域名 + 非 https）。
pub fn url_origin(raw: &str) -> (String, bool) {
    match Url::parse(raw) {
        Ok(url) => (
            url.host_str().unwrap_or_default().to_string(),
            url.scheme() == "https",
        ),
        Err(_) => (String::new(), false),
    }
}

// ==== 正文抓取与结构化提取（fetch_page / `worbrow fetch`，ADR-009）====

/// 正文噪音元素标签：提取正文时跳过其文本（含祖先链判断）。
const NOISE_TAGS: [&str; 10] = [
    "script", "style", "noscript", "nav", "footer", "header", "aside", "form", "iframe", "template",
];

/// 从 HTML 提取清洗后的正文文本（尽力语义，非 Readability 级评分提取）。
///
/// 优先 `article`/`main` 容器，回退 `body`；跳过噪音标签（script/style/nav 等）的文本；
/// 复用 [`clean_text`] 折叠空白，按 `max_chars` 截断。返回 `(正文, 是否截断)`。
pub fn extract_main_text(html: &str, max_chars: usize) -> (String, bool) {
    let doc = Html::parse_document(html);
    let Some(root) = select_first(&doc, "article, main").or_else(|| select_first(&doc, "body"))
    else {
        return (String::new(), false);
    };
    let mut out = String::new();
    for node in root.descendants() {
        let Some(text) = node.value().as_text() else {
            continue;
        };
        // 噪音容器（含祖先链）内的文本跳过；script/style 的 text 节点因此不会混入
        if node.ancestors().any(|a| {
            a.value()
                .as_element()
                .is_some_and(|el| NOISE_TAGS.contains(&el.name()))
        }) {
            continue;
        }
        out.push_str(text);
        out.push(' ');
    }
    let cleaned = clean_text(&out);
    if cleaned.chars().count() <= max_chars {
        return (cleaned, false);
    }
    (cleaned.chars().take(max_chars).collect(), true)
}

/// 从 HTML 提取结构化字段（allowlist；缺失字段缺省，绝不编造）。
///
/// 提取优先级：JSON-LD（`application/ld+json`）→ meta（og:/twitter:/article:/product:）→
/// DOM 启发式（title）。值保留 JSON 原生类型（price 字符串 / rating 数字）。
pub fn extract_fields(
    html: &str,
    fields: &[crate::domain::ExtractField],
) -> serde_json::Map<String, serde_json::Value> {
    let doc = Html::parse_document(html);
    let ld = parse_ld_json(&doc);
    let mut out = serde_json::Map::new();
    for f in fields {
        if let Some(v) = extract_field(&doc, &ld, *f) {
            out.insert(f.as_str().to_string(), v);
        }
    }
    out
}

/// 单字段提取（返回原始 `Value`：JSON-LD 值保原生类型，meta 值为字符串）。
fn extract_field(
    doc: &Html,
    ld: &[serde_json::Value],
    f: crate::domain::ExtractField,
) -> Option<serde_json::Value> {
    use crate::domain::ExtractField as F;
    match f {
        F::Title => meta_content(
            doc,
            &["meta[property='og:title']", "meta[name='twitter:title']"],
        )
        .or_else(|| title_text(doc))
        .map(serde_json::Value::String),
        F::Author => meta_content(
            doc,
            &["meta[name='author']", "meta[property='article:author']"],
        )
        .or_else(|| ld_string(ld, "author"))
        .map(serde_json::Value::String),
        F::PublishedAt => meta_content(
            doc,
            &[
                "meta[property='article:published_time']",
                "meta[property='og:article:published_time']",
            ],
        )
        .or_else(|| ld_string(ld, "datePublished"))
        .map(serde_json::Value::String),
        F::Price => meta_content(
            doc,
            &[
                "meta[property='product:price:amount']",
                "meta[property='og:price:amount']",
            ],
        )
        .map(serde_json::Value::String)
        .or_else(|| ld_value(ld, "price")),
        F::Currency => meta_content(
            doc,
            &[
                "meta[property='product:price:currency']",
                "meta[property='og:price:currency']",
            ],
        )
        .map(serde_json::Value::String)
        .or_else(|| ld_string(ld, "priceCurrency").map(serde_json::Value::String)),
        // rating 系无标准 meta，仅 JSON-LD（AggregateRating.ratingValue/bestRating/reviewCount）
        F::Rating => ld_value(ld, "ratingValue"),
        F::RatingMax => ld_value(ld, "bestRating"),
        F::ReviewsCount => ld_value(ld, "reviewCount"),
    }
}

/// 解析页面全部 JSON-LD 块（`<script type="application/ld+json">`）为 JSON 值。
fn parse_ld_json(doc: &Html) -> Vec<serde_json::Value> {
    let Ok(sel) = Selector::parse("script[type='application/ld+json']") else {
        return Vec::new();
    };
    doc.select(&sel)
        .filter_map(|el| {
            el.text()
                .collect::<String>()
                .trim()
                .parse::<serde_json::Value>()
                .ok()
        })
        .collect()
}

/// 递归搜索 JSON-LD 中首个指定 key 的原始值（保留 JSON 原生类型）。
fn ld_value(ld: &[serde_json::Value], key: &str) -> Option<serde_json::Value> {
    fn find(v: &serde_json::Value, key: &str) -> Option<serde_json::Value> {
        match v {
            serde_json::Value::Object(map) => {
                if let Some(v) = map.get(key) {
                    return Some(v.clone());
                }
                map.values().find_map(|v| find(v, key))
            }
            serde_json::Value::Array(arr) => arr.iter().find_map(|v| find(v, key)),
            _ => None,
        }
    }
    ld.iter().find_map(|v| find(v, key))
}

/// JSON-LD 中首个指定 key 的字符串（对象型值取其 `name`，如 `author`）。
fn ld_string(ld: &[serde_json::Value], key: &str) -> Option<String> {
    ld_value(ld, key).and_then(|v| match v {
        serde_json::Value::String(s) => Some(s),
        serde_json::Value::Object(map) => map
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        _ => None,
    })
}

/// 首个非空匹配 meta 的 `content`（按选择器顺序）。
fn meta_content(doc: &Html, selectors: &[&str]) -> Option<String> {
    for s in selectors {
        let Ok(sel) = Selector::parse(s) else {
            continue;
        };
        if let Some(el) = doc.select(&sel).next()
            && let Some(content) = el.value().attr("content")
            && !content.trim().is_empty()
        {
            return Some(clean_text(content));
        }
    }
    None
}

/// `<title>` 文本（清洗后）。
fn title_text(doc: &Html) -> Option<String> {
    let sel = Selector::parse("title").ok()?;
    let cleaned = clean_text(&doc.select(&sel).next()?.text().collect::<String>());
    (!cleaned.is_empty()).then_some(cleaned)
}

/// 首个匹配选择器的元素（生命周期绑定文档）。
fn select_first<'a>(doc: &'a Html, selector: &str) -> Option<scraper::ElementRef<'a>> {
    let sel = Selector::parse(selector).ok()?;
    doc.select(&sel).next()
}

/// 英文月份缩写表（供 [`extract_date`]）。
const EN_MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// 从文本中尽力提取发布日期字符串（供 `SearchResult.published_at`）。
///
/// 搜索引擎摘要常以日期开头（如 Bing "2025年5月25日 ·"），但格式随引擎/语言变化且
/// 无稳定元素。支持常见模式（尽力而为，不保证覆盖全部）：`YYYY年M月D日`、
/// `YYYY-MM-DD`、`MMM D, YYYY`、`D MMM YYYY`。提取不到返回 `None`。
pub fn extract_date(s: &str) -> Option<String> {
    // 中文/ISO：定位四位年份，向后解析 `年M月D日` 或 `-MM-DD`
    let bytes = s.as_bytes();
    for i in 0..=bytes.len().saturating_sub(4) {
        let Some(y) = s.get(i..i + 4) else {
            continue; // 非 char 边界（多字节字符中间）跳过
        };
        if !y.bytes().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let rest = &s[i + 4..];
        if let Some(rm) = rest.strip_prefix('年') {
            let Some((m, rd)) = take_digits(rm) else {
                continue;
            };
            if let Some(rd) = rd.strip_prefix('月')
                && let Some((d, _)) = take_digits(rd)
            {
                return Some(format!("{y}年{m}月{d}日"));
            }
        }
        if let Some(rm) = rest.strip_prefix('-')
            && let Some((m, rd)) = take_digits(rm)
            && let Some(rd) = rd.strip_prefix('-')
            && let Some((d, _)) = take_digits(rd)
        {
            return Some(format!("{y}-{m}-{d}"));
        }
    }
    // 英文：`MMM D, YYYY` 或 `D MMM YYYY`
    for mon in EN_MONTHS {
        let Some(pos) = s.find(mon) else { continue };
        let after = &s[pos + mon.len()..];
        if let Some(after_space) = after.strip_prefix(' ') {
            // `May 25, 2025`
            if let Some((d, rest)) = take_digits(after_space)
                && let Some(rest) = rest.strip_prefix(", ")
                && let Some(y) = rest
                    .get(..4)
                    .filter(|y| y.bytes().all(|c| c.is_ascii_digit()))
            {
                return Some(format!("{mon} {d}, {y}"));
            }
        }
        // `25 May 2025`
        let before = s[..pos].trim_end();
        if let Some((_, d)) = before.rsplit_once(' ')
            && let Some((d, _)) = take_digits(d)
            && let Some(after_space) = after.strip_prefix(' ')
            && let Some(y) = after_space
                .get(..4)
                .filter(|y| y.bytes().all(|c| c.is_ascii_digit()))
        {
            return Some(format!("{d} {mon} {y}"));
        }
    }
    None
}

/// 取开头的 1-2 位数字（日期/月份），返回 `(数字串, 剩余部分)`。
fn take_digits(s: &str) -> Option<(&str, &str)> {
    let n = s.bytes().take_while(|c| c.is_ascii_digit()).count();
    if n == 0 || n > 2 {
        return None;
    }
    Some((&s[..n], &s[n..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_uddg_redirect() {
        let raw = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Frust";
        let (url, resolved) = normalize_url(raw);
        assert_eq!(url, "https://example.com/rust");
        assert!(resolved, "uddg 展开应标记已解跳转");
    }

    #[test]
    fn strips_fragment_not_resolved() {
        let (url, resolved) = normalize_url("https://example.com/a#sec");
        assert_eq!(url, "https://example.com/a");
        assert!(!resolved, "直接 URL 去 fragment 不算解跳转");
    }

    #[test]
    fn expands_bing_ck_redirect() {
        // Bing 点击追踪链：`u` = `a1` 前缀 + base64 编码目标 URL
        let ck = "https://www.bing.com/ck/a?!&&p=b1685dc2cfed6c5dJmltdHM9MTY4NTU3NzYwMCZpZ3VpZD0wNjJhZmU2NC0yNTg3LTY3NjgtMTJmMi1lZDQ3MjRhZTY2MzImaW5zaWQ9NTE1Nw&ptn=3&hsh=3&fclid=062afe64-2587-6768-12f2-ed4724ae6632&u=a1aHR0cHM6Ly9sb3BlemNhc3Ryb21pbC5jb20v&ntb=1";
        let (url, resolved) = normalize_url(ck);
        assert_eq!(url, "https://lopezcastromil.com/");
        assert!(resolved, "ck/a 解码应标记已解跳转");
    }

    #[test]
    fn bing_ck_without_u_param_kept_as_is() {
        let ck = "https://www.bing.com/ck/a?!&p=abc";
        let (url, resolved) = normalize_url(ck);
        assert_eq!(url, ck);
        assert!(!resolved, "无 u 参数未解码，保持原样");
    }

    #[test]
    fn bing_ck_invalid_base64_kept_as_is() {
        let ck = "https://www.bing.com/ck/a?!&u=a1%21%21%21not-base64";
        let (url, resolved) = normalize_url(ck);
        assert_eq!(url, ck);
        assert!(!resolved, "非法 base64 未解码，保持原样");
    }

    #[test]
    fn bing_ck_decodes_to_non_http_kept_as_is() {
        // 解码结果是合法但非 http(s)（如 javascript:）→ 不信任，保持原样
        let ck = "https://www.bing.com/ck/a?!&u=a1amF2YXNjcmlwdDphbGVydCgxKQ";
        let (url, resolved) = normalize_url(ck);
        assert_eq!(url, ck);
        assert!(!resolved, "非 http(s) 目标不信任，保持原样");
    }

    #[test]
    fn extract_date_supports_common_formats() {
        // 中文（Bing 中文摘要常见）：`2025年5月25日 · 本章...`
        assert_eq!(
            extract_date("2025年5月25日 · 本章基于第 16 章..."),
            Some("2025年5月25日".to_string())
        );
        // ISO：`2025-05-25`
        assert_eq!(
            extract_date("发布于 2025-05-25"),
            Some("2025-05-25".to_string())
        );
        // 英文：`May 25, 2025` 与 `25 May 2025`
        assert_eq!(
            extract_date("May 25, 2025 — some snippet"),
            Some("May 25, 2025".to_string())
        );
        assert_eq!(
            extract_date("Updated 25 May 2025"),
            Some("25 May 2025".to_string())
        );
    }

    #[test]
    fn extract_date_returns_none_without_date() {
        assert_eq!(extract_date("这是摘要一"), None);
        assert_eq!(extract_date("plain snippet without date"), None);
        // 年份但无完整日期（如 "2025 综述"）→ 不误报
        assert_eq!(extract_date("2025 年 Rust 生态综述"), None);
    }

    #[test]
    fn cleans_control_chars_and_whitespace() {
        assert_eq!(clean_text("  a\t\n  b  "), "a b");
    }

    #[test]
    fn url_origin_extracts_host_and_scheme() {
        assert_eq!(
            url_origin("https://example.com/rust"),
            ("example.com".to_string(), true)
        );
        assert_eq!(
            url_origin("http://example.com/a"),
            ("example.com".to_string(), false)
        );
        // 非法 URL：空域名 + 非 https
        assert_eq!(url_origin("not a url"), (String::new(), false));
    }

    /// 特征库识别：真实词典/翻译污染样本（roadmap-result-quality.md 案例）→ 非 Web。
    #[test]
    fn result_kind_marks_pollution_urls() {
        // 词典：iciba `word?w=`（真实污染样本）
        assert_eq!(
            result_kind("https://www.iciba.com/word?w=best"),
            ResultKind::Dictionary
        );
        // 词典：剑桥（host 前缀 dictionary.）
        assert_eq!(
            result_kind("https://dictionary.cambridge.org/dictionary/english/best"),
            ResultKind::Dictionary
        );
        // 词典：eudic `dicts/en/`
        assert_eq!(
            result_kind("https://dict.eudic.net/dicts/en/best"),
            ResultKind::Dictionary
        );
        // 词典：iciba 爱词霸（路径段 word，带非空尾段也精确匹配）
        assert_eq!(
            result_kind("https://www.iciba.com/word?w=learn"),
            ResultKind::Dictionary
        );
        // 翻译：fanyi.baidu.com / fanyi.so（host 前缀 fanyi.）
        assert_eq!(
            result_kind("https://fanyi.baidu.com/#en/zh/best"),
            ResultKind::Translation
        );
        assert_eq!(
            result_kind("https://fanyi.so/dict/?q=best"),
            ResultKind::Translation
        );
        // 翻译：路径段 translate（英文翻译站）
        assert_eq!(
            result_kind("https://translate.yandex.com/"),
            ResultKind::Translation
        );
    }

    /// 回退语义与防误判：正常内容页/特征子串站 → Web。
    #[test]
    fn result_kind_falls_back_to_web() {
        // 正常内容页
        assert_eq!(result_kind("https://example.com/rust"), ResultKind::Web);
        // 非法 URL → Web
        assert_eq!(result_kind("not a url"), ResultKind::Web);
        // 防误判：路径段含 word 子串但非精确匹配（wordpress）
        assert_eq!(
            result_kind("https://wordpress.org/plugins/best"),
            ResultKind::Web
        );
        assert_eq!(
            result_kind("https://example.com/words/best"),
            ResultKind::Web
        );
        // 防误判：普通域名含 dictionary 字样但非词典站
        assert_eq!(
            result_kind("https://dictionary-not-dict.example.com/rust"),
            ResultKind::Web
        );
        // 防误判：host 含 fanyi 子串但属正常站（路径也非翻译）
        assert_eq!(
            result_kind("https://notfanyiso.example.com/rust"),
            ResultKind::Web
        );
    }

    /// 文章页 fixture（tests/fixtures/article.html）：main 内混入 nav/header/aside/form/
    /// script/footer 噪音容器 + 正文段落 + JSON-LD/meta 结构化数据。
    const ARTICLE_HTML: &str = include_str!("../tests/fixtures/article.html");

    /// 正文提取：噪音容器文本被剥离，正文段落保留、空白折叠。
    #[test]
    fn extract_main_text_keeps_body_drops_noise() {
        let (text, truncated) = extract_main_text(ARTICLE_HTML, 20_000);
        assert!(!truncated, "小页面不应截断");
        assert!(text.contains("这是第一段正文内容。"), "正文保留");
        assert!(
            text.contains("这是第二段正文内容，包含 多余 空白。"),
            "空白折叠"
        );
        assert!(!text.contains("导航链接"), "nav 噪音剥离");
        assert!(!text.contains("站点头部"), "header 噪音剥离");
        assert!(!text.contains("侧边栏广告"), "aside 噪音剥离");
        assert!(!text.contains("订阅表单"), "form 噪音剥离");
        assert!(!text.contains("不应出现"), "script 内容剥离");
        assert!(!text.contains("页脚版权"), "footer 噪音剥离");
    }

    /// 正文提取截断：超过 max_chars 截断并标记 truncated。
    #[test]
    fn extract_main_text_truncates_at_max_chars() {
        let (text, truncated) = extract_main_text(ARTICLE_HTML, 6);
        assert!(truncated, "超限应标记截断");
        assert_eq!(text.chars().count(), 6, "截断到 max_chars 字符");
    }

    /// 无正文容器：返回空文本不 panic。
    #[test]
    fn extract_main_text_empty_page() {
        let (text, truncated) = extract_main_text("<html><head></head><body></body></html>", 100);
        assert_eq!(text, "");
        assert!(!truncated);
    }

    /// 字段提取：meta 优先（title/author/published_at/price/currency），
    /// rating 系走 JSON-LD 且保留原生数字类型。
    #[test]
    fn extract_fields_prefers_meta_and_keeps_json_ld_types() {
        use crate::domain::ExtractField as F;
        let fields = extract_fields(ARTICLE_HTML, &F::ALL);
        assert_eq!(fields["title"], "示例商品页面", "og:title 优先于 <title>");
        assert_eq!(fields["author"], "张三", "meta author");
        assert_eq!(
            fields["published_at"], "2026-07-20T10:00:00Z",
            "article:published_time"
        );
        assert_eq!(fields["price"], "1299.00", "meta price 为字符串");
        assert_eq!(fields["currency"], "CNY");
        assert_eq!(fields["rating"], 4.6, "JSON-LD rating 保数字类型");
        assert_eq!(fields["rating_max"], 5);
        assert_eq!(fields["reviews_count"], 1203);
    }

    /// 字段提取：无 meta 时回退 JSON-LD（author 对象取 name），缺失字段不出现。
    #[test]
    fn extract_fields_falls_back_to_json_ld_and_omits_missing() {
        use crate::domain::ExtractField as F;
        let html = r#"<html><head>
            <script type="application/ld+json">{"@type":"Article","author":{"@type":"Person","name":"李四"},"datePublished":"2026-07-21","offers":{"price":99.5,"priceCurrency":"USD"}}</script>
        </head><body><title>无 meta 页面</title></body></html>"#;
        let fields = extract_fields(html, &[F::Author, F::PublishedAt, F::Price, F::Rating]);
        assert_eq!(fields["author"], "李四", "JSON-LD 对象取 name");
        assert_eq!(fields["published_at"], "2026-07-21");
        assert_eq!(fields["price"], 99.5, "JSON-LD price 数字保原生类型");
        assert!(
            !fields.contains_key("rating"),
            "页面无 rating 字段 → 不出现、不编造"
        );
    }

    /// 字段提取：全字段缺失 → 空对象。
    #[test]
    fn extract_fields_empty_when_nothing_found() {
        use crate::domain::ExtractField as F;
        let fields = extract_fields("<html><body>plain</body></html>", &[F::Price, F::Rating]);
        assert!(fields.is_empty());
    }
}
