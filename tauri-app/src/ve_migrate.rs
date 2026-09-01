//! 可选 localStorage 跨壳迁移。AIO 默认完全隔离，不读取其他版本数据；
//! 只有用户显式设置 `DSH_AIO_IMPORT_LEGACY=1` 时，才读取旧 Electron 导出
//! 并经 initialization_script 写入 AIO Web origin。写 stamp 幂等：只迁一次。
//!
//! 导出文件查找顺序：
//!   1. `%APPDATA%\Deepseek Harness EAC`（Electron 版真实 userData）
//!   2. 本应用 userdata（测试场景：两代共用 DSH_DESKTOP_USERDATA 重定位目录）

use crate::paths::Paths;
use crate::settings;
use std::path::{Path, PathBuf};

const MAX_EXPORT_BYTES: u64 = 5 * 1024 * 1024;
const STAMP_FILE: &str = "localstorage-migrated.stamp";
const EXPORT_FILE: &str = "dsh-localstorage-export.json";

/// stamp 已写入则视为迁移完成。
fn migrated(stamp: &Path) -> bool {
    stamp.exists()
}

fn mark_migrated(stamp: &Path) {
    if let Some(dir) = stamp.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(stamp, crate::watchdog::iso_now());
}

/// Electron 版 userData 目录（真实安装场景）：%APPDATA%\Deepseek Harness EAC。
fn electron_userdata() -> Option<PathBuf> {
    std::env::var("APPDATA")
        .ok()
        .map(|d| PathBuf::from(d).join("Deepseek Harness EAC"))
}

/// 生成迁移 initialization_script。
/// 返回 None 的情形：已迁移 / 无导出文件 / 超 5MB / 顶层不是 JSON object。
/// 只有成功生成脚本时才写 stamp；文件损坏等异常情形不写（下次启动再试）。
pub fn migration_script_for(export_file: &Path, stamp_file: &Path) -> Option<String> {
    if migrated(stamp_file) {
        return None;
    }
    let meta = std::fs::metadata(export_file).ok()?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_EXPORT_BYTES {
        return None;
    }
    let raw = std::fs::read_to_string(export_file).ok()?;
    let doc: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let obj = doc.as_object()?;
    if obj.is_empty() {
        // 空对象也算迁移完成（用户本来就没有偏好数据）。
        mark_migrated(stamp_file);
        return None;
    }
    // JSON 字面量直接内嵌（ES2019 起 JSON 是合法 JS 表达式）。
    let script = format!(
        "(function(){{if(window.__dshVeMigrated){{return;}}var d={};try{{for(var k in d){{try{{localStorage.setItem(k,d[k]);}}catch(e){{}}}}window.__dshVeMigrated=true;}}catch(e){{}}}})();",
        raw.trim()
    );
    mark_migrated(stamp_file);
    Some(script)
}

/// 按查找顺序定位导出文件并生成迁移脚本；结果记日志。
pub fn load_migration_script(paths: &Paths, log: &crate::logging::Logger) -> Option<String> {
    if std::env::var("DSH_AIO_IMPORT_LEGACY").as_deref() != Ok("1") {
        return None;
    }
    let stamp = paths.user_data.join(STAMP_FILE);
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(eu) = electron_userdata() {
        candidates.push(eu.join(EXPORT_FILE));
    }
    candidates.push(paths.user_data.join(EXPORT_FILE));
    for f in candidates {
        if let Some(script) = migration_script_for(&f, &stamp) {
            log.log(
                "boot",
                &format!(
                    "检测到 Electron 版 localStorage 导出（{}），将在页面加载时迁移",
                    f.display()
                ),
            );
            return Some(script);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dsh-ve-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn migrates_and_marks() {
        let d = tmpdir("ok");
        let export = d.join("export.json");
        let stamp = d.join("stamp");
        std::fs::write(&export, r#"{"ui.pane":"left","dsh-ve-json":"{\"a\":1}"}"#).unwrap();
        let s1 = migration_script_for(&export, &stamp);
        assert!(s1.is_some(), "首次应生成脚本");
        let s = s1.unwrap();
        assert!(s.contains(r#""ui.pane":"left""#), "脚本应内嵌键值");
        assert!(s.contains("localStorage.setItem"));
        assert!(stamp.exists(), "成功后应写 stamp");
        assert!(
            migration_script_for(&export, &stamp).is_none(),
            "二次调用应幂等跳过"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn legacy_import_is_opt_in() {
        let previous = std::env::var("DSH_AIO_IMPORT_LEGACY").ok();
        std::env::remove_var("DSH_AIO_IMPORT_LEGACY");
        let d = tmpdir("opt-in");
        let paths = Paths::new(false, None, d.clone(), "1.0.0".into());
        let log = crate::logging::Logger::open(&d.join("logs"));
        assert!(load_migration_script(&paths, &log).is_none());
        match previous {
            Some(value) => std::env::set_var("DSH_AIO_IMPORT_LEGACY", value),
            None => std::env::remove_var("DSH_AIO_IMPORT_LEGACY"),
        }
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn skips_bad_or_missing() {
        let d = tmpdir("bad");
        let stamp = d.join("stamp");
        assert!(
            migration_script_for(&d.join("nope.json"), &stamp).is_none(),
            "无文件应 None"
        );
        let bad = d.join("bad.json");
        std::fs::write(&bad, "not json").unwrap();
        assert!(
            migration_script_for(&bad, &stamp).is_none(),
            "坏 JSON 应 None"
        );
        assert!(!stamp.exists(), "失败不应写 stamp");
        let empty = d.join("empty.json");
        std::fs::write(&empty, "{}").unwrap();
        assert!(
            migration_script_for(&empty, &stamp).is_none(),
            "空对象视为已迁移"
        );
        assert!(stamp.exists(), "空对象应写 stamp");
        let _ = std::fs::remove_dir_all(&d);
    }
}
