//! 真机冒烟测试：需要本机 Firefox（CI 不运行；`cargo test -- --ignored`）。
//!
//! 说明：测试仅访问 `data:`/`about:` 页面，不依赖外网，可在无外网沙盒验证后端链路。

use std::process::Command;
use std::time::Duration;

use rplay_search::drivers::marionette::MarionetteDriver;
use rplay_search::error::Error;
use url::Url;

/// 统计本工具启动的 Firefox（按临时 profile 路径特征隔离，避免与其他测试/系统进程互相干扰）。
fn firefox_count() -> usize {
    let out = Command::new("pgrep")
        .arg("-c")
        .arg("-f")
        .arg("search-firefox-profile")
        .output()
        .expect("pgrep 应可执行");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

/// 生命周期：spawn 后 driver 离开作用域，Firefox 子进程应被清理（design.md §8）。
#[tokio::test]
#[ignore = "需要本机 Firefox"]
async fn spawn_then_drop_kills_firefox() {
    let before = firefox_count();
    {
        let _driver = MarionetteDriver::spawn()
            .await
            .expect("spawn 应成功（需要本机 Firefox）");
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(
            firefox_count() > before,
            "spawn 后应有 Firefox 进程（before={before}）"
        );
    }
    for _ in 0..20 {
        if firefox_count() <= before {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(firefox_count(), before, "driver drop 后 Firefox 应被清理");
}

/// 后端完整链路（无外网依赖）：spawn → navigate(data URL) → wait_for → html → eval → screenshot。
#[tokio::test]
#[ignore = "需要本机 Firefox"]
async fn end_to_end_on_data_url() {
    let mut driver = MarionetteDriver::spawn()
        .await
        .expect("spawn 应成功（需要本机 Firefox）");

    let url =
        Url::parse("data:text/html,<h1 id%3D%22t%22>hello</h1><p class%3D%22r%22>snippet</p>")
            .expect("data URL 应合法");
    driver.navigate(url).await.expect("navigate 应成功");

    // wait_for：结果选择器轮询
    driver
        .wait_for("h1#t", Duration::from_secs(5))
        .await
        .expect("wait_for 应成功");

    // html：GetPageSource 应包含内容
    let html = driver.html().await.expect("html 应成功");
    assert!(html.contains("hello"), "html 应包含 h1 内容: {html}");

    // eval：ExecuteScript 读页面状态
    let title = driver
        .eval("return document.querySelector('h1').textContent;")
        .await
        .expect("eval 应成功");
    assert_eq!(title, "hello");

    // screenshot：写临时文件并校验 PNG 头
    let shot_path = std::env::temp_dir().join("rplay-firefox-shot.png");
    driver
        .screenshot(&shot_path)
        .await
        .expect("screenshot 应成功");
    let png = std::fs::read(&shot_path).expect("截图文件应存在");
    assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    let _ = std::fs::remove_file(&shot_path);
}

/// wait_for 超时：不存在的选择器 → Error::Timeout。
#[tokio::test]
#[ignore = "需要本机 Firefox"]
async fn wait_for_missing_selector_times_out() {
    let mut driver = MarionetteDriver::spawn()
        .await
        .expect("spawn 应成功（需要本机 Firefox）");

    driver
        .navigate(Url::parse("data:text/html,<p>hi</p>").expect("data URL 应合法"))
        .await
        .expect("navigate 应成功");

    let err = driver
        .wait_for("nosuch-element", Duration::from_millis(800))
        .await
        .expect_err("不存在的选择器应超时");
    assert!(matches!(err, Error::Timeout(_)));
}
