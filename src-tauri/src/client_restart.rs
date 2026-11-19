//! 写入配置后重启桌面客户端。
//!
//! Claude 桌面版和 Codex 桌面版都只在启动时读配置，运行中的那个不会重读：
//! 改完配置要整个退出（包括托盘里的后台进程）再打开，新配置才生效。
//! 这里只认桌面版自己的安装位置，不碰终端里运行的 Claude Code / Codex CLI。

use crate::error::{AppError, Result};
use crate::launcher::{LaunchReceipt, LaunchRequest, Target};
use std::time::{Duration, Instant};

#[cfg(windows)]
mod win {
    /// 桌面版主程序（判断是否在运行）
    pub const CLAUDE_MAIN: &str = r"\\WindowsApps\\Claude_[^\\]+\\app\\claude\.exe$|\\AnthropicClaude\\(app-[^\\]+\\)?claude\.exe$|\\Programs\\Claude\\Claude\.exe$";
    /// 桌面版安装目录下的全部进程（退出时一起结束，否则托盘和后台服务还占着单实例锁）
    pub const CLAUDE_ALL: &str = r"\\WindowsApps\\Claude_[^\\]+\\|\\AnthropicClaude\\|\\Programs\\Claude\\";
    /// Codex 桌面版在新版 Windows 上显示为 ChatGPT；OpenAI\Codex\bin 下的是 CLI，不算
    pub const CODEX_MAIN: &str = r"\\WindowsApps\\OpenAI\.Codex_[^\\]+\\app\\(ChatGPT|Codex)\.exe$|\\Programs\\Codex\\Codex\.exe$|\\OpenAI\\Codex\\Codex\.exe$";
    pub const CODEX_ALL: &str = r"\\WindowsApps\\OpenAI\.Codex_[^\\]+\\|\\Programs\\Codex\\|\\OpenAI\\Codex\\Codex\.exe$";
}

#[cfg(target_os = "macos")]
fn bundle_id(target: Target) -> &'static str {
    if target == Target::ClaudeDesktop { "com.anthropic.claudefordesktop" } else { "com.openai.codex" }
}

fn desktop(target: Target) -> Result<()> {
    if matches!(target, Target::ClaudeDesktop | Target::CodexDesktop) {
        Ok(())
    } else {
        Err(AppError::Other("只能重启桌面版客户端".into()))
    }
}

#[cfg(windows)]
fn powershell(script: &str) -> Option<std::process::Output> {
    use std::os::windows::process::CommandExt;
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let ps = std::path::PathBuf::from(root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    crate::process::output(
        std::process::Command::new(ps)
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .creation_flags(0x08000000),
        crate::process::HELPER_TIMEOUT,
    )
    .ok()
}

/// 进程路径匹配正则的 PowerShell 筛选（正则是本文件里的常量，不含单引号）
#[cfg(windows)]
fn matching(pattern: &str) -> String {
    format!("Get-Process -ErrorAction SilentlyContinue | Where-Object {{ $_.Path -and $_.Path -match '{pattern}' }}")
}

/// macOS：LaunchServices 里这个应用的进程号（不需要自动化权限）
#[cfg(target_os = "macos")]
fn mac_pids(target: Target) -> Vec<u32> {
    let Ok(out) = crate::process::output(
        std::process::Command::new("/usr/bin/lsappinfo").args(["info", "-only", "pid", bundle_id(target)]),
        crate::process::HELPER_TIMEOUT,
    ) else {
        return Vec::new();
    };
    parse_lsappinfo_pids(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(any(target_os = "macos", test))]
fn parse_lsappinfo_pids(text: &str) -> Vec<u32> {
    text.split("\"pid\"=")
        .skip(1)
        .filter_map(|rest| rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok())
        .filter(|pid| *pid > 0)
        .collect()
}

/// 桌面版是否在运行
pub fn running(target: Target) -> bool {
    if target == Target::ClaudeDesktop && crate::claude_desktop::running() {
        return true;
    }
    #[cfg(windows)]
    {
        let main = if target == Target::ClaudeDesktop { win::CLAUDE_MAIN } else { win::CODEX_MAIN };
        return powershell(&format!("@({}).Count", matching(main)))
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u32>().ok())
            .is_some_and(|n| n > 0);
    }
    #[cfg(target_os = "macos")]
    {
        return !mac_pids(target).is_empty();
    }
    #[allow(unreachable_code)]
    {
        let _ = target;
        false
    }
}

fn wait_until_stopped(target: Target, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if !running(target) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    !running(target)
}

/// 退出桌面版（连同后台进程）。Windows 上桌面版关窗口只是缩到托盘，只能结束进程。
fn stop(target: Target) -> Result<()> {
    #[cfg(windows)]
    {
        let all = if target == Target::ClaudeDesktop { win::CLAUDE_ALL } else { win::CODEX_ALL };
        powershell(&format!("{} | Stop-Process -Force -ErrorAction SilentlyContinue", matching(all)));
    }
    #[cfg(target_os = "macos")]
    {
        // 先发 SIGTERM，让它像从菜单退出一样收尾；不退再强制结束
        let signal = |sig: &str| {
            for pid in mac_pids(target) {
                let _ = crate::process::output(
                    std::process::Command::new("/bin/kill").args([sig, &pid.to_string()]),
                    crate::process::HELPER_TIMEOUT,
                );
            }
        };
        signal("-TERM");
        if !wait_until_stopped(target, Duration::from_secs(8)) {
            signal("-KILL");
        }
    }
    if wait_until_stopped(target, Duration::from_secs(10)) {
        Ok(())
    } else {
        Err(AppError::Other("没能退出客户端，请手动完全退出（包括托盘图标）后再打开".into()))
    }
}

/// 正在运行的桌面客户端（只查传入的几个）
#[tauri::command]
pub async fn running_desktop_clients(targets: Vec<Target>) -> Vec<Target> {
    tauri::async_runtime::spawn_blocking(move || {
        targets.into_iter().filter(|t| desktop(*t).is_ok() && running(*t)).collect()
    })
    .await
    .unwrap_or_default()
}

/// 退出并重新打开桌面客户端，让它读到新配置
#[tauri::command]
pub async fn restart_desktop_client(target: Target) -> Result<LaunchReceipt> {
    tauri::async_runtime::spawn_blocking(move || restart(target))
        .await
        .map_err(|e| AppError::Other(e.to_string()))?
}

pub fn restart(target: Target) -> Result<LaunchReceipt> {
    desktop(target)?;
    if running(target) {
        stop(target)?;
        // 单实例锁和托盘图标释放需要一点时间，太快拉起会被旧实例的残留吞掉
        std::thread::sleep(Duration::from_millis(800));
    }
    crate::launcher::launch(LaunchRequest { target, directory: String::new(), service: None })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchOutcome {
    /// 改动前的备份；本来就是第三方模式时为空
    pub backup_id: Option<String>,
    pub message: String,
}

/// 桌面版里是否记着用户选了 Claude.ai 官方登录（用 API 启动前要先切到第三方推理）
#[tauri::command]
pub async fn claude_desktop_official_login() -> bool {
    tauri::async_runtime::spawn_blocking(crate::claude_desktop::official_login_chosen).await.unwrap_or(false)
}

/// 用户选了 Claude.ai 官方登录时，一键切到第三方推理：退出桌面版 → 备份并改选择 → 重新打开。
/// 必须先退出：运行中的桌面版会回写这个文件，先改会被它覆盖。
#[tauri::command]
pub async fn switch_claude_desktop_to_third_party() -> Result<SwitchOutcome> {
    tauri::async_runtime::spawn_blocking(switch_to_third_party)
        .await
        .map_err(|e| AppError::Other(e.to_string()))?
}

fn switch_to_third_party() -> Result<SwitchOutcome> {
    use crate::claude_desktop;
    let target = Target::ClaudeDesktop;
    let backup_id = {
        let _guard = crate::backup::lock()?;
        let path = claude_desktop::app_config_path()?;
        // 先确认能改（格式损坏就不动桌面版），再退出
        claude_desktop::with_third_party_mode(crate::atomic::read_text_optional(&path)?.as_deref())?;
        if running(target) {
            stop(target)?;
            std::thread::sleep(Duration::from_millis(800));
        }
        // 退出时桌面版可能刚回写过，重新读一次
        let before = crate::atomic::read_text_optional(&path)?;
        let after = claude_desktop::with_third_party_mode(before.as_deref())?;
        if claude_desktop::chose_official_login(before.as_deref()) || before.is_none() {
            let id = crate::backup::create_for_paths(std::slice::from_ref(&path))?;
            crate::atomic::write_atomic(&path, &after)?;
            Some(id)
        } else {
            None
        }
    };
    crate::launcher::launch(LaunchRequest { target, directory: String::new(), service: None }).map_err(|e| {
        AppError::Other(format!("已切换到第三方推理，但重新打开 Claude 桌面版失败，请手动打开：{e}"))
    })?;
    Ok(SwitchOutcome { backup_id, message: "Claude 桌面版已切换到第三方推理并重新打开".into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pids_from_lsappinfo() {
        assert_eq!(parse_lsappinfo_pids("\"pid\"=4242\n"), vec![4242]);
        assert_eq!(parse_lsappinfo_pids("\"pid\"=12 \"pid\"=34"), vec![12, 34]);
        assert!(parse_lsappinfo_pids("").is_empty());
        assert!(parse_lsappinfo_pids("\"pid\"=[ NULL ]").is_empty());
    }

    #[test]
    fn only_desktop_targets_can_be_restarted() {
        assert!(desktop(Target::ClaudeDesktop).is_ok());
        assert!(desktop(Target::CodexDesktop).is_ok());
        assert!(desktop(Target::ClaudeCli).is_err());
        assert!(desktop(Target::CodexCli).is_err());
    }

    /// 实测：真的退出并重新打开本机的 Codex 桌面版（不改任何配置）。
    /// 手动运行：cargo test --lib client_restart::tests::restart_codex_desktop_for_real -- --ignored --nocapture
    #[test]
    #[ignore = "会真的重启本机 Codex 桌面版"]
    fn restart_codex_desktop_for_real() {
        let before = running(Target::CodexDesktop);
        eprintln!("codex desktop running before: {before}");
        let started = Instant::now();
        let receipt = restart(Target::CodexDesktop).expect("restart failed");
        eprintln!("restart returned after {:?}: {}", started.elapsed(), receipt.message);
        let deadline = Instant::now() + Duration::from_secs(30);
        while !running(Target::CodexDesktop) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(500));
        }
        assert!(running(Target::CodexDesktop), "codex desktop did not come back");
        eprintln!("codex desktop running again after {:?}", started.elapsed());
    }

    /// 只读：确认匹配规则能认出本机正在运行的桌面版、且不会把 CLI 算进去
    #[test]
    #[cfg(windows)]
    #[ignore = "依赖本机正在运行的客户端"]
    fn detects_running_desktops_on_this_machine() {
        eprintln!("claude desktop running: {}", running(Target::ClaudeDesktop));
        eprintln!("codex desktop running: {}", running(Target::CodexDesktop));
    }

    #[test]
    #[cfg(windows)]
    fn windows_patterns_match_desktop_apps_but_not_the_cli() {
        let re = regex_lite;
        let codex_main = re(win::CODEX_MAIN);
        assert!(codex_main(r"C:\Program Files\WindowsApps\OpenAI.Codex_26.1111.2228.0_x64__2p2nqsd0c76g0\app\ChatGPT.exe"));
        assert!(!codex_main(r"C:\Program Files\WindowsApps\OpenAI.Codex_26.1111.2228.0_x64__2p2nqsd0c76g0\app\resources\codex.exe"));
        assert!(!codex_main(r"C:\Users\a\AppData\Local\OpenAI\Codex\bin\0.170.0\codex.exe"));
        let codex_all = re(win::CODEX_ALL);
        assert!(!codex_all(r"C:\Users\a\AppData\Local\OpenAI\Codex\bin\0.170.0\codex.exe"));
        assert!(!codex_all(r"C:\Users\a\AppData\Roaming\npm\node_modules\@openai\codex\codex.exe"));
        let claude_main = re(win::CLAUDE_MAIN);
        assert!(claude_main(r"C:\Program Files\WindowsApps\Claude_5.11492.5.0_x64__pzs8sxrjxfjjc\app\claude.exe"));
        assert!(!claude_main(r"C:\Program Files\WindowsApps\Claude_5.11492.5.0_x64__pzs8sxrjxfjjc\app\resources\cowork-svc.exe"));
        let claude_all = re(win::CLAUDE_ALL);
        assert!(!claude_all(r"C:\Users\a\AppData\Local\Claude-3p\claude-code\2.1.260\claude.exe"));
        assert!(!claude_all(r"C:\Users\a\.local\bin\claude.exe"));
    }

    /// 用 PowerShell 自己的 -match 校验，和运行时完全一致
    #[cfg(windows)]
    fn regex_lite(pattern: &'static str) -> impl Fn(&str) -> bool {
        move |path: &str| {
            let script = format!("'{}' -match '{}'", path.replace('\'', "''"), pattern);
            powershell(&script).is_some_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "True")
        }
    }
}
