//! 浏览器会话池：MCP 长驻进程内复用浏览器进程（docs/roadmap-session-pool.md）。
//!
//! 目标：
//! - 消除每次搜索 spawn 新浏览器的 2-5s 开销（MCP 高频调用场景收益最大）
//! - 空闲 TTL 回收防残留（design.md §8 语义延续，进程退出 Drop 兜底）
//! - 并发上限与排队（`Semaphore`，池有界）
//! - 崩溃会话复活（错误驱动健康判定：命令失败 → 标记不健康 → 丢弃重建）
//!
//! 设计：
//! - [`SessionPool`]：空闲会话 LIFO（最近归还优先复用）+ `Semaphore` 限并发 +
//!   TTL reaper 后台任务（回收超时空闲会话）
//! - [`SessionGuard`]：借出/归还 RAII——Drop 时健康则回池，否则 Drop 触发
//!   driver 回收（kill 子进程）；`mark_unhealthy` 由调用方在使用出错时调用
//!
//! 依赖方向：本模块属于 drivers（adapters 层），面向 `ports::BrowserDriver` 编程，
//! 经注入的 spawn 工厂创建新会话（生产 = `drivers::resolve`，测试可注入计数假驱动）；
//! 不反向依赖 app/mcp。

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::domain::BrowserKind;
use crate::error::Error;
use crate::ports::BrowserDriver;

/// 会话 spawn 工厂：`BrowserKind` → 已连接的 `BrowserDriver`（生产为 `drivers::resolve`）。
type SpawnFn = dyn Fn() -> Pin<Box<dyn Future<Output = Result<Box<dyn BrowserDriver>, Error>> + Send>>
    + Send
    + Sync;

/// 会话池：空闲会话复用 + 并发上限 + 空闲 TTL 回收。
///
/// 同一池只服务单一 `BrowserKind`（不混池，避免并发 profile 冲突）；MCP 按浏览器类型
/// 惰性建池（见 mcp.rs）。创建须在 tokio runtime 上下文（reaper 后台任务需要）；
/// 无 runtime 时跳过 reaper，仅靠借出时的惰性修剪与进程退出 Drop 兜底。
pub struct SessionPool {
    kind: BrowserKind,
    /// 会话创建工厂（生产 = `drivers::resolve`；测试可注入计数假驱动）。
    spawn: Box<SpawnFn>,
    /// 空闲会话（LIFO：队尾为最近归还，优先复用）。
    idle: Mutex<Vec<IdleSession>>,
    /// 并发上限（含借出中）：超限的 acquire 排队等待。
    semaphore: Arc<Semaphore>,
    /// 空闲回收阈值：会话闲置超过该时长即被 reaper 回收。
    idle_ttl: Duration,
    /// 空闲保留上限：超过时丢弃最旧（防并发峰值后浏览器进程堆积）。
    max_idle: usize,
}

/// 空闲会话条目（含归还时刻，供 TTL 判定）。
struct IdleSession {
    driver: Box<dyn BrowserDriver>,
    returned_at: Instant,
}

impl std::fmt::Debug for SessionPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // spawn 工厂为闭包（不可 Debug），只输出可观测的池状态；
        // try_lock 避免 Debug（如 tracing 日志）在 async 上下文阻塞 worker
        let idle_len = self.idle.try_lock().map(|v| v.len()).unwrap_or(0);
        f.debug_struct("SessionPool")
            .field("kind", &self.kind)
            .field("idle_len", &idle_len)
            .field("semaphore", &self.semaphore)
            .field("idle_ttl", &self.idle_ttl)
            .field("max_idle", &self.max_idle)
            .finish()
    }
}

impl SessionPool {
    /// 生产创建：会话经 `drivers::resolve(kind)` 建立。
    pub fn new(
        kind: BrowserKind,
        max_sessions: usize,
        idle_ttl: Duration,
        max_idle: usize,
    ) -> Arc<Self> {
        let spawn_kind = kind;
        Self::with_spawn(
            kind,
            max_sessions,
            idle_ttl,
            max_idle,
            Box::new(move || Box::pin(crate::drivers::resolve(spawn_kind))),
        )
    }

    /// 注入 spawn 工厂（测试用：计数/失败注入）。
    pub(crate) fn with_spawn(
        kind: BrowserKind,
        max_sessions: usize,
        idle_ttl: Duration,
        max_idle: usize,
        spawn: Box<SpawnFn>,
    ) -> Arc<Self> {
        let pool = Arc::new(Self {
            kind,
            spawn,
            idle: Mutex::new(Vec::new()),
            semaphore: Arc::new(Semaphore::new(max_sessions.max(1))),
            idle_ttl,
            max_idle: max_idle.max(1),
        });
        // 后台 reaper：仅在 tokio runtime 上下文 spawn（CLI 等无 runtime 场景跳过，
        // TTL 回收由进程退出 Drop 兜底；MCP/测试均运行在 runtime 内）
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(Self::reaper_loop(Arc::downgrade(&pool)));
        }
        pool
    }

    /// 借出一个会话：优先复用最近归还的空闲会话；无则经 spawn 工厂新建。
    /// 并发超限时排队等待（Semaphore permit）。
    pub async fn acquire(self: &Arc<Self>) -> Result<SessionGuard, Error> {
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::Internal("session pool is closed".into()))?;
        // 先从空闲池取（锁在无 await 的分支内即释放，避免 MutexGuard 跨 await 非 Send）
        let idle_driver = {
            let mut idle = self
                .idle
                .lock()
                .map_err(|_| Error::Internal("session pool lock poisoned".into()))?;
            idle.pop().map(|entry| entry.driver)
        };
        let driver = match idle_driver {
            Some(d) => d,
            None => (self.spawn)().await?,
        };
        Ok(SessionGuard {
            pool: Arc::clone(self),
            driver: Some(driver),
            healthy: true,
            _permit: permit,
        })
    }

    /// 回收超过 TTL 的空闲会话（reaper 周期调用 + 归还时惰性修剪）。
    fn reap_expired(&self) {
        let Ok(mut idle) = self.idle.lock() else {
            return;
        };
        let now = Instant::now();
        // 1. 回收闲置超 TTL 的会话（Drop → 触发 driver 回收/kill 子进程）
        idle.retain(|s| now.saturating_duration_since(s.returned_at) < self.idle_ttl);
        // 2. 超过 max_idle 的丢弃最旧（队列头，队尾为最近归还）
        let overflow = idle.len().saturating_sub(self.max_idle);
        if overflow > 0 {
            idle.drain(0..overflow);
        }
    }

    /// reaper 后台循环：周期回收过期空闲会话；池被 Drop（Weak 升级失败）即退出。
    ///
    /// **不跨 await 持有 `Arc<Self>`**：`pool.upgrade()` 的临时强引用必须在 await 前
    /// 释放，否则 reaper 自身成为池的强引用，SessionPool 永不 Drop（泄漏浏览器进程）。
    async fn reaper_loop(pool: Weak<Self>) {
        loop {
            // 短暂 upgrade 读取 interval 后立即释放（不跨 await 持有）
            let interval = match pool.upgrade() {
                Some(this) => (this.idle_ttl / 2).max(Duration::from_millis(100)),
                None => return,
            };
            tokio::time::sleep(interval).await;
            // 短暂 upgrade 执行回收后立即释放
            let Some(this) = pool.upgrade() else {
                return;
            };
            this.reap_expired();
        }
    }
}

/// 借出的会话句柄（RAII）。
///
/// - `driver()`：访问底层 `BrowserDriver`（调用方执行搜索）
/// - `mark_unhealthy()`：使用出错时标记，Drop 时丢弃而非回池（触发重建）
/// - Drop：健康 → 归还空闲池；不健康 → driver Drop（kill 子进程）；permit 释放
pub struct SessionGuard {
    pool: Arc<SessionPool>,
    driver: Option<Box<dyn BrowserDriver>>,
    healthy: bool,
    _permit: OwnedSemaphorePermit,
}

impl std::fmt::Debug for SessionGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionGuard")
            .field("pool", &self.pool)
            .field("borrowed", &self.driver.is_some())
            .field("healthy", &self.healthy)
            .finish_non_exhaustive()
    }
}

impl SessionGuard {
    /// 底层驱动（借用；guard 存续期间独占）。
    pub fn driver(&mut self) -> &mut dyn BrowserDriver {
        self.driver
            .as_mut()
            .expect("guard 已归还（不应再访问）")
            .as_mut()
    }

    /// 标记会话不健康：Drop 时丢弃重建（调用方在命令失败后调用）。
    pub fn mark_unhealthy(&mut self) {
        self.healthy = false;
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        if let Some(driver) = self.driver.take()
            && self.healthy
            && let Ok(mut idle) = self.pool.idle.lock()
        {
            idle.push(IdleSession {
                driver,
                returned_at: Instant::now(),
            });
            // 归还时惰性修剪超限（队头为最旧）
            let overflow = idle.len().saturating_sub(self.pool.max_idle);
            if overflow > 0 {
                idle.drain(0..overflow);
            }
        }
        // 不健康或锁中毒：driver 在此 drop（回收浏览器进程）
        // _permit 在此自动释放
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 计数假驱动：记录实例总数，供复用/重建断言。
    #[derive(Default)]
    struct CountingDriver {
        html: String,
    }

    impl CountingDriver {
        fn with_html(html: impl Into<String>) -> Self {
            Self { html: html.into() }
        }
    }

    #[async_trait::async_trait]
    impl BrowserDriver for CountingDriver {
        async fn navigate(&mut self, _url: url::Url) -> Result<(), Error> {
            Ok(())
        }
        async fn wait_for(&mut self, _s: &str, _t: Duration) -> Result<(), Error> {
            Ok(())
        }
        async fn html(&self) -> Result<String, Error> {
            Ok(self.html.clone())
        }
        async fn eval(&mut self, _js: &str) -> Result<serde_json::Value, Error> {
            Ok(serde_json::Value::Null)
        }
        async fn screenshot(&mut self, _path: &std::path::Path) -> Result<(), Error> {
            Ok(())
        }
    }

    /// 注入计数 spawn 工厂，返回 `(pool, spawn 次数 Arc)`。
    fn test_pool(
        max_sessions: usize,
        ttl: Duration,
        max_idle: usize,
    ) -> (Arc<SessionPool>, Arc<AtomicUsize>) {
        let spawns = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&spawns);
        let pool = SessionPool::with_spawn(
            BrowserKind::Fake,
            max_sessions,
            ttl,
            max_idle,
            Box::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                Box::pin(async {
                    Ok(Box::new(CountingDriver::with_html("<html>pool</html>"))
                        as Box<dyn BrowserDriver>)
                })
            }),
        );
        (pool, spawns)
    }

    /// 归还后 acquire 复用同一实例（spawn 次数不增）。
    #[tokio::test]
    async fn reuses_idle_session() {
        let (pool, spawns) = test_pool(1, Duration::from_secs(60), 4);
        {
            let _g = pool.acquire().await.unwrap();
        } // drop 归还
        let _g2 = pool.acquire().await.unwrap();
        assert_eq!(spawns.load(Ordering::SeqCst), 1, "第二次应复用空闲会话");
    }

    /// 并发上限 = 1 时第二个 acquire 排队（permit 限制）。
    #[tokio::test]
    async fn concurrent_acquire_is_limited() {
        let (pool, spawns) = test_pool(1, Duration::from_secs(60), 4);
        let g1 = pool.acquire().await.unwrap();
        // 第二个 acquire 在 g1 释放前不应完成（Semaphore 上限 1）：短窗口内应超时
        let timed_out = tokio::time::timeout(Duration::from_millis(100), pool.acquire())
            .await
            .is_err();
        assert!(timed_out, "上限 1 时第二个应排队等待");
        drop(g1);
        let _g2 = pool.acquire().await.unwrap();
        assert_eq!(spawns.load(Ordering::SeqCst), 1, "排队后应复用归还的会话");
    }

    /// 不健康会话 Drop 时丢弃重建（下次 acquire 重新 spawn）。
    #[tokio::test]
    async fn unhealthy_session_is_discarded() {
        let (pool, spawns) = test_pool(1, Duration::from_secs(60), 4);
        {
            let mut g = pool.acquire().await.unwrap();
            g.mark_unhealthy();
        } // drop：不健康 → 丢弃
        let _g2 = pool.acquire().await.unwrap();
        assert_eq!(spawns.load(Ordering::SeqCst), 2, "不健康会话应重建");
    }

    /// 空闲 TTL：过期会话被 reaper 回收，下次 acquire 重建。
    #[tokio::test]
    async fn idle_ttl_reaps_expired_session() {
        let (pool, spawns) = test_pool(1, Duration::from_millis(150), 4);
        {
            let _g = pool.acquire().await.unwrap();
        } // 归还
        // reaper 间隔 = ttl/2 = 75ms（下限 100ms）；等足够时长触发回收
        tokio::time::sleep(Duration::from_millis(400)).await;
        let _g2 = pool.acquire().await.unwrap();
        assert_eq!(
            spawns.load(Ordering::SeqCst),
            2,
            "TTL 过期会话应被回收并重建"
        );
    }

    /// TTL 内复用（不重建）：验证 reaper 不误杀活跃空闲会话。
    #[tokio::test]
    async fn within_ttl_reuses_session() {
        let (pool, spawns) = test_pool(1, Duration::from_secs(60), 4);
        {
            let _g = pool.acquire().await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
        let _g2 = pool.acquire().await.unwrap();
        assert_eq!(spawns.load(Ordering::SeqCst), 1, "TTL 内应复用");
    }

    /// 空闲上限：归还超过 max_idle 时丢弃最旧。
    #[tokio::test]
    async fn max_idle_trims_oldest() {
        let (pool, spawns) = test_pool(4, Duration::from_secs(60), 2);
        // 借出 4 个会话（超出 max_idle 上限的场景：并发峰值）
        let mut guards = Vec::new();
        for _ in 0..4 {
            guards.push(pool.acquire().await.unwrap());
        }
        assert_eq!(spawns.load(Ordering::SeqCst), 4);
        drop(guards); // 全部归还：应只保留最近 2 个
        let idle = pool.idle.lock().unwrap();
        assert_eq!(idle.len(), 2, "归还后空闲会话应修剪到 max_idle");
        assert_eq!(spawns.load(Ordering::SeqCst), 4, "修剪只丢弃不新建");
    }

    /// spawn 工厂失败：acquire 返回错误且 permit 释放（后续 acquire 可用）。
    #[tokio::test]
    async fn spawn_failure_releases_permit() {
        let pool = SessionPool::with_spawn(
            BrowserKind::Fake,
            1,
            Duration::from_secs(60),
            4,
            Box::new(|| Box::pin(async { Err(Error::Env("测试注入 spawn 失败".into())) })),
        );
        let err = pool.acquire().await.expect_err("spawn 失败应返回错误");
        assert!(matches!(err, Error::Env(_)));
        // 失败后 permit 已释放：替换工厂重试应成功
        let (pool2, _) = test_pool(1, Duration::from_secs(60), 4);
        let _g = pool2.acquire().await.unwrap();
    }
}
