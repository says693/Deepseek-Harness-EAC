//! 渲染进程崩溃/挂起自恢复（renderer-recovery.js 的 Tauri 适配版）。
//!
//! Electron 事件面（render-process-gone / unresponsive / did-fail-load）在
//! WebView2 下不完全可用；本实现以两个可靠信号驱动同一决策树：
//!   1. 心跳丢失（注入脚本每 5s 上报，可见窗口 45s 未报 = 挂起/崩溃）
//!   2. 服务探活失败（expecting_web 期间 HTTP 探测失败 = 加载失败/服务间隙）
//! 决策函数（compute_backoff / next_action）与 JS 版参数一致，单测对齐。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const MAX_ATTEMPTS: u32 = 4;
pub const ATTEMPT_WINDOW_MS: u64 = 90 * 1000;
pub const STABILITY_MS: u64 = 30 * 1000;
pub const FIRST_DELAY_MS: u64 = 800;
pub const BACKOFF_BASE_MS: u64 = 2000;
pub const BACKOFF_MAX_MS: u64 = 15000;
pub const HEARTBEAT_MISS_MS: u64 = 45 * 1000;
pub const CHECK_INTERVAL_MS: u64 = 15 * 1000;

/// 与 renderer-recovery.js computeBackoff 一致（同参数同曲线）。
pub fn compute_backoff(failures: u32) -> u64 {
    if failures <= 1 {
        return FIRST_DELAY_MS;
    }
    let exp = (failures - 1).min(16);
    let cap = BACKOFF_MAX_MS.min(BACKOFF_BASE_MS.saturating_mul(1u64 << exp));
    // +15%~+35% 抖动（用系统时钟熵，避免引 rand）
    let jitter_seed = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos()
        % 100) as u64;
    let jitter = cap * (15 + jitter_seed * 20 / 99) / 100;
    cap + jitter
}

/// 与 renderer-recovery.js nextAction 一致：1~2 reload；3 且未重建过 rebuild；>4 give-up。
pub fn next_action(failures: u32, rebuilt_in_burst: bool) -> RecoveryAction {
    if failures > MAX_ATTEMPTS {
        return RecoveryAction::GiveUp;
    }
    if failures == 3 && !rebuilt_in_burst {
        return RecoveryAction::Rebuild;
    }
    RecoveryAction::Reload
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RecoveryAction {
    Reload,
    Rebuild,
    GiveUp,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RecoveryState {
    pub failures: u32,
    pub gave_up: bool,
    pub expecting_web: bool,
    pub last_failure: Option<LastFailure>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LastFailure {
    pub reason: String,
    pub at: String,
}

struct Inner {
    failures: u32,
    window_start: Option<Instant>,
    gave_up: bool,
    rebuilt_in_burst: bool,
    expecting_web: bool,
    user_hidden: bool,
    last_heartbeat: Option<Instant>,
    last_failure: Option<LastFailure>,
    /// 稳定期开始时刻（页面加载成功后 30s 无新故障则清零）。
    stability_since: Option<Instant>,
    failures_at_load: u32,
    /// 恢复动作在途标记：避免探活失败与心跳判定双计数。
    action_in_flight: bool,
}

impl Default for Inner {
    fn default() -> Self {
        Inner {
            failures: 0,
            window_start: None,
            gave_up: false,
            rebuilt_in_burst: false,
            expecting_web: false,
            user_hidden: true,
            last_heartbeat: None,
            last_failure: None,
            stability_since: None,
            failures_at_load: 0,
            action_in_flight: false,
        }
    }
}

pub struct Recovery {
    inner: Mutex<Inner>,
    pub active: AtomicBool,
}

impl Recovery {
    pub fn new() -> Recovery {
        Recovery {
            inner: Mutex::new(Inner::default()),
            active: AtomicBool::new(true),
        }
    }

    pub fn note_heartbeat(&self) {
        let mut g = self.inner.lock().unwrap();
        g.last_heartbeat = Some(Instant::now());
    }

    pub fn set_user_hidden(&self, hidden: bool) {
        let mut g = self.inner.lock().unwrap();
        g.user_hidden = hidden;
        // 重新可见：给 renderer 宽限恢复心跳（后台节流会让时间戳陈旧）。
        if !hidden {
            g.last_heartbeat = Some(Instant::now());
        }
    }

    /// 页面加载成功且 URL 是 Web UI：进入心跳监控 + 稳定期判定。
    pub fn note_web_loaded(&self) {
        let mut g = self.inner.lock().unwrap();
        if g.gave_up {
            return;
        }
        g.expecting_web = true;
        g.failures_at_load = g.failures;
        g.stability_since = Some(Instant::now());
        g.action_in_flight = false;
    }

    /// 页面切到本地页（loading/recovery）：退出心跳监控。
    pub fn note_local_page(&self) {
        let mut g = self.inner.lock().unwrap();
        g.expecting_web = false;
        g.stability_since = None;
    }

    pub fn set_action_in_flight(&self, v: bool) {
        self.inner.lock().unwrap().action_in_flight = v;
    }

    /// 用户在恢复页点「重新加载」：清零并立即重试。
    pub fn retry_now(&self) {
        let mut g = self.inner.lock().unwrap();
        g.failures = 0;
        g.window_start = None;
        g.gave_up = false;
        g.rebuilt_in_burst = false;
        g.last_failure = None;
        g.action_in_flight = true;
    }

    pub fn state_of(&self) -> RecoveryState {
        let g = self.inner.lock().unwrap();
        RecoveryState {
            failures: g.failures,
            gave_up: g.gave_up,
            expecting_web: g.expecting_web,
            last_failure: g.last_failure.clone(),
        }
    }

    /// 周期体检（15s）：心跳丢失 + 稳定期到期判定。返回需要执行的恢复动作。
    pub fn check(&self) -> Option<RecoveryAction> {
        let mut g = self.inner.lock().unwrap();
        // 稳定期到期：无新故障则清零（脏检查对齐 JS 版）。
        if let Some(since) = g.stability_since {
            if since.elapsed() >= Duration::from_millis(STABILITY_MS) {
                if g.failures == g.failures_at_load {
                    g.failures = 0;
                    g.window_start = None;
                    g.rebuilt_in_burst = false;
                    g.last_failure = None;
                }
                g.stability_since = None;
            }
        }
        if g.gave_up || !g.expecting_web || g.user_hidden || g.action_in_flight {
            return None;
        }
        // 心跳丢失 → 挂起/崩溃。
        if let Some(last) = g.last_heartbeat {
            if last.elapsed() >= Duration::from_millis(HEARTBEAT_MISS_MS) {
                return Some(Self::count_and_decide(&mut g, "heartbeat-miss"));
            }
        } else if g.expecting_web {
            // 从未收到心跳（注入脚本失效/极早期崩溃）。
            return Some(Self::count_and_decide(&mut g, "no-heartbeat"));
        }
        None
    }

    /// 服务探活失败（expecting_web 期间）：等价 did-fail-load 路径。
    pub fn note_probe_failed(&self) -> Option<RecoveryAction> {
        let mut g = self.inner.lock().unwrap();
        if g.gave_up || !g.expecting_web || g.user_hidden || g.action_in_flight {
            return None;
        }
        Some(Self::count_and_decide(&mut g, "load-failed"))
    }

    fn count_and_decide(g: &mut Inner, reason: &str) -> RecoveryAction {
        let now = Instant::now();
        match g.window_start {
            Some(ws) if now.duration_since(ws) > Duration::from_millis(ATTEMPT_WINDOW_MS) => {
                g.window_start = Some(now);
                g.failures = 0;
                g.rebuilt_in_burst = false;
            }
            None => g.window_start = Some(now),
            _ => {}
        }
        g.failures += 1;
        g.last_failure = Some(LastFailure {
            reason: reason.into(),
            at: crate::watchdog::iso_now(),
        });
        let action = next_action(g.failures, g.rebuilt_in_burst);
        if action == RecoveryAction::Rebuild {
            g.rebuilt_in_burst = true;
        }
        if action == RecoveryAction::GiveUp {
            g.gave_up = true;
        } else {
            g.action_in_flight = true;
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_curve() {
        // 与 renderer-recovery.js computeBackoff 同曲线：cap = min(MAX, BASE*2^(n-1))，+15%~35% 抖动。
        assert_eq!(compute_backoff(0), FIRST_DELAY_MS);
        assert_eq!(compute_backoff(1), FIRST_DELAY_MS);
        let d2 = compute_backoff(2);
        assert!(
            d2 >= BACKOFF_BASE_MS * 2 && d2 <= BACKOFF_BASE_MS * 270 / 100,
            "d2={}",
            d2
        );
        let d5 = compute_backoff(5);
        assert!(
            d5 >= BACKOFF_MAX_MS && d5 <= BACKOFF_MAX_MS * 135 / 100,
            "d5={}",
            d5
        );
    }

    #[test]
    fn action_ladder() {
        assert_eq!(next_action(1, false), RecoveryAction::Reload);
        assert_eq!(next_action(2, false), RecoveryAction::Reload);
        assert_eq!(next_action(3, false), RecoveryAction::Rebuild);
        assert_eq!(next_action(3, true), RecoveryAction::Reload);
        assert_eq!(next_action(4, true), RecoveryAction::Reload);
        assert_eq!(next_action(5, true), RecoveryAction::GiveUp);
    }

    #[test]
    fn machine_counts_and_gives_up() {
        let r = Recovery::new();
        r.note_web_loaded();
        r.set_user_hidden(false);
        r.set_action_in_flight(false);
        // 连续故障推进决策梯。
        let a1 = r.note_probe_failed();
        assert_eq!(a1, Some(RecoveryAction::Reload));
        r.set_action_in_flight(false);
        let a2 = r.note_probe_failed();
        assert_eq!(a2, Some(RecoveryAction::Reload));
        r.set_action_in_flight(false);
        let a3 = r.note_probe_failed();
        assert_eq!(a3, Some(RecoveryAction::Rebuild));
        r.set_action_in_flight(false);
        let _a4 = r.note_probe_failed();
        r.set_action_in_flight(false);
        let a5 = r.note_probe_failed();
        assert_eq!(a5, Some(RecoveryAction::GiveUp));
        assert!(r.state_of().gave_up);
        // give-up 后不再产生动作。
        assert_eq!(r.note_probe_failed(), None);
    }

    #[test]
    fn retry_now_resets() {
        let r = Recovery::new();
        r.note_web_loaded();
        r.set_user_hidden(false);
        for _ in 0..5 {
            r.note_probe_failed();
            r.set_action_in_flight(false);
        }
        assert!(r.state_of().gave_up);
        r.retry_now();
        assert!(!r.state_of().gave_up);
        assert_eq!(r.state_of().failures, 0);
    }
}
