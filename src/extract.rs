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
    // 展开 Bing 点击追踪链 ck/a（解码失败保持原样）
    if let Some(expanded) = expand_bing_ck(&url) {
        return expanded;
    }
    url.set_fragment(None);
    url.to_string()
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
    fn expands_bing_ck_redirect() {
        // Bing 点击追踪链：`u` = `a1` 前缀 + base64 编码目标 URL
        let ck = "https://www.bing.com/ck/a?!&&p=b1685dc2cfed6c5dJmltdHM9MTY4NTU3NzYwMCZpZ3VpZD0wNjJhZmU2NC0yNTg3LTY3NjgtMTJmMi1lZDQ3MjRhZTY2MzImaW5zaWQ9NTE1Nw&ptn=3&hsh=3&fclid=062afe64-2587-6768-12f2-ed4724ae6632&u=a1aHR0cHM6Ly9sb3BlemNhc3Ryb21pbC5jb20v&ntb=1";
        assert_eq!(normalize_url(ck), "https://lopezcastromil.com/");
    }

    #[test]
    fn bing_ck_without_u_param_kept_as_is() {
        let ck = "https://www.bing.com/ck/a?!&p=abc";
        assert_eq!(normalize_url(ck), ck);
    }

    #[test]
    fn bing_ck_invalid_base64_kept_as_is() {
        let ck = "https://www.bing.com/ck/a?!&u=a1%21%21%21not-base64";
        assert_eq!(normalize_url(ck), ck);
    }

    #[test]
    fn bing_ck_decodes_to_non_http_kept_as_is() {
        // 解码结果是合法但非 http(s)（如 javascript:）→ 不信任，保持原样
        let ck = "https://www.bing.com/ck/a?!&u=a1amF2YXNjcmlwdDphbGVydCgxKQ";
        assert_eq!(normalize_url(ck), ck);
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
