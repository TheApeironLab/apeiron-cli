use super::*;
use std::{
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Instant,
};

#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn check(&self) -> Result<()> {
        if self.0.load(Ordering::Acquire) {
            Err(fail("操作已取消。", 499))
        } else {
            Ok(())
        }
    }
}

// Drain stdout concurrently so a full pipe cannot deadlock timeout/cancellation.
// Child diagnostics are deliberately discarded: helpers may handle credentials.
pub fn capture(
    command: &mut Command,
    input: &[u8],
    timeout: Duration,
    limit: usize,
    cancel: &Cancellation,
) -> Result<Vec<u8>> {
    cancel.check()?;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|_| fail("无法启动所需的本地工具。", 502))?;
    let mut stdin = child.stdin.take().unwrap();
    let input = input.to_vec();
    let writer = thread::spawn(move || stdin.write_all(&input));
    let stdout = child.stdout.take().unwrap();
    let reader = thread::spawn(move || {
        let mut data = Vec::new();
        stdout.take(limit as u64 + 1).read_to_end(&mut data).map(|_| data)
    });
    let start = Instant::now();
    let result = loop {
        if cancel.check().is_err() || start.elapsed() >= timeout {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            break Err(fail("操作已取消或超过时间限制。", 502));
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                break if status.success() {
                    Ok(())
                } else {
                    Err(fail("本地工具未确认操作成功。", 502))
                }
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(fail("无法等待本地工具退出。", 502));
            }
        }
    };
    let _ = writer.join();
    let bytes = reader
        .join()
        .map_err(|_| fail("无法读取工具输出。", 502))?
        .map_err(|_| fail("无法读取工具输出。", 502))?;
    result?;
    if bytes.len() > limit {
        return Err(fail("工具响应超过大小限制。", 502));
    }
    Ok(bytes)
}
pub fn text(command: &mut Command, input: &[u8], seconds: u64, limit: usize, cancel: &Cancellation) -> Result<String> {
    String::from_utf8(capture(command, input, Duration::from_secs(seconds), limit, cancel)?)
        .map_err(|_| fail("工具响应格式不正确。", 502))
}
pub fn private_directory(path: &Path) -> Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|_| fail("无法创建私有工作目录。", 500))
}
pub fn home() -> Result<std::path::PathBuf> {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| fail("HOME 未配置。", 400))
}
pub fn config_path(value: Option<&String>) -> Result<std::path::PathBuf> {
    let path = match value {
        Some(v) => std::path::PathBuf::from(v),
        None => home()?.join(".apeiron/config.json"),
    };
    if path.to_string_lossy().chars().any(char::is_control) {
        return Err(fail("配置路径不能包含控制字符。", 400));
    }
    Ok(if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|_| fail("无法读取当前目录。", 500))?
            .join(path)
    })
}
