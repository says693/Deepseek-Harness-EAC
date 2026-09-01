//! bundle-integrity.js 移植（issue #7）：对照打包期 manifest 逐包数文件，
//! 检测升级中断留下的空包骨架，给出「程序文件受损」明确提示而不是
//! ERR_MODULE_NOT_FOUND 死循环。

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// FNV-1a 64 位哈希（无外部依赖），用于 bundle-manifest 内容指纹。
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 递归统计目录下文件数（符号链接计为文件）。
pub fn count_files(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut n = 0;
    for e in entries.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            n += count_files(&e.path());
        } else {
            n += 1; // 符号链接也计入（is_dir 为 false）
        }
    }
    n
}

/// 构建清单：顶层包 + @scope/* 包（深度 2），键为完整包名。
pub fn build_bundle_manifest(nm_root: &Path) -> Value {
    let mut packages: BTreeMap<String, Value> = BTreeMap::new();
    if let Ok(entries) = std::fs::read_dir(nm_root) {
        for e in entries.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if !ft.is_dir() || ft.is_symlink() {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('@') {
                if let Ok(scoped) = std::fs::read_dir(e.path()) {
                    for s in scoped.flatten() {
                        let Ok(sft) = s.file_type() else { continue };
                        if !sft.is_dir() || sft.is_symlink() {
                            continue;
                        }
                        let sub = s.file_name().to_string_lossy().to_string();
                        packages.insert(
                            format!("{}/{}", name, sub),
                            serde_json::json!({ "files": count_files(&s.path()) }),
                        );
                    }
                }
            } else {
                packages.insert(name, serde_json::json!({ "files": count_files(&e.path()) }));
            }
        }
    }
    serde_json::json!({ "version": 1, "packages": packages })
}

#[derive(Debug, Clone)]
pub struct Damage {
    pub name: String,
    pub reason: String,
    pub expected: u64,
    pub actual: u64,
}

pub struct VerifyResult {
    pub ok: bool,
    pub skipped: bool,
    pub damaged: Vec<Damage>,
}

/// 校验已安装 node_modules 与 manifest：目录缺失 / package.json 丢失 /
/// 文件数下降都算受损；多出的文件容忍（只有丢失会破坏模块解析）。
pub fn verify_bundle(nm_root: &Path, manifest: Option<&Value>) -> VerifyResult {
    let Some(manifest) = manifest else {
        return VerifyResult {
            ok: true,
            skipped: true,
            damaged: vec![],
        };
    };
    let Some(packages) = manifest.get("packages").and_then(|p| p.as_object()) else {
        return VerifyResult {
            ok: true,
            skipped: true,
            damaged: vec![],
        };
    };
    let mut damaged = Vec::new();
    for (name, meta) in packages {
        let expected = meta.get("files").and_then(|f| f.as_u64()).unwrap_or(0);
        let mut pkg_dir = nm_root.to_path_buf();
        for seg in name.split('/') {
            pkg_dir.push(seg);
        }
        if !pkg_dir.exists() {
            damaged.push(Damage {
                name: name.clone(),
                reason: "missing".into(),
                expected,
                actual: 0,
            });
            continue;
        }
        if !pkg_dir.join("package.json").exists() {
            damaged.push(Damage {
                name: name.clone(),
                reason: "no package.json (empty skeleton)".into(),
                expected,
                actual: count_files(&pkg_dir),
            });
            continue;
        }
        let actual = count_files(&pkg_dir);
        if actual < expected {
            damaged.push(Damage {
                name: name.clone(),
                reason: "files lost".into(),
                expected,
                actual,
            });
        }
    }
    VerifyResult {
        ok: damaged.is_empty(),
        skipped: false,
        damaged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_detects_missing_and_skeleton() {
        let base = std::env::temp_dir().join(format!("dsh-integ-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let nm = base.join("node_modules");
        // 健康树：两个包各有 package.json + 文件。
        std::fs::create_dir_all(nm.join("@scope/pkg/lib")).unwrap();
        std::fs::write(nm.join("@scope/pkg/package.json"), "{}").unwrap();
        std::fs::write(nm.join("@scope/pkg/lib/a.js"), "").unwrap();
        std::fs::create_dir_all(nm.join("other")).unwrap();
        std::fs::write(nm.join("other/package.json"), "{}").unwrap();
        std::fs::write(nm.join("other/b.js"), "").unwrap();

        let manifest = build_bundle_manifest(&nm);
        let r = verify_bundle(&nm, Some(&manifest));
        assert!(r.ok);
        assert!(!r.skipped);

        // 模拟损坏：丢文件 + 清成骨架。
        std::fs::remove_file(nm.join("@scope/pkg/lib/a.js")).unwrap();
        std::fs::remove_file(nm.join("other/package.json")).unwrap();
        let r2 = verify_bundle(&nm, Some(&manifest));
        assert!(!r2.ok);
        assert!(r2
            .damaged
            .iter()
            .any(|d| d.name == "@scope/pkg" && d.reason == "files lost"));
        assert!(r2
            .damaged
            .iter()
            .any(|d| d.name == "other" && d.reason.contains("skeleton")));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_manifest_skips() {
        let r = verify_bundle(Path::new("Z:/nonexistent"), None);
        assert!(r.ok && r.skipped);
    }
}
