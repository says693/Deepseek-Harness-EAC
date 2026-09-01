//! 路径解析：开发模式直接用仓库根（与 Electron 版共享 vendor/node_modules），
//! 打包模式用 Tauri resource 目录下 staging 脚本铺好的 app/node/npm。

use std::path::PathBuf;

pub struct Paths {
    /// 应用 JS 根（shell-host.js / assets / node_modules 所在目录）。
    pub app_root: PathBuf,
    /// 内置 node.exe。
    pub node_exe: PathBuf,
    /// 内置 npm CLI 入口。
    pub npm_cli: PathBuf,
    /// 用户数据目录（%APPDATA%/<identifier>，Tauri app_data_dir）。
    pub user_data: PathBuf,
    /// 日志目录。
    pub logs_dir: PathBuf,
    /// DSH_HOME（env 显式覆盖优先，否则使用本产品独立数据目录）。
    pub dsh_home: PathBuf,
    pub packaged: bool,
    pub version: String,
}

impl Paths {
    pub fn new(
        packaged: bool,
        resource_dir: Option<PathBuf>,
        app_data_dir: PathBuf,
        version: String,
    ) -> Paths {
        // Tauri v2 在 Windows 上把 resources/**/* 落在 <exe目录>\resources\ 下，
        // 而 resource_dir() 返回 exe 目录本身 —— 必须补上 resources 层，
        // 否则打包版找不到 shell-host/assets/node_modules（真机验证发现）。
        // 另外 resource_dir() 返回 \\?\ 开头的 verbatim 路径，Node 解析主入口会炸（EISDIR: lstat 'D:'），
        // 这里统一剥掉前缀（dunce 手法）。
        let res = strip_verbatim(resource_dir.clone().unwrap_or_default().join("resources"));
        let app_root = if packaged {
            res.join("app")
        } else {
            // dev：tauri-app 的上一级即仓库根
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default()
        };
        let node_exe = if packaged {
            res.join("node").join("node.exe")
        } else {
            app_root.join("vendor").join("node").join("node.exe")
        };
        let npm_cli = if packaged {
            res.join("npm").join("bin").join("npm-cli.js")
        } else {
            app_root
                .join("vendor")
                .join("npm")
                .join("bin")
                .join("npm-cli.js")
        };
        // 便携包在 exe 同级带 `.dsh-portable`，数据进入 `.dsh-aio-data`；
        // 安装版继续使用独立 appData。环境变量始终优先，供自动化与高级用户覆盖。
        let portable_data = portable_data_dir(packaged, resource_dir.as_deref());
        let user_data = match std::env::var("DSH_DESKTOP_USERDATA") {
            Ok(v) if !v.trim().is_empty() => PathBuf::from(v),
            _ => portable_data.clone().unwrap_or(app_data_dir),
        };
        let dsh_home = match std::env::var("DSH_HOME") {
            Ok(v) if !v.trim().is_empty() => PathBuf::from(v),
            _ => portable_data
                .map(|dir| dir.join("dsh-home"))
                .unwrap_or_else(|| user_data.join("dsh-home")),
        };
        Paths {
            app_root,
            node_exe,
            npm_cli,
            logs_dir: user_data.join("logs"),
            user_data,
            dsh_home,
            packaged,
            version,
        }
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.app_root.join("assets")
    }

    /// 当前生效的 dsh bin：用户目录更新覆盖层优先，内置副本兜底。
    pub fn dsh_bin(&self) -> PathBuf {
        let overlay = self
            .user_data
            .join("agent")
            .join("node_modules")
            .join("@deepseek-ai")
            .join("dsh")
            .join("lib")
            .join("bin.js");
        if overlay.exists() {
            return overlay;
        }
        self.app_root
            .join("node_modules")
            .join("@deepseek-ai")
            .join("dsh")
            .join("lib")
            .join("bin.js")
    }

    pub fn desktop_profile(&self) -> String {
        // 共享模式（settings.shareWebProfile === true）走官方 web profile；
        // 默认桌面专属 profile 与原生 CLI 彻底共存。settings 由调用方注入，
        // 这里读一次文件即可（低频路径）。
        let s = crate::settings::load_at(&self.settings_file());
        match s.get("shareWebProfile").and_then(|v| v.as_bool()) {
            Some(true) => "web".to_string(),
            _ => DESKTOP_PROFILE.to_string(),
        }
    }

    pub fn desktop_profile_dir(&self) -> PathBuf {
        self.dsh_home.join("profiles").join(self.desktop_profile())
    }

    pub fn settings_file(&self) -> PathBuf {
        self.user_data.join("settings.json")
    }

    pub fn run_state_file(&self) -> PathBuf {
        self.user_data.join("run-state.json")
    }

    pub fn koffi_overlay_file(&self) -> PathBuf {
        self.user_data.join("picker-browse.overlay.yml")
    }

    /// Copy the packaged current-profile snapshot on first launch. The copy is
    /// intentionally one-shot so later user changes are never overwritten.
    pub fn seed_distribution_profile(&self) -> Result<bool, String> {
        let seed = self
            .app_root
            .parent()
            .ok_or_else(|| "resource root is unavailable".to_string())?
            .join("profile-seed");
        if !seed.exists() || self.desktop_profile_dir().join("node_modules").exists() {
            return Ok(false);
        }
        copy_tree(&seed, &self.dsh_home)?;
        Ok(true)
    }
}

pub const DESKTOP_PROFILE: &str = "web-desktop";
/// 与官方 web profile 出厂模板一致（@deepseek-ai/dsh-base + dsh-web-app）。
pub const DESKTOP_PROFILE_BUNDLES: [&str; 2] =
    ["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-web-app"];

pub fn dirs_home() -> Option<PathBuf> {
    std::env::var("USERPROFILE").ok().map(PathBuf::from)
}

fn copy_tree(source: &std::path::Path, destination: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(destination)
        .map_err(|e| format!("create {}: {e}", destination.display()))?;
    for entry in std::fs::read_dir(source).map_err(|e| format!("read {}: {e}", source.display()))? {
        let entry = entry.map_err(|e| format!("read entry in {}: {e}", source.display()))?;
        let src = entry.path();
        let dst = destination.join(entry.file_name());
        let kind = entry
            .file_type()
            .map_err(|e| format!("inspect {}: {e}", src.display()))?;
        if kind.is_dir() {
            copy_tree(&src, &dst)?;
        } else if kind.is_file() {
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("create {}: {e}", parent.display()))?;
            }
            std::fs::copy(&src, &dst)
                .map_err(|e| format!("copy {} -> {}: {e}", src.display(), dst.display()))?;
        }
    }
    Ok(())
}

/// 剥掉 Windows verbatim 前缀（\\?\ 与 \\?\UNC\）。
/// Tauri 的路径解析器会返回 verbatim 路径；Node 把以 \\?\ 开头的主入口
/// 参数解析失败（EISDIR lstat 'D:'），spawn 子进程前必须还原普通路径。
fn portable_data_dir(packaged: bool, resource_dir: Option<&std::path::Path>) -> Option<PathBuf> {
    if !packaged {
        return None;
    }
    resource_dir
        .filter(|dir| dir.join(".dsh-portable").is_file())
        .map(|dir| dir.join(".dsh-aio-data"))
}

fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.as_os_str().to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_marker_selects_sibling_data_root() {
        let root = std::env::temp_dir().join(format!("dsh-aio-portable-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        std::fs::write(root.join(".dsh-portable"), b"").unwrap();
        assert_eq!(portable_data_dir(true, Some(&root)), Some(root.join(".dsh-aio-data")));
        assert_eq!(portable_data_dir(false, Some(&root)), None);
        let _ = std::fs::remove_dir_all(root);
    }
}
