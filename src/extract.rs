//! 链接归一化与文本清洗（design.md §6.5 公共工具，供各引擎适配器复用）。

use base64::Engine as _;
use url::Url;

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
}
