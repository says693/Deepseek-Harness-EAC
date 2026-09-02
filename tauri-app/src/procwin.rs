//! Windows 进程管理原语：Job Object 兜底查杀、taskkill 优雅→强杀两段式、
//! 存活检测、分离拉起。对应 main.js 的 killTree / killTreeAndWait /
//! waitForProcExit（M2/V4 修复语义完整保留）。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(windows)]
use windows::Win32::Foundation::{CloseHandle, HANDLE};
#[cfg(windows)]
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
#[cfg(windows)]
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
};

/// taskkill 优雅段等待时长（M2：给进程收尾机会，避免撕裂 zstd 会话尾）。
pub const GRACE_MS: u64 = 1200;
/// 强杀后的兜底等待。
pub const HARD_MS: u64 = 4000;

pub fn alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(windows)]
    {
        unsafe {
            let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return false;
            };
            let _ = CloseHandle(h);
            true
        }
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// taskkill /pid X /T（无 /F：投递关闭，控制台进程多半无效但无害）。
fn taskkill(pid: u32, force: bool) {
    let mut args = vec!["/pid".to_string(), pid.to_string(), "/T".to_string()];
    if force {
        args.push("/F".to_string());
    }
    let mut cmd = Command::new("taskkill");
    cmd.args(&args).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let _ = cmd.spawn();
}

/// 有界强回收：优雅 taskkill → 等 grace → 仍存活则 /T /F → 再等 hard。
/// 全程有界，绝不无限阻塞退出（V4「退出后残留一对进程」修复）。
pub fn kill_tree_and_wait(child: &mut Child, grace: Duration, hard: Duration) {
    let pid = child.id();
    taskkill(pid, false);
    if wait_child_exit(child, grace) {
        return;
    }
    taskkill(pid, true);
    let _ = wait_child_exit(child, hard);
}

/// 只知道 PID 时（如市场排队任务子进程）的有界强杀。
pub fn kill_pid_tree_and_wait(pid: u32, grace: Duration, hard: Duration) {
    if pid == 0 || !alive(pid) {
        return;
    }
    taskkill(pid, false);
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if !alive(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    taskkill(pid, true);
    let deadline = Instant::now() + hard;
    while Instant::now() < deadline {
        if !alive(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn wait_child_exit(child: &mut Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => {}
            Err(_) => return true,
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// HANDLE 不是 Send；Job 句柄由 ServiceHandle 跨线程持有，用 newtype 声明
/// 线程安全（句柄本身可跨线程使用，windows 句柄无线程亲和性）。
#[cfg(windows)]
pub struct JobHandle(pub HANDLE);
#[cfg(windows)]
unsafe impl Send for JobHandle {}
#[cfg(windows)]
unsafe impl Sync for JobHandle {}

/// 把子进程挂进 KILL_ON_JOB_CLOSE 的 Job：壳进程无论怎么死（崩溃/任务管理器
/// 结束），dsh web 进程树都会被内核回收 —— 比纯 taskkill 多一层保险。
/// 先设 limit 再挂进程；任何一步失败都立即销毁 Job 并返回错误说明。
#[cfg(windows)]
pub fn assign_job(child_pid: u32) -> Result<JobHandle, String> {
    unsafe {
        let job = CreateJobObjectW(None, None).map_err(|e| format!("CreateJobObject: {e}"))?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if let Err(e) = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) {
            let _ = CloseHandle(job);
            return Err(format!("SetInformation(KILL_ON_JOB_CLOSE): {e}"));
        }
        // AssignProcessToJobObject 要求句柄具备 PROCESS_SET_QUOTA | PROCESS_TERMINATE。
        let Ok(proc_h) = OpenProcess(
            PROCESS_SET_QUOTA | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            child_pid,
        ) else {
            let _ = CloseHandle(job);
            return Err("OpenProcess(SET_QUOTA|TERMINATE|QUERY)".into());
        };
        if let Err(e) = AssignProcessToJobObject(job, proc_h) {
            let code = e.code();
            let _ = CloseHandle(proc_h);
            let _ = CloseHandle(job);
            return Err(format!("Assign: {e} (code={:?})", code));
        }
        let _ = CloseHandle(proc_h);
        Ok(JobHandle(job))
    }
}

/// 诊断用：进程当前是否处于任意 Job 中（IsProcessInJob，job=NULL 查任意 Job）。
#[cfg(windows)]
pub fn process_in_any_job(pid: u32) -> bool {
    use windows::Win32::Foundation::BOOL;
    use windows::Win32::System::JobObjects::IsProcessInJob;
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut in_job = BOOL::default();
        let ok = IsProcessInJob(h, None, &mut in_job).is_ok();
        let _ = CloseHandle(h);
        ok && in_job.as_bool()
    }
}
#[cfg(not(windows))]
pub fn process_in_any_job(_pid: u32) -> bool {
    true
}

/// 分离启动（看门狗/重启客户端）：不随父进程退出而被杀。
pub fn spawn_detached(
    exe: &str,
    args: &[String],
    cwd: Option<&std::path::Path>,
) -> std::io::Result<()> {
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .current_dir(cwd.unwrap_or_else(|| std::path::Path::new(".")))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NO_WINDOW
        cmd.creation_flags(0x0000_0008 | 0x0800_0000);
    }
    cmd.spawn().map(|_| ())
}

/// 复用 netprobe 的探测（供 service.rs 与 recovery.rs 共用）。
pub fn http_probe_ok(port: u16, timeout: Duration) -> bool {
    crate::netprobe::probe_localhost(port, timeout)
}
