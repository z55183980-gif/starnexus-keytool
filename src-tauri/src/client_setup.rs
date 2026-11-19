//! Manual installation guides, terminal helpers and opt-in version checks.
use crate::{
    error::{AppError, Result},
    launcher::{self, Target},
};
use semver::Version;
use serde::Serialize;
use std::time::Duration;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Guide {
    platform: String,
    url: &'static str,
    command: Option<&'static str>,
    shell: &'static str,
    note: &'static str,
    variants: Vec<CommandVariant>,
    can_run: bool,
}

#[derive(Serialize)]
pub struct CommandVariant { shell: &'static str, command: &'static str }

fn guide(target: Target, platform: &str) -> Guide {
    let mut result = match target {
        Target::ClaudeCli => Guide { platform: platform.into(),
            url: "https://code.claude.com/docs/en/setup",
            command: Some(if platform == "windows" { "irm https://claude.ai/install.ps1 | iex" } else { "curl -fsSL https://claude.ai/install.sh | bash" }),
            shell: if platform == "windows" { "PowerShell" } else { "Terminal" },
            note: "使用官方安装器；更新方式见官方说明。",
            variants: vec![], can_run: false,
        },
        Target::CodexCli => Guide { platform: platform.into(),
            url: "https://github.com/openai/codex#installation",
            command: Some(if platform == "windows" { "npm.cmd install -g @openai/codex@latest" } else { "npm install -g @openai/codex@latest" }),
            shell: if platform == "windows" { "PowerShell" } else { "Terminal" },
            note: "需 Node.js 和 npm；同一命令可更新 npm 安装。",
            variants: vec![], can_run: false,
        },
        Target::ClaudeDesktop => Guide { platform: platform.into(), url: "https://claude.com/download", command: None, shell: "", note: "从官网下载，安装后重新检测。", variants: vec![], can_run: false },
        Target::CodexDesktop => Guide { platform: platform.into(), url: "https://developers.openai.com/codex/app", command: None, shell: "", note: "从官网下载，安装后重新检测。", variants: vec![], can_run: false },
    };
    if let Some(command) = result.command {
        result.shell = if platform == "windows" { "PowerShell 5.1 / 7+" } else { "Terminal · zsh / bash" };
        result.variants.push(CommandVariant { shell: result.shell, command });
        if platform == "windows" {
            result.variants.push(CommandVariant { shell: "CMD", command: if target == Target::ClaudeCli {
                "curl -fsSL https://claude.ai/install.cmd -o install.cmd && install.cmd && del install.cmd"
            } else { "npm install -g @openai/codex@latest" } });
        }
        result.can_run = matches!(platform, "windows" | "macos");
    }
    result
}

#[tauri::command]
pub fn client_install_guide(target: Target) -> Guide {
    guide(target, std::env::consts::OS)
}

#[tauri::command]
pub fn open_client_download(app: tauri::AppHandle, target: Target) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(guide(target, std::env::consts::OS).url, None::<&str>)
        .map_err(|_| AppError::Other("无法打开官方页面，请检查默认浏览器".into()))
}

fn launch_setup_terminal(script: &str) -> Result<()> {
        use std::process::Command;
        #[cfg(target_os = "windows")]
        let mut command = {
            use base64::Engine;
            let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
            let executable = std::path::PathBuf::from(root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
            let bootstrap = powershell_bootstrap(&executable.to_string_lossy(), script);
            let mut command = Command::new(executable);
            command.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-EncodedCommand"])
                .arg(base64::engine::general_purpose::STANDARD.encode(bootstrap.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>()));
            command
        };
        #[cfg(target_os = "macos")]
        let mut command = { let mut command = Command::new("/usr/bin/osascript"); command.args(["-e", "tell application \"Terminal\" to activate", "-e", &terminal_applescript(script)]); command };
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        { let _ = script; return Err(AppError::Other("此系统请复制命令到终端执行。".into())); }
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            if let Some(home) = dirs::home_dir() { command.current_dir(home); }
            let result = crate::process::output(&mut command, Duration::from_secs(15))?;
            if !result.status.success() { return Err(AppError::Other("无法打开终端，请手动打开后执行命令。".into())); }
            Ok(())
        }
}

#[cfg(any(windows, test))]
fn powershell_bootstrap(executable: &str, script: &str) -> String {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(script.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>());
    // Visible interactive terminal uses fresh console handles; helper stays hidden.
    format!("$ErrorActionPreference='Stop'; Start-Process -FilePath '{}' -ArgumentList @('-NoLogo','-NoProfile','-NoExit','-EncodedCommand','{}')", executable.replace('\'', "''"), encoded)
}

#[cfg(any(target_os = "macos", test))]
fn terminal_applescript(script: &str) -> String {
    launcher::mac_script_dispatch(script)
}

#[tauri::command]
pub async fn open_setup_terminal() -> Result<()> {
    crate::commands::blocking(|| launch_setup_terminal("")).await
}

pub(crate) fn parse_version_output(text: &str) -> Option<String> {
    text.lines()
        .take(8)
        .filter(|line| {
            let line = line.trim();
            line.starts_with("codex-cli ") || line.ends_with("(Claude Code)") || line.split_whitespace().count() == 1
        })
        .flat_map(str::split_whitespace)
        .filter_map(|s| {
            Version::parse(s.trim_matches(['(', ')', ',']).trim_start_matches('v')).ok()
        })
        .next()
        .map(|v| v.to_string())
}

pub(crate) fn target_version(target: Target, text: &str) -> Option<String> {
    text.lines().take(8).filter(|line| match target {
        Target::CodexCli => line.trim().starts_with("codex-cli "),
        Target::ClaudeCli => line.trim().ends_with("(Claude Code)"),
        _ => false,
    }).find_map(parse_version_output)
}

pub(crate) fn release_version(document: &[u8]) -> Result<String> {
    let data: serde_json::Value = serde_json::from_slice(document)?;
    if data["draft"].as_bool() != Some(false) || data["prerelease"].as_bool() != Some(false) {
        return Err(AppError::Other("最新稳定版本信息不可用".into()));
    }
    let tag = data["tag_name"]
        .as_str()
        .ok_or_else(|| AppError::Other("最新稳定版本信息不可用".into()))?;
    let raw = tag
        .strip_prefix("rust-v")
        .or_else(|| tag.strip_prefix('v'))
        .unwrap_or(tag);
    let version =
        Version::parse(raw).map_err(|_| AppError::Other("最新稳定版本信息不可用".into()))?;
    if !version.pre.is_empty() {
        return Err(AppError::Other("最新稳定版本信息不可用".into()));
    }
    Ok(version.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientVersion {
    pub verified_builtin: bool,
    pub current: String,
    pub latest: Option<String>,
    pub update_available: Option<bool>,
    pub path: String,
    pub warning: Option<&'static str>,
}

fn version_status(current: String, path: String, latest: Result<String>) -> ClientVersion {
    let latest = latest.ok();
    let update_available = latest.as_ref().and_then(|v| newer_version(v, &current).ok());
    ClientVersion {
        verified_builtin: crate::codex_config::verified_builtin(Some(&current)),
        current,
        path,
        update_available,
        warning: update_available
            .is_none()
            .then_some("更新检查失败，当前显示本机版本。"),
        latest,
    }
}

/// Semver for CLI releases, numeric tuples for Windows/macOS desktop versions.
pub(crate) fn newer_version(candidate: &str, current: &str) -> Result<bool> {
    if let (Ok(candidate), Ok(current)) = (Version::parse(candidate), Version::parse(current)) {
        return Ok(candidate.cmp_precedence(&current).is_gt());
    }
    fn parts(value: &str) -> Option<[u64;4]> {
        let mut result=[0;4]; let values:Vec<_>=value.split('.').collect();
        if !(2..=4).contains(&values.len()) { return None; }
        for (i,value) in values.iter().enumerate() {
            if value.is_empty() || !value.bytes().all(|b|b.is_ascii_digit()) { return None; }
            result[i]=value.parse().ok()?;
        }
        Some(result)
    }
    match (parts(candidate),parts(current)) {
        (Some(candidate),Some(current)) => Ok(candidate>current),
        _ => Err(AppError::Other("无法比较版本，请检查更新后重试。".into())),
    }
}

static CHECK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
#[tauri::command]
pub async fn check_client_version(target: Target, ignore_proxy: bool, check_latest: Option<bool>) -> Result<ClientVersion> {
    crate::updates::ensure_supported()?;
    let _guard = CHECK.lock().await;
    let _ = ignore_proxy; // Installation metadata uses the same route as downloads: official if reachable, otherwise the relay.
    let (current, path) = crate::commands::blocking(move || launcher::installed_version(target)).await?
        .ok_or_else(|| AppError::Other("未找到客户端，请先安装或设置程序路径".into()))?;
    if !check_latest.unwrap_or(true) {
        return Ok(ClientVersion { verified_builtin: target == Target::CodexCli && crate::codex_config::verified_builtin(Some(&current)), current, path, latest: None, update_available: None, warning: None });
    }
    Ok(version_status(
        current,
        path,
        crate::client_download::latest_available_version(target).await,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manual_guides_cover_windows_and_mac_shells() {
        let windows = guide(Target::ClaudeCli, "windows");
        assert_eq!(windows.variants.len(), 2);
        assert_eq!(windows.variants[1].shell, "CMD");
        assert!(guide(Target::CodexCli, "windows").command.unwrap().starts_with("npm.cmd "));
        for target in [Target::ClaudeCli, Target::CodexCli] {
            assert!(guide(target, "macos").can_run);
            assert!(!guide(target, "linux").can_run);
            assert!(guide(target, "macos").shell.contains("zsh / bash"));
        }
    }
    #[test]
    fn terminal_wrappers_quote_paths_and_preserve_scripts() {
        use base64::Engine;
        let script = "Write-Host '安装'; $x=\"a&b\"";
        let bootstrap = powershell_bootstrap("C:\\用户\\it's\\powershell.exe", script);
        assert!(bootstrap.contains("it''s"));
        assert!(bootstrap.contains("-NoExit"));
        let encoded = base64::engine::general_purpose::STANDARD.encode(script.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>());
        assert!(bootstrap.contains(&encoded));
        let apple = terminal_applescript("echo '安装'; echo \"a&b\"");
        assert!(apple.starts_with("tell application \"Terminal\" to do script \"/bin/bash --noprofile --norc -c "));
        assert!(apple.contains("\\\""));
    }
    #[test]
    fn version_parsing_and_stable_release_comparison() {
        assert!(!newer_version("0.169.0","0.169.0").unwrap());
        assert!(!newer_version("0.168.0","0.169.0").unwrap());
        assert!(newer_version("0.170.0","0.169.0").unwrap());
        assert!(newer_version("0.169.0","0.169.0-beta.1").unwrap());
        assert!(newer_version("26.1112.1.0","26.1111.2228.0").unwrap());
        assert!(!newer_version("5.11492.5","5.11492.5.0").unwrap());
        assert!(newer_version("invalid","0.169.0").is_err());
        assert_eq!(
            parse_version_output("codex-cli 0.99.0\n"),
            Some("0.99.0".into())
        );
        assert_eq!(
            parse_version_output("2.1.89 (Claude Code)"),
            Some("2.1.89".into())
        );
        assert_eq!(parse_version_output("command not found"), None);
        assert!(target_version(Target::CodexCli, "2.1.20 (Claude Code)").is_none());
        assert!(target_version(Target::ClaudeCli, "codex-cli 0.100.0").is_none());
        assert!(target_version(Target::CodexCli, "0.100.0").is_none());
        assert_eq!(parse_version_output("Warning: requires Node 22.0.0\ncodex-cli 0.99.0"), Some("0.99.0".into()));
        assert_eq!(parse_version_output("Warning: requires Node 22.0.0"), None);
        assert_eq!(
            release_version(br#"{"tag_name":"rust-v0.100.0","draft":false,"prerelease":false}"#)
                .unwrap(),
            "0.100.0"
        );
        for data in [
            r#"{"tag_name":"v2.0.0-beta.1","draft":false,"prerelease":false}"#,
            r#"{"tag_name":"v2.0.0","draft":true,"prerelease":false}"#,
            "{}",
        ] {
            assert!(release_version(data.as_bytes()).is_err());
        }
        assert_eq!(
            version_status("0.99.0".into(), "path".into(), Ok("0.100.0".into())).update_available,
            Some(true)
        );
        assert_eq!(
            version_status("2.0.0+custom".into(), "path".into(), Ok("2.0.0".into()))
                .update_available,
            Some(false)
        );
        let offline = version_status(
            "1.0.0".into(),
            "path".into(),
            Err(AppError::Other("offline".into())),
        );
        assert_eq!(offline.current, "1.0.0");
        assert!(offline.update_available.is_none() && offline.warning.is_some());
    }
    #[test]
    fn official_guides_are_platform_specific_and_never_accept_commands_from_the_ui() {
        assert_eq!(
            guide(Target::ClaudeCli, "windows").command,
            Some("irm https://claude.ai/install.ps1 | iex")
        );
        assert!(guide(Target::ClaudeCli, "macos")
            .command
            .unwrap()
            .contains("install.sh"));
        assert!(guide(Target::CodexCli, "windows")
            .command
            .unwrap()
            .contains("@openai/codex@latest"));
        for target in Target::all() {
            assert!(guide(target, "windows").url.starts_with("https://"));
        }
        assert!(guide(Target::ClaudeDesktop, "windows").command.is_none());
        assert!(guide(Target::CodexDesktop, "macos").command.is_none());
    }
}
