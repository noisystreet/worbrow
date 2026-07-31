//! 浏览器二进制发现（design.md §10.2）。
//!
//! 发现顺序：`CHROME_PATH` / `FIREFOX_PATH` 环境变量 → PATH 搜索 → 平台默认位置。

use std::path::PathBuf;

use super::BrowserKind;
use crate::error::Error;

/// 各平台已知的浏览器可执行文件名（PATH 搜索用）。
const CHROME_NAMES: &[&str] = &[
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "microsoft-edge",
    "msedge",
];
const FIREFOX_NAMES: &[&str] = &["firefox", "firefox-esr", "firefox-developer-edition"];

/// 平台默认安装位置（PATH 之外，最后兜底）。
#[cfg(target_os = "macos")]
const CHROME_DEFAULT: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
#[cfg(target_os = "macos")]
const FIREFOX_DEFAULT: &str = "/Applications/Firefox.app/Contents/MacOS/firefox";

#[cfg(target_os = "windows")]
const CHROME_DEFAULT: &str = r"C:\Program Files\Google\Chrome\Application\chrome.exe";
#[cfg(target_os = "windows")]
const FIREFOX_DEFAULT: &str = r"C:\Program Files\Mozilla Firefox\firefox.exe";

#[cfg(target_os = "linux")]
const CHROME_DEFAULT: &str = "/usr/bin/google-chrome";
#[cfg(target_os = "linux")]
const FIREFOX_DEFAULT: &str = "/usr/bin/firefox";

/// 查找浏览器可执行文件路径。
pub fn find_browser(kind: BrowserKind) -> Result<PathBuf, Error> {
    let (env_var, names, default) = match kind {
        BrowserKind::Chrome => ("CHROME_PATH", CHROME_NAMES, CHROME_DEFAULT),
        BrowserKind::Firefox => ("FIREFOX_PATH", FIREFOX_NAMES, FIREFOX_DEFAULT),
        BrowserKind::Fake => return Err(Error::Internal("fake 无真实二进制".into())),
    };

    // 1. 环境变量显式指定
    if let Ok(path) = std::env::var(env_var) {
        if !path.trim().is_empty() {
            let p = PathBuf::from(path);
            if p.exists() {
                return Ok(p);
            }
            return Err(Error::Env(format!("{env_var}={} 不存在", p.display())));
        }
    }

    // 2. PATH 搜索
    if let Some(found) = search_path(names) {
        return Ok(found);
    }

    // 3. 平台默认位置
    let default = PathBuf::from(default);
    if default.exists() {
        return Ok(default);
    }

    Err(Error::Env(format!(
        "未找到 {} 浏览器二进制（设置 {env_var} 或安装后重试）",
        kind
    )))
}

/// 在 PATH 各目录中查找任一可执行文件。
fn search_path(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for name in names {
            let candidate = dir.join(name);
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// 读取浏览器主版本号（`--version` 输出解析，如 "Mozilla Firefox 140.10.0esr" → 140、
/// "Google Chrome 123.0.6312.106" → 123）。读取失败返回 `None`。
pub fn browser_major_version(binary: &std::path::Path) -> Option<u32> {
    let out = std::process::Command::new(binary)
        .arg("--version")
        .output()
        .ok()?;
    parse_major_version(&String::from_utf8_lossy(&out.stdout))
}

/// 从 `--version` 输出文本解析主版本号（纯函数，便于测试）。
fn parse_major_version(text: &str) -> Option<u32> {
    let token = text
        .split_whitespace()
        .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))?;
    token
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|major| major.parse().ok())
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_file()
        && std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_names_cover_common_binaries() {
        // 至少包含主流通用名，避免将来误删
        assert!(CHROME_NAMES.contains(&"google-chrome"));
        assert!(CHROME_NAMES.contains(&"chromium"));
        assert!(FIREFOX_NAMES.contains(&"firefox"));
    }

    #[test]
    fn fake_kind_has_no_binary() {
        let err = find_browser(BrowserKind::Fake).unwrap_err();
        assert!(matches!(err, Error::Internal(_)));
    }

    #[test]
    fn parses_firefox_and_chrome_version_strings() {
        assert_eq!(
            parse_major_version("Mozilla Firefox 140.10.0esr"),
            Some(140)
        );
        assert_eq!(
            parse_major_version("Google Chrome 123.0.6312.106"),
            Some(123)
        );
        assert_eq!(parse_major_version("Mozilla Firefox 58.0.1"), Some(58));
        assert_eq!(parse_major_version("unexpected output"), None);
        assert_eq!(parse_major_version(""), None);
    }
}
