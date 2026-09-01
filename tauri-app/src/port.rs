//! stable-port.js 移植：稳定端口选择（保 localStorage 按 origin 隔离的用户
//! 偏好）+ Chromium 受限端口表（ERR_UNSAFE_PORT 会白屏）。

use crate::settings;
use std::net::TcpListener;
use std::path::Path;

/// Chromium bad-port list（截取常见的系统/保留端口，完整列表见
/// chromium net/base/port_util.cc）。
pub const CHROMIUM_RESTRICTED_PORTS: &[u16] = &[
    1, 7, 9, 11, 13, 15, 17, 19, 20, 21, 22, 23, 25, 37, 42, 43, 53, 69, 77, 79, 87, 95, 101, 102,
    103, 104, 109, 110, 111, 113, 115, 117, 119, 123, 135, 137, 139, 143, 161, 179, 389, 427, 465,
    512, 513, 514, 515, 526, 530, 531, 532, 540, 548, 554, 556, 563, 587, 601, 636, 989, 990, 993,
    995, 1719, 1720, 1723, 2049, 3659, 4045, 4190, 5060, 5061, 6000, 6566, 6665, 6666, 6667, 6668,
    6669, 6697, 10080,
];

/// url 命中受限端口时返回该端口号，否则返回 0。
pub fn restricted_port_of(url: &str) -> u16 {
    let port = extract_port(url);
    if CHROMIUM_RESTRICTED_PORTS.contains(&port) {
        port
    } else {
        0
    }
}

/// 从 URL 提取端口；无显式端口按协议默认（http=80 / https=443）。
pub fn extract_port(url: &str) -> u16 {
    // 手工解析，避免为单个端口引入 url crate：
    // scheme://host[:port]/...
    let (scheme, rest) = match url.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r),
        None => return 0,
    };
    let default_port = if scheme == "https" { 443 } else { 80 };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // 去掉 userinfo
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    // IPv6 字面量 [::1]:80
    if let Some(close) = authority.rfind(']') {
        let after = &authority[close + 1..];
        return after
            .strip_prefix(':')
            .and_then(|p| p.parse().ok())
            .unwrap_or(default_port);
    }
    match authority.rsplit_once(':') {
        Some((_, p)) => p.parse().unwrap_or(default_port),
        None => default_port,
    }
}

fn port_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// 选一个尽量稳定的 127.0.0.1 端口并持久化到 settings.webPort。
/// 逻辑与 stable-port.js 一致：优先复用已存端口；不可用或受限则随机挑空闲
/// 端口（最多 max_free_retries 次，命中受限表重挑；耗尽回落 0 = dsh 随机）。
pub fn choose_stable_web_port(settings_file: &Path) -> u16 {
    let max_free_retries = 5u32;
    let doc = settings::load_at(settings_file);
    let preferred = doc.get("webPort").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
    let save = |port: u16| {
        let _ = settings::set_key(settings_file, "webPort", serde_json::json!(port));
        port
    };
    if preferred > 0 && !CHROMIUM_RESTRICTED_PORTS.contains(&preferred) && port_free(preferred) {
        return save(preferred);
    }
    for _ in 0..max_free_retries {
        if let Ok(l) = TcpListener::bind(("127.0.0.1", 0)) {
            if let Ok(port) = l.local_addr().map(|a| a.port()) {
                drop(l);
                // 关闭后立刻复测：降低 TOCTOU 窗口（与 JS 版 probe.close 语义一致）。
                if !CHROMIUM_RESTRICTED_PORTS.contains(&port) && port_free(port) {
                    return save(port);
                }
                continue;
            }
        }
    }
    save(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restricted_ports() {
        assert_eq!(restricted_port_of("http://127.0.0.1:6000/web"), 6000);
        assert_eq!(restricted_port_of("http://127.0.0.1:6666/"), 6666);
        assert_eq!(restricted_port_of("http://127.0.0.1:18080/"), 0);
        assert_eq!(restricted_port_of("not a url"), 0);
    }

    #[test]
    fn port_extraction() {
        assert_eq!(extract_port("http://127.0.0.1:5173/x"), 5173);
        assert_eq!(extract_port("http://127.0.0.1/"), 80);
        assert_eq!(extract_port("https://[::1]:8443/"), 8443);
        assert_eq!(extract_port("https://example.com"), 443);
    }
}
