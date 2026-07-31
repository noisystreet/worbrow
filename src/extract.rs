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
}
