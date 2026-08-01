//! 链接归一化与文本清洗（design.md §6.5 公共工具，供各引擎适配器复用）。

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

/// 归一化结果链接：补齐协议相对链接、展开跳转参数（uddg 等）、去 fragment。
pub fn normalize_url(raw: &str) -> String {
    // DuckDuckGo html 版使用协议相对链接
    let raw = if let Some(rest) = raw.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        raw.to_string()
    };
    let Ok(mut url) = Url::parse(&raw) else {
        return raw;
    };
    // 展开 DDG 跳转参数 uddg（真实目标）
    if let Some((_, target)) = url
        .query_pairs()
        .find(|(k, _)| k == "uddg")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
    {
        return normalize_url(&target);
    }
    url.set_fragment(None);
    url.to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_uddg_redirect() {
        let raw = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Frust";
        assert_eq!(normalize_url(raw), "https://example.com/rust");
    }

    #[test]
    fn strips_fragment() {
        assert_eq!(
            normalize_url("https://example.com/a#sec"),
            "https://example.com/a"
        );
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
