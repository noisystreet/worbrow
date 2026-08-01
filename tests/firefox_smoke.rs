//! 真机冒烟测试：需要本机 Firefox（CI 不运行；`cargo test -- --ignored`）。
//!
//! 说明：测试仅访问 `data:`/`about:` 页面，不依赖外网，可在无外网沙盒验证后端链路。

use std::process::Command;
use std::time::Duration;

use url::Url;
use worbrow::BrowserKind;
use worbrow::drivers;
use worbrow::error::Error;

/// 进程计数断言依赖全局 pgrep，并发测试的浏览器进程会互相干扰（count 含其他实例）：
/// 真机冒烟必须串行执行。`cargo test` 无法按文件设置 `--test-threads`，用共享锁替代——
/// 所有 spawn 浏览器的测试开头获取该锁。
static PROC_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// 统计本工具启动的 Firefox（按临时 profile 路径特征隔离，避免与其他测试/系统进程互相干扰）。
fn firefox_count() -> usize {
    let out = Command::new("pgrep")
        .arg("-c")
        .arg("-f")
        .arg("worbrow-firefox-profile")
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
    let _lock = PROC_LOCK.lock().await;
    let before = firefox_count();
    {
        let _driver = drivers::resolve(BrowserKind::Firefox)
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

/// 显式取消：abort 持有 driver 的任务 → driver drop → Firefox 进程应被清理。
#[tokio::test]
#[ignore = "需要本机 Firefox"]
async fn abort_cancels_and_kills_browser() {
    let _lock = PROC_LOCK.lock().await;
    let before = firefox_count();
    let handle = tokio::spawn(async move {
        let mut driver = drivers::resolve(BrowserKind::Firefox)
            .await
            .expect("spawn 应成功（需要本机 Firefox）");
        // 长时间"搜索"：仅持有 driver 不返回；被 abort 后 driver 随任务 drop
        tokio::time::sleep(Duration::from_secs(30)).await;
        let _ = &mut driver;
    });
    // 轮询等待 Firefox 进程出现（resolve 可能慢于固定 sleep）
    let mut spawned = false;
    for _ in 0..75 {
        if firefox_count() > before {
            spawned = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(spawned, "spawn 后应有 Firefox 进程（before={before}）");
    handle.abort();
    for _ in 0..20 {
        if firefox_count() <= before {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(firefox_count(), before, "任务取消后 Firefox 应被清理");
}

/// app 超时：全流程 timeout 触发 → driver drop → Firefox 进程应被回收。
#[tokio::test]
#[ignore = "需要本机 Firefox"]
async fn search_timeout_recycles_browser() {
    let _lock = PROC_LOCK.lock().await;
    let before = firefox_count();
    let cfg = worbrow::Config::new("rust", "bing", BrowserKind::Firefox)
        .with_timeout(Duration::from_millis(100))
        .with_max_results(5);
    let err = worbrow::run(cfg).await.unwrap_err();
    assert!(matches!(err, Error::Timeout(_)));
    for _ in 0..20 {
        if firefox_count() <= before {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(firefox_count(), before, "搜索超时后 Firefox 应被回收");
}

/// 后端完整链路（无外网依赖）：spawn → navigate(data URL) → wait_for → html → eval → screenshot。
#[tokio::test]
#[ignore = "需要本机 Firefox"]
async fn end_to_end_on_data_url() {
    let _lock = PROC_LOCK.lock().await;
    let mut driver = drivers::resolve(BrowserKind::Firefox)
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
    let shot_path = std::env::temp_dir().join("worbrow-firefox-shot.png");
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
    let _lock = PROC_LOCK.lock().await;
    let mut driver = drivers::resolve(BrowserKind::Firefox)
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

/// 并发 spawn：两个实例随机端口不冲突（design.md §10.1）。
#[tokio::test]
#[ignore = "需要本机 Firefox"]
async fn concurrent_spawns_use_distinct_ports() {
    let _lock = PROC_LOCK.lock().await;
    let (a, b) = tokio::join!(
        drivers::resolve(BrowserKind::Firefox),
        drivers::resolve(BrowserKind::Firefox)
    );
    assert!(a.is_ok(), "实例 A 应成功（端口冲突或启动失败）");
    assert!(b.is_ok(), "实例 B 应成功（端口冲突或启动失败）");
}

/// 无效 URL 导航：不挂死，40s 内必须返回（pageLoad 超时已收紧，design.md §8）。
#[tokio::test]
#[ignore = "需要本机 Firefox"]
async fn navigate_invalid_url_errors_instead_of_hanging() {
    let _lock = PROC_LOCK.lock().await;
    let mut driver = drivers::resolve(BrowserKind::Firefox)
        .await
        .expect("spawn 应成功（需要本机 Firefox）");
    let result = tokio::time::timeout(
        Duration::from_secs(40),
        driver.navigate(Url::parse("http://127.0.0.1:1/").expect("URL 应合法")),
    )
    .await;
    assert!(result.is_ok(), "navigate 不应挂死（40s 无响应即失败）");
}

/// 会话池复用：连续两次 acquire 复用同一 Firefox 进程（进程数不增长），
/// 且归还后 TTL 内再借出仍是同一进程（roadmap-session-pool.md §5 真机冒烟）。
#[tokio::test]
#[ignore = "需要本机 Firefox"]
async fn pool_reuses_same_firefox_process() {
    use worbrow::drivers::SessionPool;

    let _lock = PROC_LOCK.lock().await;
    let before = firefox_count();
    let pool = SessionPool::new(BrowserKind::Firefox, 1, Duration::from_secs(60), 4);

    // 第一次 acquire：启动一个 Firefox 进程
    let g1 = pool
        .acquire()
        .await
        .expect("首次 acquire 应成功（需要本机 Firefox）");
    let mut spawned = false;
    for _ in 0..75 {
        if firefox_count() > before {
            spawned = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        spawned,
        "首次 acquire 后应有 Firefox 进程（before={before}）"
    );
    drop(g1);

    // 第二次 acquire（TTL 内）：应复用同一进程，进程数不增长
    let g2 = pool.acquire().await.expect("复用 acquire 应成功");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        firefox_count(),
        before + 1,
        "池化后进程数应保持 1（复用而非新建）"
    );
    drop(g2);

    // 显式 drop 池：空闲会话随池 Drop → driver Drop → kill Firefox（design.md §8）
    drop(pool);
    for _ in 0..20 {
        if firefox_count() <= before {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(firefox_count(), before, "池 Drop 后 Firefox 应被清理");
}
