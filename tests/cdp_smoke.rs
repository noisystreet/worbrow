//! 真机冒烟测试：需要本机 Chrome/Chromium ≥ 109（CI 不运行；`cargo test -- --ignored`）。
//!
//! 说明：测试仅访问 `data:`/`about:` 页面与本地拒绝端口，不依赖外网，可在无外网沙盒验证 CDP 后端链路。

use std::process::Command;
use std::time::Duration;

use url::Url;
use worbrow::drivers::cdp::CdpDriver;
use worbrow::error::Error;

/// 统计本工具启动的 Chrome（按临时 user-data-dir 路径特征隔离）。
fn chrome_count() -> usize {
    let out = Command::new("pgrep")
        .arg("-c")
        .arg("-f")
        .arg("worbrow-chrome-profile")
        .output()
        .expect("pgrep 应可执行");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

/// 生命周期：spawn 后 driver 离开作用域，Chrome 子进程应被清理（design.md §8）。
#[tokio::test]
#[ignore = "需要本机 Chrome/Chromium ≥ 109"]
async fn spawn_then_drop_kills_chrome() {
    let before = chrome_count();
    {
        let _driver = CdpDriver::spawn()
            .await
            .expect("spawn 应成功（需要本机 Chrome/Chromium ≥ 109）");
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(
            chrome_count() > before,
            "spawn 后应有 Chrome 进程（before={before}）"
        );
    }
    for _ in 0..20 {
        if chrome_count() <= before {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(chrome_count(), before, "driver drop 后 Chrome 应被清理");
}

/// 后端完整链路（无外网依赖）：spawn → navigate(data URL) → wait_for → html → eval → screenshot。
#[tokio::test]
#[ignore = "需要本机 Chrome/Chromium ≥ 109"]
async fn end_to_end_on_data_url() {
    let mut driver = CdpDriver::spawn()
        .await
        .expect("spawn 应成功（需要本机 Chrome/Chromium ≥ 109）");

    let url =
        Url::parse("data:text/html,<h1 id%3D%22t%22>hello</h1><p class%3D%22r%22>snippet</p>")
            .expect("data URL 应合法");
    driver.navigate(url).await.expect("navigate 应成功");

    driver
        .wait_for("h1#t", Duration::from_secs(5))
        .await
        .expect("wait_for 应成功");

    let html = driver.html().await.expect("html 应成功");
    assert!(html.contains("hello"), "html 应包含 h1 内容: {html}");

    let title = driver
        .eval("document.querySelector('h1').textContent;")
        .await
        .expect("eval 应成功");
    assert_eq!(title, "hello");

    let shot_path = std::env::temp_dir().join("worbrow-chrome-shot.png");
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
#[ignore = "需要本机 Chrome/Chromium ≥ 109"]
async fn wait_for_missing_selector_times_out() {
    let mut driver = CdpDriver::spawn()
        .await
        .expect("spawn 应成功（需要本机 Chrome/Chromium ≥ 109）");

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

/// 并发 spawn：两个实例随机端口不冲突（design.md §10.1）。
#[tokio::test]
#[ignore = "需要本机 Chrome/Chromium ≥ 109"]
async fn concurrent_spawns_use_distinct_ports() {
    let (a, b) = tokio::join!(CdpDriver::spawn(), CdpDriver::spawn());
    assert!(a.is_ok(), "实例 A 应成功（端口冲突或启动失败）");
    assert!(b.is_ok(), "实例 B 应成功（端口冲突或启动失败）");
}

/// 无效 URL 导航：不挂死，40s 内必须返回（readyState 轮询超时，design.md §8）。
#[tokio::test]
#[ignore = "需要本机 Chrome/Chromium ≥ 109"]
async fn navigate_invalid_url_errors_instead_of_hanging() {
    let mut driver = CdpDriver::spawn()
        .await
        .expect("spawn 应成功（需要本机 Chrome/Chromium ≥ 109）");
    let result = tokio::time::timeout(
        Duration::from_secs(40),
        driver.navigate(Url::parse("http://127.0.0.1:1/").expect("URL 应合法")),
    )
    .await;
    assert!(result.is_ok(), "navigate 不应挂死（40s 无响应即失败）");
}
