//! 全局应用状态：壳层各模块共享的句柄与标志位。

use crate::logging::Logger;
use crate::paths::Paths;
use crate::recovery::Recovery;
use crate::service::ServiceHandle;
use crate::sidecar::Sidecar;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::AppHandle;

pub struct AppState {
    pub paths: Paths,
    pub log: std::sync::Arc<crate::logging::Logger>,
    pub sidecar: OnceLock<Sidecar>,
    pub service: Mutex<Option<Arc<ServiceHandle>>>,
    pub web_url: Mutex<Option<String>>,
    pub quitting: AtomicBool,
    pub force_quit: AtomicBool,
    pub restarting: AtomicBool,
    pub shutdown_started: AtomicBool,
    pub recovery: Recovery,
    pub balance_cache: Mutex<Option<serde_json::Value>>,
    /// koffi 预检失败时注入的目录选择器降级 overlay 路径。
    pub picker_overlay: Mutex<Option<String>>,
    pub tray_ready: AtomicBool,
    /// junction 修复通知只发一次。
    pub junction_notified: AtomicBool,
    /// AppHandle 在 setup 阶段回填（AppState 先于 app 存在）。
    pub app: OnceLock<AppHandle>,
}

impl AppState {
    pub fn new(paths: Paths, log: std::sync::Arc<crate::logging::Logger>) -> AppState {
        AppState {
            paths,
            log,
            sidecar: OnceLock::new(),
            service: Mutex::new(None),
            web_url: Mutex::new(None),
            quitting: AtomicBool::new(false),
            force_quit: AtomicBool::new(false),
            restarting: AtomicBool::new(false),
            shutdown_started: AtomicBool::new(false),
            recovery: Recovery::new(),
            balance_cache: Mutex::new(None),
            picker_overlay: Mutex::new(None),
            tray_ready: AtomicBool::new(false),
            junction_notified: AtomicBool::new(false),
            app: OnceLock::new(),
        }
    }

    pub fn sidecar(&self) -> Result<&Sidecar, String> {
        self.sidecar.get().ok_or_else(|| "sidecar 未运行".into())
    }

    pub fn app_handle(&self) -> Option<AppHandle> {
        self.app.get().cloned()
    }

    /// 系统通知（tauri-plugin-notification）。tag 仅用于日志。
    pub fn notify(&self, tag: &str, title: &str, body: &str) -> Result<(), String> {
        self.log.log(tag, &format!("通知: {} — {}", title, body));
        use tauri_plugin_notification::NotificationExt;
        match self.app_handle() {
            Some(app) => app
                .notification()
                .builder()
                .title(title.to_string())
                .body(body.to_string())
                .show()
                .map_err(|e| e.to_string()),
            None => Err("app 未就绪".into()),
        }
    }

    pub fn service_running(&self) -> bool {
        match self.service.lock() {
            Ok(g) => match g.as_ref() {
                Some(h) => {
                    !h.exited.load(Ordering::SeqCst) && !h.intentional.load(Ordering::SeqCst)
                }
                None => false,
            },
            Err(_) => false,
        }
    }
}
