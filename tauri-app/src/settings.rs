//! settings.json 读写（updater.js 的 loadSettings/saveSettings 对应物）。
//! 用 serde_json::Value 透传未知字段，保证与 sidecar/旧版本互不丢数据；
//! 序列化格式与 JS 版一致（2 空格缩进 + 结尾换行）。
//! 刻意不做进程内缓存：sidecar 与壳双端都会写这个文件，直读保证一致。

use std::path::Path;

pub fn load_at(file: &Path) -> serde_json::Value {
    read_from_disk(file)
}

fn read_from_disk(file: &Path) -> serde_json::Value {
    match std::fs::read_to_string(file) {
        Ok(text) => {
            serde_json::from_str(&text).unwrap_or(serde_json::Value::Object(Default::default()))
        }
        Err(_) => serde_json::Value::Object(Default::default()),
    }
}

pub fn save_at(file: &Path, value: &serde_json::Value) -> Result<(), String> {
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())? + "\n";
    // 原子写：tmp + rename，避免半截 JSON。
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, text.as_bytes()).map_err(|e| e.to_string())?;
    match std::fs::rename(&tmp, file) {
        Ok(()) => {}
        Err(_) => {
            // Windows 上 rename 覆盖已存在文件偶发 PermissionError，退化为直写。
            std::fs::write(file, text.as_bytes()).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_file(&tmp);
        }
    }
    Ok(())
}

pub fn get_bool(file: &Path, key: &str, default: bool) -> bool {
    load_at(file)
        .get(key)
        .and_then(|v| v.as_bool())
        .unwrap_or(default)
}

pub fn set_key(file: &Path, key: &str, value: serde_json::Value) -> Result<(), String> {
    let mut doc = load_at(file);
    if !doc.is_object() {
        doc = serde_json::Value::Object(Default::default());
    }
    if let Some(obj) = doc.as_object_mut() {
        obj.insert(key.to_string(), value);
    }
    save_at(file, &doc)
}

/// 退出行为三档：ask（每次询问）/ minimize（后台运行）/ quit（直接退出）。
/// 兼容旧 closeToTray 布尔开关迁移逻辑（与 main.js getExitAction 一致）。
pub fn exit_action_of(doc: &serde_json::Value) -> String {
    match doc.get("exitAction").and_then(|v| v.as_str()) {
        Some(v @ ("ask" | "minimize" | "quit")) => return v.to_string(),
        _ => {}
    }
    match doc.get("closeToTray").and_then(|v| v.as_bool()) {
        Some(false) => "quit".into(),
        Some(true) => "minimize".into(),
        None => "ask".into(),
    }
}

pub fn shortcut_policy_of(doc: &serde_json::Value) -> &'static str {
    if doc.get("shortcutPolicy").and_then(|v| v.as_str()) == Some("never") {
        "never"
    } else {
        "auto"
    }
}
