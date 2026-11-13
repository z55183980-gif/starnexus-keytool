//! OS launch adapters. Configuration and credentials stay in the existing config layer.
//! Never interpolate user paths into cmd.exe, and never put credentials in process arguments.
use crate::{
    atomic,
    error::{AppError, Result},
    paths,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
};

static PREFS_LOCK: Mutex<()> = Mutex::new(());
static LAUNCH_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    ClaudeDesktop,
    ClaudeCli,
    CodexDesktop,
    CodexCli,
}
impl Target {
    pub fn all() -> [Self; 4] {
        [
            Self::ClaudeDesktop,
            Self::ClaudeCli,
            Self::CodexDesktop,
            Self::CodexCli,
        ]
    }
    fn cli(self) -> bool {
        matches!(self, Self::ClaudeCli | Self::CodexCli)
    }
    fn binary(self) -> &'static str {
        if matches!(self, Self::ClaudeCli | Self::ClaudeDesktop) {
            "claude"
        } else {
            "codex"
        }
    }
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Desktop,
    #[default]
    Cli,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Preferences {
    pub project_directory: String,
    pub recent_directories: Vec<String>,
    pub claude_mode: Mode,
    pub codex_mode: Mode,
    pub claude_service: String,
    pub codex_service: String,
    pub custom_paths: BTreeMap<Target, String>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            project_directory: dirs::home_dir()
                .map(|p| p.to_string_lossy().into())
                .unwrap_or_default(),
            recent_directories: vec![],
            claude_mode: Mode::Cli,
            codex_mode: Mode::Cli,
            claude_service: "current".into(),
            codex_service: "current".into(),
            custom_paths: BTreeMap::new(),
        }
    }
}
fn preferences_path() -> Result<PathBuf> {
    Ok(paths::app_dir()?.join("launcher.json"))
}
pub fn load_preferences() -> Result<Preferences> {
    match atomic::read_text_optional(&preferences_path()?)? {
        None => Ok(Preferences::default()),
        Some(text) => serde_json::from_str(&text)
            .map_err(|e| AppError::Other(format!("启动偏好读取失败，原文件未修改：{e}"))),
    }
}
#[tauri::command]
pub fn load_launcher_preferences() -> Result<Preferences> {
    load_preferences()
}

#[tauri::command]
pub fn save_launcher_preferences(mut preferences: Preferences) -> Result<()> {
    let _guard = PREFS_LOCK
        .lock()
        .map_err(|_| AppError::Other("启动偏好锁不可用".into()))?;
    // Do not overwrite a corrupt preferences file with silent defaults.
    load_preferences()?;
    preferences.recent_directories.truncate(8);
    preferences
        .custom_paths
        .retain(|_, path| !path.trim().is_empty());
    atomic::write_atomic(
        &preferences_path()?,
        &serde_json::to_string_pretty(&preferences)?,
    )
}

#[derive(Clone)]
enum Location {
    Executable(PathBuf),
    #[cfg(target_os = "windows")]
    AppId(String),
    #[cfg(target_os = "macos")]
    Bundle(PathBuf),
}
impl Location {
    fn display(&self) -> String {
        match self {
            Self::Executable(p) => p.to_string_lossy().into(),
            #[cfg(target_os = "windows")]
            Self::AppId(id) => format!("shell:AppsFolder\\{id}"),
            #[cfg(target_os = "macos")]
            Self::Bundle(p) => p.to_string_lossy().into(),
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Installation {
    pub target: Target,
    pub installed: bool,
    pub path: Option<String>,
    pub message: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Scan {
    pub platform: String,
    pub installations: Vec<Installation>,
}

fn executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return path
            .metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false);
    }
    #[cfg(not(unix))]
    {
        true
    }
}
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|v| {
            std::env::split_paths(&v)
                .filter(|p| p.is_absolute())
                .collect()
        })
        .unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        dirs.extend([
            home.join(".local/bin"),
            home.join(".npm-global/bin"),
            home.join(".bun/bin"),
        ]);
    }
    #[cfg(windows)]
    if let Some(roaming) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(roaming).join("npm"));
    }
    #[cfg(unix)]
    dirs.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
    ]);
    dirs
}
fn find_cli(name: &str) -> Option<PathBuf> {
    for dir in search_dirs() {
        // WindowsApps aliases may point at the desktop client with the same name.
        #[cfg(windows)]
        if dir
            .file_name()
            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("WindowsApps"))
        {
            continue;
        }
        #[cfg(windows)]
        let names = [
            format!("{name}.exe"),
            format!("{name}.ps1"),
            format!("{name}.cmd"),
            format!("{name}.bat"),
        ];
        #[cfg(not(windows))]
        let names = [name.to_owned()];
        for file in names {
            let path = dir.join(file);
            if executable(&path) {
                return Some(path);
            }
        }
    }
    None
}

#[cfg(windows)]
fn system_exe(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join(name)
}
#[cfg(windows)]
fn powershell() -> PathBuf {
    system_exe("System32/WindowsPowerShell/v1.0/powershell.exe")
}

// Only read the registered, exact product names; do not scan or run arbitrary shortcuts.
#[cfg(windows)]
fn packaged_apps() -> BTreeMap<String, String> {
    use std::{
        os::windows::process::CommandExt,
        time::{Duration, Instant},
    };
    let script = "[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new(); Get-StartApps | Where-Object { $_.Name -in @('Claude','Codex') -and $_.AppID -match '^[A-Za-z0-9._-]+![A-Za-z0-9._-]+$' } | ForEach-Object { $_.Name + '|' + $_.AppID }";
    let Ok(mut child) = Command::new(powershell())
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(0x08000000)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return BTreeMap::new();
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < Duration::from_secs(8) => {
                std::thread::sleep(Duration::from_millis(40))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return BTreeMap::new();
            }
        }
    }
    child
        .wait_with_output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter_map(|l| {
                    l.trim()
                        .split_once('|')
                        .map(|(name, id)| (name.to_lowercase(), id.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn custom_location(target: Target, path: &str) -> Result<Location> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(AppError::Other("请选择绝对路径".into()));
    }
    #[cfg(target_os = "macos")]
    if !target.cli() && path.is_dir() && path.extension().is_some_and(|x| x == "app") {
        return Ok(Location::Bundle(path));
    }
    if !executable(&path) {
        return Err(AppError::Other(
            "指定的程序不存在或不可执行，请在设置中重新选择".into(),
        ));
    }
    #[cfg(windows)]
    {
        let ext = path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        if !(ext == "exe" || target.cli() && ["cmd", "bat", "ps1"].contains(&ext.as_str())) {
            return Err(AppError::Other(
                "桌面版请选择 .exe；CLI 请选择 .exe、.ps1 或 .cmd/.bat".into(),
            ));
        }
    }
    #[cfg(not(windows))]
    let _ = target;
    Ok(Location::Executable(path))
}

fn discover(
    target: Target,
    prefs: &Preferences,
    apps: &BTreeMap<String, String>,
) -> Result<Option<Location>> {
    if let Some(path) = prefs
        .custom_paths
        .get(&target)
        .filter(|p| !p.trim().is_empty())
    {
        return custom_location(target, path).map(Some);
    }
    if target.cli() {
        return Ok(find_cli(target.binary()).map(Location::Executable));
    }
    #[cfg(windows)]
    {
        if let Some(id) = apps.get(target.binary()) {
            return Ok(Some(Location::AppId(id.clone())));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let local = PathBuf::from(local);
            let mut candidates = if target == Target::ClaudeDesktop {
                vec![
                    local.join("AnthropicClaude/claude.exe"),
                    local.join("Programs/Claude/Claude.exe"),
                ]
            } else {
                vec![
                    local.join("Programs/Codex/Codex.exe"),
                    local.join("OpenAI/Codex/Codex.exe"),
                ]
            };
            if target == Target::CodexDesktop {
                // The desktop updater keeps versioned installations under OpenAI/Codex/bin.
                let mut versions: Vec<_> = std::fs::read_dir(local.join("OpenAI/Codex/bin"))
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e.ok())
                    .map(|e| e.path().join("codex.exe"))
                    .filter(|p| p.is_file())
                    .collect();
                versions.sort_by_key(|p| {
                    std::cmp::Reverse(p.metadata().and_then(|m| m.modified()).ok())
                });
                candidates.extend(versions);
            }
            if let Some(path) = candidates.into_iter().find(|p| p.is_file()) {
                return Ok(Some(Location::Executable(path)));
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        let name = if target == Target::ClaudeDesktop {
            "Claude.app"
        } else {
            "Codex.app"
        };
        let mut candidates = vec![PathBuf::from("/Applications").join(name)];
        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join("Applications").join(name));
        }
        if let Some(path) = candidates.into_iter().find(|p| p.is_dir()) {
            return Ok(Some(Location::Bundle(path)));
        }
    }
    let _ = apps;
    Ok(None)
}
fn app_inventory() -> BTreeMap<String, String> {
    #[cfg(windows)]
    {
        packaged_apps()
    }
    #[cfg(not(windows))]
    {
        BTreeMap::new()
    }
}
pub fn scan() -> Result<Scan> {
    let prefs = load_preferences()?;
    let apps = app_inventory();
    let installations = Target::all()
        .into_iter()
        .map(|target| match discover(target, &prefs, &apps) {
            Ok(Some(location)) => Installation {
                target,
                installed: true,
                path: Some(location.display()),
                message: "已检测到程序".into(),
            },
            Ok(None) => Installation {
                target,
                installed: false,
                path: None,
                message: "未检测到，请安装或在设置中指定路径".into(),
            },
            Err(e) => Installation {
                target,
                installed: false,
                path: prefs.custom_paths.get(&target).cloned(),
                message: e.to_string(),
            },
        })
        .collect();
    Ok(Scan {
        platform: std::env::consts::OS.into(),
        installations,
    })
}
#[tauri::command]
pub async fn scan_launchers() -> Result<Scan> {
    tauri::async_runtime::spawn_blocking(scan)
        .await
        .map_err(|e| AppError::Other(e.to_string()))?
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchRequest {
    pub target: Target,
    pub directory: String,
}
fn working_directory(value: &str) -> Result<PathBuf> {
    let path = PathBuf::from(value);
    if !path.is_absolute() || !path.is_dir() {
        return Err(AppError::Other("工作目录不存在，请重新选择文件夹".into()));
    }
    Ok(path)
}

#[cfg(any(windows, test))]
fn ps_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
#[cfg(any(unix, test))]
fn sh_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
#[cfg(any(windows, test))]
fn cli_script(executable: &str, directory: &str) -> String {
    format!(
        "$ErrorActionPreference='Stop'; Set-Location -LiteralPath {}; & {}",
        ps_literal(directory),
        ps_literal(executable)
    )
}

fn command_for(location: &Location, target: Target, directory: Option<&Path>) -> Result<Command> {
    #[cfg(windows)]
    {
        use base64::Engine;
        use std::os::windows::process::CommandExt;
        if target.cli() {
            let Location::Executable(path) = location else {
                return Err(AppError::Other("CLI 程序路径无效".into()));
            };
            let dir = directory.ok_or_else(|| AppError::Other("请选择工作目录".into()))?;
            let script = cli_script(&path.to_string_lossy(), &dir.to_string_lossy());
            let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
            let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
            // Start-Process creates fresh console handles. CREATE_NEW_CONSOLE on a Rust
            // child alone still inherits the GUI/dev host's redirected standard handles.
            let bootstrap = format!("$ErrorActionPreference='Stop'; Start-Process -FilePath {} -WorkingDirectory {} -ArgumentList @('-NoLogo','-NoProfile','-NoExit','-ExecutionPolicy','Bypass','-EncodedCommand',{})", ps_literal(&powershell().to_string_lossy()), ps_literal(&dir.to_string_lossy()), ps_literal(&encoded));
            let bootstrap_bytes: Vec<u8> = bootstrap
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect();
            let mut cmd = Command::new(powershell());
            cmd.args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-EncodedCommand",
            ])
            .arg(base64::engine::general_purpose::STANDARD.encode(bootstrap_bytes))
            .current_dir(dir)
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
            return Ok(cmd);
        }
        let mut cmd = match location {
            Location::AppId(id) => {
                let mut c = Command::new(system_exe("explorer.exe"));
                c.arg(format!("shell:AppsFolder\\{id}"));
                c
            }
            Location::Executable(path) => Command::new(path),
        };
        cmd.creation_flags(0x08000000);
        Ok(cmd)
    }
    #[cfg(target_os = "macos")]
    {
        if target.cli() {
            let dir = directory.ok_or_else(|| AppError::Other("请选择工作目录".into()))?;
            let shell = format!(
                "cd -- {} && {}",
                sh_literal(&dir.to_string_lossy()),
                sh_literal(&location.display())
            );
            let apple = shell
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\r', "\\r");
            let mut cmd = Command::new("/usr/bin/osascript");
            cmd.args([
                "-e",
                "tell application \"Terminal\" to activate",
                "-e",
                &format!("tell application \"Terminal\" to do script \"{apple}\""),
            ]);
            return Ok(cmd);
        }
        let mut cmd = Command::new("/usr/bin/open");
        cmd.arg("-a").arg(location.display());
        Ok(cmd)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        if target.cli() {
            let dir = directory.ok_or_else(|| AppError::Other("请选择工作目录".into()))?;
            for name in [
                "x-terminal-emulator",
                "gnome-terminal",
                "konsole",
                "xfce4-terminal",
                "xterm",
            ] {
                if let Some(terminal) = find_cli(name) {
                    let mut cmd = Command::new(terminal);
                    cmd.current_dir(dir);
                    cmd.arg(if name == "gnome-terminal" {
                        "--"
                    } else if name == "xfce4-terminal" {
                        "-x"
                    } else {
                        "-e"
                    });
                    cmd.arg(location.display());
                    return Ok(cmd);
                }
            }
            return Err(AppError::Other(
                "未找到终端，请先安装 xterm 或 GNOME Terminal 等终端".into(),
            ));
        }
        Ok(Command::new(location.display()))
    }
}

fn prepare(req: &LaunchRequest) -> Result<Command> {
    let prefs = load_preferences()?;
    let apps = if req.target.cli() {
        BTreeMap::new()
    } else {
        app_inventory()
    };
    let location = discover(req.target, &prefs, &apps)?
        .ok_or_else(|| AppError::Other("未找到应用，请安装或在设置中指定程序路径".into()))?;
    let directory = if req.target.cli() {
        Some(working_directory(&req.directory)?)
    } else {
        None
    };
    command_for(&location, req.target, directory.as_deref())
}
#[tauri::command]
pub async fn validate_launch(req: LaunchRequest) -> Result<()> {
    tauri::async_runtime::spawn_blocking(move || prepare(&req).map(|_| ()))
        .await
        .map_err(|e| AppError::Other(e.to_string()))?
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchReceipt {
    pub message: String,
}
fn launch(req: LaunchRequest) -> Result<LaunchReceipt> {
    let _guard = LAUNCH_LOCK
        .try_lock()
        .map_err(|_| AppError::Other("启动中，请稍候".into()))?;
    let mut cmd = prepare(&req)?;
    // explorer.exe exits with 1 even after it has activated a packaged (MSIX/Store) app,
    // and reports real activation failures in its own dialog, so its exit code means nothing.
    #[cfg(windows)]
    let exit_code_meaningful = cmd.get_program() != system_exe("explorer.exe").as_os_str();
    #[cfg(not(windows))]
    let exit_code_meaningful = true;
    // The Windows CLI bootstrap opens an interactive child and exits after dispatch.
    if !req.target.cli() {
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::Other(format!("启动失败：{e}")))?;
    #[cfg(windows)]
    if req.target.cli() {
        let start = std::time::Instant::now();
        while child.try_wait()?.is_none() {
            if start.elapsed() > std::time::Duration::from_secs(15) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AppError::Other(
                    "终端启动请求超时，请检查是否已有窗口打开后再重试".into(),
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    #[cfg(not(windows))]
    std::thread::sleep(std::time::Duration::from_millis(250));
    #[cfg(windows)]
    if !req.target.cli() {
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    if let Some(exit) = child.try_wait()? {
        if exit_code_meaningful && !exit.success() {
            return Err(AppError::Other(format!(
                "启动程序已退出（{exit}），请检查程序路径或在终端中查看错误"
            )));
        }
    } else {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
    Ok(LaunchReceipt {
        message: if req.target.cli() {
            "已发出终端启动请求，请在终端中确认 CLI 状态"
        } else {
            "已发出桌面应用启动请求，请在应用内确认登录状态"
        }
        .into(),
    })
}
#[tauri::command]
pub async fn launch_client(req: LaunchRequest) -> Result<LaunchReceipt> {
    tauri::async_runtime::spawn_blocking(move || launch(req))
        .await
        .map_err(|e| AppError::Other(e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_missing_or_relative_directory() {
        assert!(working_directory(".").is_err());
        assert!(working_directory(
            &std::env::temp_dir()
                .join(atomic::unique_id())
                .to_string_lossy()
        )
        .is_err());
        assert!(working_directory(&std::env::temp_dir().to_string_lossy()).is_ok());
    }
    #[test]
    fn quotes_shell_metacharacters_as_data() {
        assert_eq!(
            ps_literal("C:\\work\\a'b & $(whoami);%PATH%"),
            "'C:\\work\\a''b & $(whoami);%PATH%'"
        );
        assert_eq!(sh_literal("a'b; $(id)"), "'a'\"'\"'b; $(id)'");
        assert_eq!(cli_script("C:\\工具\\cli.ps1", "C:\\work's dir"), "$ErrorActionPreference='Stop'; Set-Location -LiteralPath 'C:\\work''s dir'; & 'C:\\工具\\cli.ps1'");
    }
    #[test]
    fn invalid_custom_path_does_not_fall_back() {
        let mut prefs = Preferences::default();
        prefs
            .custom_paths
            .insert(Target::CodexCli, "missing.exe".into());
        assert!(discover(Target::CodexCli, &prefs, &BTreeMap::new()).is_err());
    }
    #[test]
    fn old_preferences_get_safe_defaults() {
        let p: Preferences = serde_json::from_str("{}").unwrap();
        assert_eq!(p.codex_service, "current");
        assert!(p.custom_paths.is_empty());
        assert!(serde_json::from_str::<Preferences>(r#"{"codexMode":"bad"}"#).is_err());
    }
    #[cfg(windows)]
    #[test]
    #[ignore = "opens a short-lived console to verify the native Windows adapter"]
    fn windows_console_smoke() {
        use std::time::{Duration, Instant};
        let root = std::env::temp_dir().join(format!("starnexus-launch-{}", atomic::unique_id()));
        let dir = root.join("中文 space's & $(noop)");
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("fake cli's & test.ps1");
        let marker = dir.join("result.json");
        std::fs::write(&script, "$result = @{cwd=$PWD.Path; inputRedirected=[Console]::IsInputRedirected; outputRedirected=[Console]::IsOutputRedirected}; [IO.File]::WriteAllText($env:STARNEXUS_LAUNCH_SMOKE_MARKER, ($result | ConvertTo-Json -Compress)); [Environment]::Exit(0)").unwrap();
        let mut cmd =
            command_for(&Location::Executable(script), Target::ClaudeCli, Some(&dir)).unwrap();
        cmd.env("STARNEXUS_LAUNCH_SMOKE_MARKER", &marker);
        let mut child = cmd.spawn().unwrap();
        let start = Instant::now();
        while !marker.exists() && start.elapsed() < Duration::from_secs(15) {
            std::thread::sleep(Duration::from_millis(100));
        }
        let content = std::fs::read_to_string(&marker);
        let _ = child.kill();
        let _ = child.wait();
        let value: serde_json::Value =
            serde_json::from_str(&content.expect("CLI fixture never executed")).unwrap();
        assert_eq!(value["cwd"].as_str(), Some(dir.to_string_lossy().as_ref()));
        assert_eq!(
            value["inputRedirected"], false,
            "CLI must receive interactive console input"
        );
        assert_eq!(
            value["outputRedirected"], false,
            "CLI output must be visible in its new console"
        );
        assert!(root.starts_with(std::env::temp_dir()));
        let cleanup_start = Instant::now();
        loop {
            match std::fs::remove_dir_all(&root) {
                Ok(()) => break,
                Err(_) if cleanup_start.elapsed() < Duration::from_secs(5) => {
                    std::thread::sleep(Duration::from_millis(100))
                }
                Err(e) => panic!("fixture cleanup failed: {e}"),
            }
        }
    }
    #[cfg(windows)]
    #[test]
    fn encoded_launch_command_roundtrips_unicode_and_quotes() {
        use base64::Engine;
        let path = PathBuf::from("C:\\工具's & folder\\codex.ps1");
        let cwd = PathBuf::from("C:\\项目 & $(echo nope)");
        let cmd = command_for(
            &Location::Executable(path.clone()),
            Target::CodexCli,
            Some(&cwd),
        )
        .unwrap();
        let args: Vec<_> = cmd
            .get_args()
            .map(|x| x.to_string_lossy().into_owned())
            .collect();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(args.last().unwrap())
            .unwrap();
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        let bootstrap = String::from_utf16(&units).unwrap();
        let inner_bytes: Vec<u8> = cli_script(&path.to_string_lossy(), &cwd.to_string_lossy())
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert!(bootstrap.contains(&base64::engine::general_purpose::STANDARD.encode(inner_bytes)));
        assert!(bootstrap.contains(&format!(
            "-WorkingDirectory {}",
            ps_literal(&cwd.to_string_lossy())
        )));
        assert_eq!(cmd.get_current_dir(), Some(cwd.as_path()));
    }
}
