//! 原生对话框：MessageBoxW 实现（双按钮「是/否」+ 单按钮告知）。
//!
//! 曾用 TaskDialogIndirect 提供多按钮+勾选框，但真机实测它与微信输入法
//! （WeType）经 TSF 注入的组件冲突，调用即触发 ntdll 堆访问违规
//! （bug #11：点 X 必崩 0xc0000005@ntdll 0x9f5bd；其携带的 CrashRpt
//! 上报器接管后还会弹「Error launching CrashSender.exe」）。且基于模块
//! 快照的事前注入检测不可靠（注入时机晚于检测点），故全平台统一改用
//! MessageBoxW：双按钮场景以「是/否」承载并在正文标注语义；
//! 「记住我的选择」勾选框暂缺（后续可用自绘窗口补齐）。所有原生弹窗
//! 由调用方保证在主线程弹出（见 close_flow / show_dialog_on_main）。

use std::path::Path;

#[derive(Clone, Copy, PartialEq)]
pub enum DialogIcon {
    Error,
    Info,
    Question,
    Warning,
}

pub struct DialogSpec {
    pub title: String,
    pub message: String,
    pub detail: String,
    pub buttons: Vec<String>,
    pub default_index: usize,
    /// 兼容字段：MessageBoxW 无勾选框，恒返回 unchecked。
    pub checkbox: Option<(String, bool)>,
    pub icon: DialogIcon,
    /// false 时禁用 Esc/X 关闭（对应 Electron cancelId: -1 的退出确认弹窗）。
    pub cancellable: bool,
}

pub struct DialogResult {
    /// 按钮下标（buttons 顺序）；关闭/取消 = cancel_index。
    pub index: usize,
    pub cancel: bool,
    pub checked: bool,
}

#[cfg(windows)]
pub fn show(parent_hwnd: isize, spec: &DialogSpec) -> DialogResult {
    // 形状对齐 Electron 版 dialog.showMessageBox(parent, opts)；当前各调用
    // 点均无真实父窗需求（传 0）。
    let _ = parent_hwnd;
    show_message_box(spec)
}

#[cfg(windows)]
fn show_message_box(spec: &DialogSpec) -> DialogResult {
    use windows::core::HSTRING;
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDYES, MB_DEFBUTTON1, MB_DEFBUTTON2, MB_ICONERROR, MB_ICONINFORMATION,
        MB_ICONWARNING, MB_OK, MB_YESNO,
    };
    let icon = match spec.icon {
        DialogIcon::Error => MB_ICONERROR,
        DialogIcon::Warning | DialogIcon::Question => MB_ICONWARNING,
        DialogIcon::Info => MB_ICONINFORMATION,
    };
    // 双按钮（退出确认：最小化/退出）→ 是/否语义并在正文标注映射；
    // 其余（单/多按钮）→ 单 OK + 默认按钮语义（多按钮的附加动作降级不可用，
    // 关键信息须已在 message/detail 正文中可达）。
    let (text, style) = if spec.buttons.len() == 2 {
        (
            HSTRING::from(format!(
                "{}\n\n{}\n\n「是」= {}；「否」= {}",
                spec.message, spec.detail, spec.buttons[1], spec.buttons[0]
            )),
            icon | MB_YESNO
                | if spec.default_index == 1 {
                    MB_DEFBUTTON2
                } else {
                    MB_DEFBUTTON1
                },
        )
    } else {
        (
            HSTRING::from(format!("{}\n\n{}", spec.message, spec.detail)),
            icon | MB_OK,
        )
    };
    let caption = HSTRING::from(&spec.title);
    let r = unsafe { MessageBoxW(None, &text, &caption, style) };
    if spec.buttons.len() == 2 {
        // 「否」是真实选择（如退出确认里的最小化到后台），不算取消。
        DialogResult {
            index: if r == IDYES { 1 } else { 0 },
            cancel: false,
            checked: false,
        }
    } else {
        DialogResult {
            index: spec.default_index.min(spec.buttons.len().saturating_sub(1)),
            cancel: false,
            checked: false,
        }
    }
}

#[cfg(not(windows))]
pub fn show(_parent_hwnd: isize, spec: &DialogSpec) -> DialogResult {
    eprintln!("[dialog] {} — {}", spec.title, spec.message);
    DialogResult {
        index: spec.default_index,
        cancel: false,
        checked: false,
    }
}

/// buildErrorDetail 移植：错误 + 日志位置，同时是「复制日志」的剪贴板内容。
pub fn build_error_detail(err: &str, logs_dir: &Path, log_files: &[&str]) -> String {
    let mut lines = vec![format!("错误：{}", err)];
    lines.push(String::new());
    lines.push(format!("日志目录：{}", logs_dir.display()));
    for f in log_files {
        lines.push(format!("日志文件：{}", f));
    }
    lines.join("\n")
}
