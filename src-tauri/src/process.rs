//! Bounded waits for short-lived system helpers. Never use for interactive clients.
use crate::error::{AppError, Result};
use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

pub const HELPER_TIMEOUT: Duration = Duration::from_secs(5);
const OUTPUT_LIMIT: usize = 64 * 1024;

fn drain(mut pipe: impl Read) -> std::io::Result<Vec<u8>> {
    let mut result = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let n = pipe.read(&mut buffer)?;
        if n == 0 { return Ok(result); }
        let keep = n.min(OUTPUT_LIMIT.saturating_sub(result.len()));
        result.extend_from_slice(&buffer[..keep]);
    }
}

pub fn output(command: &mut Command, timeout: Duration) -> Result<Output> {
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let (tx, rx) = std::sync::mpsc::channel();
    let err_tx = tx.clone();
    std::thread::spawn(move || { let _ = tx.send((true, drain(stdout))); });
    std::thread::spawn(move || { let _ = err_tx.send((false, drain(stderr))); });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(match result {
                    Err(e) => e.into(),
                    _ => AppError::Other("系统辅助程序执行超时，请稍后重试".into()),
                });
            }
        }
    };
    let mut out = Output { status, stdout: Vec::new(), stderr: Vec::new() };
    for _ in 0..2 {
        let (is_stdout, bytes) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| AppError::Other("读取系统辅助程序输出超时".into()))?;
        if is_stdout { out.stdout = bytes?; } else { out.stderr = bytes?; }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "subprocess fixture, run by helper tests"]
    fn child_fixture() {
        match std::env::var("ZAI_HELPER_TEST").as_deref() {
            Ok("sleep") => std::thread::sleep(Duration::from_secs(10)),
            Ok("output") => { print!("{}", "x".repeat(200_000)); eprint!("fixture-error"); },
            _ => panic!("fixture requires explicit test mode"),
        }
    }
    fn fixture(mode: &str) -> Command {
        let mut c = Command::new(std::env::current_exe().unwrap());
        c.args(["--exact", "process::tests::child_fixture", "--ignored", "--nocapture"])
            .env("ZAI_HELPER_TEST", mode);
        c
    }
    #[test]
    fn kills_timed_out_helper_and_returns_promptly() {
        let start = Instant::now();
        assert!(output(&mut fixture("sleep"), Duration::from_millis(500)).is_err());
        assert!(start.elapsed() < Duration::from_secs(5));
    }
    #[test]
    fn drains_both_pipes_and_bounds_retained_output() {
        let o = output(&mut fixture("output"), Duration::from_secs(5)).unwrap();
        assert!(o.status.success());
        assert_eq!(o.stdout.len(), OUTPUT_LIMIT);
        assert!(String::from_utf8_lossy(&o.stderr).contains("fixture-error"));
    }
}
