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
pub async fn load_launcher_preferences() -> Result<Preferences> {
    crate::commands::blocking(load_launcher_preferences_blocking).await
}

fn load_launcher_preferences_blocking() -> Result<Preferences> {
    load_preferences()
}

#[tauri::command]
pub async fn save_launcher_preferences(app: tauri::AppHandle, preferences: Preferences) -> Result<()> {
    crate::commands::blocking(move || save_launcher_preferences_blocking(preferences)).await?;
    crate::tray_menu::refresh(&app);
    Ok(())
}

#[tauri::command]
pub async fn validate_project_directory(path: String) -> Result<()> {
    crate::commands::blocking(move || working_directory(&path).map(|_| ())).await
}

fn save_launcher_preferences_blocking(mut preferences: Preferences) -> Result<()> {
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

/// Use the verified private installation without changing the user's shell or global PATH.
pub(crate) fn use_installed_client(target: Target, path: &Path) -> Result<()> {
    let _guard = PREFS_LOCK.lock().map_err(|_| AppError::Other("启动偏好锁不可用".into()))?;
    let mut preferences = load_preferences()?;
    preferences.custom_paths.insert(target, path.to_string_lossy().into());
    atomic::write_atomic(&preferences_path()?, &serde_json::to_string_pretty(&preferences)?)
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
            home.join(".volta/bin"),
            home.join(".asdf/shims"),
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

// Match registered package identities as well as legacy product names. Display names
// can change (OpenAI.Codex is labelled ChatGPT on current Windows installations).
#[cfg(any(windows, test))]
fn parse_packaged_apps(text: &str) -> BTreeMap<String, String> {
    text.lines().filter_map(|line| {
        let (name, id) = line.trim().split_once('|')?;
        let (family, application) = id.split_once('!')?;
        if family.is_empty() || application.is_empty()
            || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-!".contains(&b))
            || application.contains('!') {
            return None;
        }
        let product = if family.starts_with("OpenAI.Codex_") && application == "App" {
            "codex"
        } else if family.starts_with("Claude_") && application == "Claude" {
            "claude"
        } else if name.eq_ignore_ascii_case("Codex") {
            "codex"
        } else if name.eq_ignore_ascii_case("Claude") {
            "claude"
        } else {
            return None;
        };
        Some((product.into(), id.into()))
    }).collect()
}

#[cfg(windows)]
fn packaged_apps() -> Result<BTreeMap<String, String>> {
    let script = "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[System.Text.UTF8Encoding]::new(); Get-StartApps | Where-Object { $_.Name -in @('Claude','Codex') -or $_.AppID -match '^(OpenAI[.]Codex_|Claude_)' } | ForEach-Object { $_.Name + '|' + $_.AppID }";
    let output = crate::process::output(Command::new(powershell())
        .args(["-NoProfile", "-NonInteractive", "-Command", script]), std::time::Duration::from_secs(15))?;
    if !output.status.success() { return Err(AppError::Other("Windows 应用检测失败，请重试；这不代表程序未安装。".into())); }
    Ok(parse_packaged_apps(&String::from_utf8_lossy(&output.stdout)))
}

#[cfg(target_os = "macos")]
fn validate_bundle(target: Target, path: &Path) -> Result<()> {
    let expected = if target == Target::ClaudeDesktop { "com.anthropic.claudefordesktop" } else { "com.openai.codex" };
    let output = crate::process::output(Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print :CFBundleIdentifier"]).arg(path.join("Contents/Info.plist")), crate::process::HELPER_TIMEOUT)?;
    if !output.status.success() || String::from_utf8_lossy(&output.stdout).trim() != expected {
        return Err(AppError::Other("所选应用与客户端不匹配，请选择对应的 Claude 或 Codex 应用。".into()));
    }
    Ok(())
}

#[cfg(any(windows, test))]
fn desktop_executable(local: &Path, target: Target) -> Option<PathBuf> {
    let candidates = if target == Target::ClaudeDesktop {
        vec![local.join("AnthropicClaude/claude.exe"), local.join("Programs/Claude/Claude.exe")]
    } else {
        // OpenAI/Codex/bin/<version>/codex.exe is the bundled CLI, not a desktop app.
        vec![local.join("Programs/Codex/Codex.exe"), local.join("OpenAI/Codex/Codex.exe")]
    };
    candidates.into_iter().find(|path| path.is_file())
}

fn custom_location(target: Target, path: &str) -> Result<Location> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(AppError::Other("请选择绝对路径".into()));
    }
    #[cfg(target_os = "macos")]
    if !target.cli() && path.is_dir() && path.extension().is_some_and(|x| x == "app") {
        validate_bundle(target, &path)?;
        return Ok(Location::Bundle(path));
    }
    #[cfg(target_os = "macos")]
    if !target.cli() { return Err(AppError::Other("桌面版请选择完整的 .app 应用程序包。".into())); }
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
            if let Some(path) = desktop_executable(&local, target) {
                return Ok(Some(Location::Executable(path)));
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        let names: &[&str] = if target == Target::ClaudeDesktop {
            &["Claude.app"]
        } else {
            &["ChatGPT.app", "Codex.app"]
        };
        let mut candidates: Vec<_> = names.iter().map(|name|PathBuf::from("/Applications").join(name)).collect();
        if let Some(home) = dirs::home_dir() {
            candidates.extend(names.iter().map(|name|home.join("Applications").join(name)));
        }
        if let Some(path) = candidates.into_iter().find(|p| {
            if !p.is_dir() { return false; }
            validate_bundle(target, p).is_ok()
        }) {
            return Ok(Some(Location::Bundle(path)));
        }
    }
    let _ = apps;
    Ok(None)
}
fn app_inventory() -> Result<BTreeMap<String, String>> {
    #[cfg(windows)]
    {
        packaged_apps()
    }
    #[cfg(not(windows))]
    {
        Ok(BTreeMap::new())
    }
}
pub fn scan() -> Result<Scan> {
    let prefs = load_preferences()?;
    let apps = app_inventory()?;
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

/// Explicit, bounded version probe; never run during the normal installation scan.
pub(crate) fn cli_version(target: Target) -> Result<(String, String)> {
    if !target.cli() { return Err(AppError::Other("版本检查仅支持 CLI".into())); }
    let prefs = load_preferences()?;
    let Some(Location::Executable(path)) = discover(target, &prefs, &BTreeMap::new())? else {
        return Err(AppError::Other("未找到 CLI，请先安装或设置程序路径".into()));
    };
    let mut cmd = version_command(&path);
    if let Some(home) = dirs::home_dir() { cmd.current_dir(home); }
    let output = crate::process::output(&mut cmd, std::time::Duration::from_secs(15))?;
    if !output.status.success() { return Err(AppError::Other("CLI 版本检查失败，请在终端运行 --version 检查".into())); }
    let text = String::from_utf8_lossy(&output.stdout);
    let version = crate::client_setup::target_version(target, &text)
        .ok_or_else(|| AppError::Other("无法确认 CLI 身份或版本，请选择对应产品的官方可执行程序。".into()))?;
    Ok((version, path.to_string_lossy().into()))
}

/// Returns None only for a confirmed missing client; unreadable versions remain errors.
pub(crate) fn installed_version(target: Target) -> Result<Option<(String, String)>> {
    let prefs = load_preferences()?;
    let apps = if target.cli() || prefs.custom_paths.contains_key(&target) { BTreeMap::new() } else { app_inventory()? };
    let Some(location) = discover(target, &prefs, &apps)? else { return Ok(None); };
    if target.cli() { return cli_version(target).map(Some); }
    #[cfg(windows)]
    let mut command = {
        let script = match &location {
            Location::AppId(id) => {
                let name = if target == Target::ClaudeDesktop { "Claude" } else { "OpenAI.Codex" };
                format!("$p=Get-AppxPackage -Name '{name}' -ErrorAction Stop | Where-Object {{ $_.PackageFamilyName -eq {} }} | Select-Object -First 1; if (-not $p) {{ throw 'Package not found' }}; $p.Version", ps_literal(id.split('!').next().unwrap_or_default()))
            },
            Location::Executable(path) => format!("(Get-Item -LiteralPath {}).VersionInfo.ProductVersion", ps_literal(&path.to_string_lossy())),
        };
        let mut command = Command::new(powershell());
        command.args(["-NoProfile", "-NonInteractive", "-Command", &format!("$ErrorActionPreference='Stop'; {script}")]); command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let Location::Bundle(path) = &location else { return Err(AppError::Other("无法读取桌面版版本，请选择应用程序包。".into())); };
        let mut command = Command::new("/usr/libexec/PlistBuddy");
        command.args(["-c", "Print :CFBundleShortVersionString"]).arg(path.join("Contents/Info.plist")); command
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    return Err(AppError::Other("此系统暂不支持桌面版版本检查。".into()));
    #[cfg(any(windows, target_os = "macos"))]
    {
        let output = crate::process::output(&mut command, std::time::Duration::from_secs(10))?;
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !output.status.success() || version.is_empty() { return Err(AppError::Other("无法读取本机版本，请重试。".into())); }
        let path = match location {
            Location::Executable(path) => path.to_string_lossy().into_owned(),
            #[cfg(windows)] Location::AppId(id) => id,
            #[cfg(target_os = "macos")] Location::Bundle(path) => path.to_string_lossy().into_owned(),
        };
        Ok(Some((version, path)))
    }
}

fn version_command(path: &Path) -> Command {
    #[cfg(windows)]
    {
        use base64::Engine;
        let script = format!("$ErrorActionPreference='Stop'; & {} --version; if ($null -ne $LASTEXITCODE) {{ exit $LASTEXITCODE }}", ps_literal(&path.to_string_lossy()));
        let mut command = Command::new(powershell());
        command.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand"])
            .arg(base64::engine::general_purpose::STANDARD.encode(script.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<_>>()));
        command
    }
    #[cfg(not(windows))]
    { let mut command = Command::new(path); command.arg("--version");
      if let Ok(value) = cli_path(path) { command.env("PATH", value); }
      command }
}

#[cfg(any(unix, test))]
fn cli_path(path: &Path) -> Result<std::ffi::OsString> {
    let mut dirs = Vec::new();
    if let Some(parent) = path.parent() { dirs.push(parent.to_path_buf()); }
    // npm symlinks can point into a Node version's own bin directory.
    if let Ok(resolved) = path.canonicalize() {
        if let Some(parent) = resolved.parent() { dirs.push(parent.to_path_buf()); }
        // npm's bin link points into <prefix>/lib/node_modules, while node lives in <prefix>/bin.
        for ancestor in resolved.ancestors() {
            if ancestor.file_name().is_some_and(|name| name == "lib") {
                if let Some(prefix) = ancestor.parent() {
                    let bin = prefix.join("bin");
                    if executable(&bin.join("node")) { dirs.push(bin); }
                }
            }
        }
    }
    dirs.extend(search_dirs());
    std::env::join_paths(dirs).map_err(|_| AppError::Other("程序搜索路径无效，请检查 PATH 设置。".into()))
}

#[cfg(any(target_os = "macos", test))]
fn mac_terminal_script(executable: &str, directory: &str, path: &str, config_name: &str, config_dir: &str) -> String {
    let shell = format!("cd -- {} && /usr/bin/env {} {} {}", sh_literal(directory),
        sh_literal(&format!("PATH={path}")), sh_literal(&format!("{config_name}={config_dir}")), sh_literal(executable));
    mac_script_dispatch(&shell)
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn mac_script_dispatch(shell: &str) -> String {
    use base64::Engine;
    // Only fixed ASCII syntax reaches the user's shell (including fish). Dynamic paths
    // are decoded inside bash; this avoids differing backslash/single-quote semantics.
    let encoded = base64::engine::general_purpose::STANDARD.encode(shell.as_bytes());
    let command = format!("/bin/bash --noprofile --norc -c 'exec /bin/bash --noprofile --norc -c \"$(/usr/bin/printf %s {encoded} | /usr/bin/base64 -D)\"'");
    let apple = command.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r");
    format!("tell application \"Terminal\" to do script \"{apple}\"")
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchRequest {
    pub target: Target,
    pub directory: String,
    /// 本次启动选用的服务：密钥编号、"current"（沿用当前配置）或 "official"。老前端不传。
    #[serde(default)]
    pub service: Option<String>,
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
            let Location::Executable(path) = location else { return Err(AppError::Other("CLI 程序路径无效".into())); };
            let (config_name, config_dir) = if target == Target::ClaudeCli { ("CLAUDE_CONFIG_DIR", paths::claude_home()?) } else { ("CODEX_HOME", paths::codex_home()?) };
            let apple = mac_terminal_script(&path.to_string_lossy(), &dir.to_string_lossy(), &cli_path(path)?.to_string_lossy(), config_name, &config_dir.to_string_lossy());
            let mut cmd = Command::new("/usr/bin/osascript");
            cmd.args([
                "-e",
                "tell application \"Terminal\" to activate",
                "-e",
                &apple,
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
    let apps = if req.target.cli() || prefs.custom_paths.contains_key(&req.target) {
        BTreeMap::new()
    } else {
        app_inventory()?
    };
    let location = discover(req.target, &prefs, &apps)?
        .ok_or_else(|| AppError::Other("未找到应用，请安装或在设置中指定程序路径".into()))?;
    let directory = if req.target.cli() {
        Some(working_directory(&req.directory)?)
    } else {
        None
    };
    let mut command = command_for(&location, req.target, directory.as_deref())?;
    // Relative overrides were resolved when writing configuration, before changing cwd.
    if req.target.cli() {
        if req.target == Target::CodexCli { command.env("CODEX_HOME", paths::codex_home()?); }
        else { command.env("CLAUDE_CONFIG_DIR", paths::claude_home()?); }
    }
    Ok(command)
}
#[tauri::command]
pub async fn validate_launch(req: LaunchRequest) -> Result<()> {
    tauri::async_runtime::spawn_blocking(move || {
        prepare(&req)?;
        // Catch missing Node, policy restrictions and incompatible binaries before writing keys.
        if req.target.cli() {
            let (version, _) = cli_version(req.target)?;
            if req.target == Target::CodexCli && codex_service_needs_rewrite(req.service.as_deref(), &version, || {
                Ok(atomic::read_text_optional(&paths::config_path()?)?.unwrap_or_default())
            })? {
                return Err(AppError::Other("此 Codex CLI 未验证支持当前网关写法；请在密钥页重新写入独立 provider 后再启动。".into()));
            }
        }
        Ok(())
    })
        .await
        .map_err(|e| AppError::Other(e.to_string()))?
}
/// 只拦「要启动本工具里的某把密钥，但现有配置是该版本未验证的内置网关写法」。
/// 沿用当前配置是用户自己的选择，切回官方会在写入阶段处理，两者都不在这里拦。
fn codex_service_needs_rewrite(service: Option<&str>, version: &str, config: impl FnOnce() -> Result<String>) -> Result<bool> {
    let keytool_service = service.is_some_and(|s| !s.is_empty() && s != "current" && s != "official");
    if !keytool_service || crate::codex_config::verified_builtin(Some(version)) {
        return Ok(false);
    }
    Ok(crate::codex_config::has_builtin_relay(&config()?))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchReceipt {
    pub message: String,
}
fn cli_receipt(req: &LaunchRequest) -> LaunchReceipt {
    let files: &[&str] = if req.target == Target::CodexCli { &[".codex/config.toml"] } else { &[".claude/settings.json", ".claude/settings.local.json"] };
    let home = dirs::home_dir();
    let project = Path::new(&req.directory).ancestors().any(|directory| {
        if Some(directory) == home.as_deref() { return false; }
        files.iter().any(|file| directory.join(file).is_file())
    });
    LaunchReceipt { message: if project { "已请求启动；检测到项目级设置，实际服务和模型请在终端确认。" } else { "已请求启动，请在终端确认。" }.into() }
}
pub(crate) fn launch(req: LaunchRequest) -> Result<LaunchReceipt> {
    crate::updates::ensure_supported()?;
    let _guard = LAUNCH_LOCK
        .try_lock()
        .map_err(|_| AppError::Other("启动中，请稍候".into()))?;
    let mut cmd = prepare(&req)?;
    // Wait for short-lived dispatchers so permission denial and missing terminal errors
    // cannot arrive after the UI has reported successful dispatch. Never wait on the app itself.
    #[cfg(any(windows, target_os = "macos"))]
    if req.target.cli() || cfg!(target_os = "macos") {
        let output = crate::process::output(&mut cmd, std::time::Duration::from_secs(30))?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr);
            return Err(AppError::Other(format!("启动请求失败，请检查终端、程序路径和系统自动化权限：{}", detail.trim())));
        }
        return Ok(if req.target.cli() { cli_receipt(&req) } else { LaunchReceipt { message: "已请求启动，请在应用内确认登录。".into() } });
    }
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
            "已请求启动，请在终端确认。"
        } else {
            "已请求启动，请在应用内确认登录。"
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
    fn codex_preflight_only_blocks_keytool_services_on_unverified_builtin_relay() {
        let relay = || Ok("openai_base_url='https://relay.invalid/v1'".to_string());
        let independent = || Ok("model_provider='dbkey'
[model_providers.dbkey]
base_url='https://relay.invalid/v1'".to_string());
        // 旧版本 + 内置网关写法 + 选用本工具的密钥：要求先重新写入
        assert!(codex_service_needs_rewrite(Some("profile-1"), "0.100.0", relay).unwrap());
        // 沿用当前配置、切回官方、老前端不传服务：不拦
        for service in [Some("current"), Some("official"), Some(""), None] {
            assert!(!codex_service_needs_rewrite(service, "0.100.0", relay).unwrap());
        }
        // 0.167.0 起（含中间版本和后续版本）放行；独立 provider 不受版本影响
        for version in ["0.167.0", "0.168.0", "0.169.0", "0.170.0", "1.0.0"] {
            assert!(!codex_service_needs_rewrite(Some("profile-1"), version, || panic!("不应读取配置")).unwrap());
        }
        assert!(!codex_service_needs_rewrite(Some("profile-1"), "0.100.0", independent).unwrap());
    }

    #[test]
    fn mac_dispatch_quotes_paths_and_pins_config_directory() {
        use base64::Engine;
        let script = mac_terminal_script("/Users/中文/tool's & $(id)/codex", "/work/a'b\nnext", "/opt/homebrew/bin:/usr/bin", "CODEX_HOME", "/Users/custom config");
        assert!(script.starts_with("tell application \"Terminal\" to do script \"/bin/bash --noprofile --norc -c "));
        assert!(!script.contains('\n'));
        let encoded = script.split("printf %s ").nth(1).unwrap().split(' ').next().unwrap();
        let payload = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(encoded).unwrap()).unwrap();
        assert!(payload.contains("CODEX_HOME=/Users/custom config"));
        assert!(payload.contains("PATH=/opt/homebrew/bin:/usr/bin"));
        assert!(payload.contains("'\"'\"'"));
        assert!(script.contains("\\\""));
    }
    #[test]
    fn mac_shell_payload_roundtrips_special_characters_in_real_bash() {
        #[cfg(windows)]
        let bash = PathBuf::from("C:/Program Files/Git/bin/bash.exe");
        #[cfg(not(windows))]
        let bash = PathBuf::from("/bin/bash");
        if !bash.is_file() { eprintln!("bash fixture unavailable; shell execution not tested"); return; }
        let value = "中文 space's \\backslash $HOME `whoami` ; & %PATH% !\nnext";
        let apple = mac_script_dispatch(&format!("printf %s {}", sh_literal(value)));
        let command = apple.strip_prefix("tell application \"Terminal\" to do script \"").unwrap().strip_suffix('"').unwrap()
            .replace("\\\"", "\"").replace("\\\\", "\\");
        // BSD base64 uses -D; the Windows Git Bash fixture uses GNU base64's -d.
        #[cfg(windows)]
        let command = command.replace("base64 -D", "base64 -d");
        let output = crate::process::output(Command::new(bash).args(["--noprofile", "--norc", "-c", &command]), std::time::Duration::from_secs(15)).unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(String::from_utf8(output.stdout).unwrap(), value);
    }
    #[test]
    fn cli_search_path_preserves_selected_binary_directory() {
        let fixture = atomic::tests::TestDir::new();
        let binary = fixture.0.join("cli");
        let value = cli_path(&binary).unwrap();
        assert_eq!(std::env::split_paths(&value).next().unwrap(), fixture.0);
    }
    #[test]
    #[cfg(windows)]
    fn version_probe_quotes_paths_and_only_passes_version_argument() {
        let dir = crate::atomic::tests::TestDir::new();
        let path = dir.0.join("version & 'quoted' $test.ps1");
        std::fs::write(&path, "if ($args.Count -ne 1 -or $args[0] -ne '--version') { exit 9 }; Write-Output 'codex-cli 0.100.0'").unwrap();
        // This checks quoting and arguments, not PowerShell cold-start latency.
        // Hosted Windows runners can take over five seconds to start the shell.
        // The process tests separately verify that deadlines terminate helpers.
        let output = crate::process::output(&mut version_command(&path), std::time::Duration::from_secs(30)).unwrap();
        assert!(output.status.success());
        assert_eq!(crate::client_setup::parse_version_output(&String::from_utf8_lossy(&output.stdout)), Some("0.100.0".into()));
    }
    #[test]
    #[cfg(windows)]
    fn cmd_and_bat_version_probes_preserve_argument_and_exit_status() {
        let dir = crate::atomic::tests::TestDir::new();
        for ext in ["cmd", "bat"] {
            let path = dir.0.join(format!("cli space & quoted'.{ext}"));
            std::fs::write(&path, "@echo off\r\nif not \"%~1\"==\"--version\" exit /b 9\r\necho codex-cli 0.169.0\r\nexit /b 0\r\n").unwrap();
            let output = crate::process::output(&mut version_command(&path), std::time::Duration::from_secs(30)).unwrap();
            assert!(output.status.success(), "{}",String::from_utf8_lossy(&output.stderr));
            assert!(String::from_utf8_lossy(&output.stdout).contains("0.169.0"));
            std::fs::write(&path,"@exit /b 7\r\n").unwrap();
            let output = crate::process::output(&mut version_command(&path), std::time::Duration::from_secs(30)).unwrap();
            assert_eq!(output.status.code(),Some(7));
        }
    }
    #[test]
    #[cfg(windows)]
    fn powershell_7_executes_the_same_version_adapter_when_available() {
        let available = crate::process::output(Command::new("pwsh").args(["-NoProfile", "-NonInteractive", "-Command", "$PSVersionTable.PSVersion.Major"]), std::time::Duration::from_secs(15));
        if !available.is_ok_and(|o| o.status.success()) { eprintln!("PowerShell 7 unavailable; optional engine test skipped"); return; }
        let fixture = atomic::tests::TestDir::new();
        for extension in ["ps1", "cmd", "bat"] {
            let path = fixture.0.join(format!("中文 space's & [brackets].{extension}"));
            let script = if extension == "ps1" { "if ($args[0] -ne '--version') { exit 9 }; Write-Output 'codex-cli 0.169.0'" }
                else { "@echo off\r\nif not \"%~1\"==\"--version\" exit /b 9\r\necho codex-cli 0.169.0\r\nexit /b 0\r\n" };
            std::fs::write(&path,script).unwrap();
            let reference = version_command(&path);
            let output = crate::process::output(Command::new("pwsh").args(reference.get_args()),std::time::Duration::from_secs(15)).unwrap();
            assert!(output.status.success(), "{}: {}", extension, String::from_utf8_lossy(&output.stderr));
            assert!(String::from_utf8_lossy(&output.stdout).contains("0.169.0"));
        }
    }
    #[test]
    #[cfg(unix)]
    fn npm_symlink_retains_its_node_prefix_on_unix() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let fixture = atomic::tests::TestDir::new();
        let bin = fixture.0.join("node-version/bin");
        let package = fixture.0.join("node-version/lib/node_modules/tool/bin");
        std::fs::create_dir_all(&bin).unwrap(); std::fs::create_dir_all(&package).unwrap();
        std::fs::write(bin.join("node"), "fixture").unwrap();
        std::fs::set_permissions(bin.join("node"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(package.join("cli.js"),"fixture").unwrap();
        let link = fixture.0.join("codex"); symlink(package.join("cli.js"),&link).unwrap();
        let paths: Vec<_> = std::env::split_paths(&cli_path(&link).unwrap()).collect();
        assert!(paths.contains(&bin.canonicalize().unwrap()));
    }
    #[test]
    fn desktop_identity_survives_display_name_changes() {
        let apps = parse_packaged_apps("ChatGPT|OpenAI.Codex_2p2nqsd0c76g0!App\nClaude|Claude_pzs8sxrjxfjjc!Claude\nOther|unrelated!App\nCodex|bad!App!extra\nCodex|bad/route!App");
        assert_eq!(apps.len(), 2);
        assert_eq!(apps["codex"], "OpenAI.Codex_2p2nqsd0c76g0!App");
        assert_eq!(apps["claude"], "Claude_pzs8sxrjxfjjc!Claude");
        assert_eq!(parse_packaged_apps("Codex|Legacy_123!App")["codex"], "Legacy_123!App");
    }
    #[test]
    fn bundled_codex_cli_is_never_a_desktop_candidate() {
        let fixture = atomic::tests::TestDir::new();
        let local = fixture.0.as_path();
        let cli = local.join("OpenAI/Codex/bin/version/codex.exe");
        std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
        std::fs::write(&cli, "CLI fixture").unwrap();
        assert!(desktop_executable(local, Target::CodexDesktop).is_none());
        let desktop = local.join("OpenAI/Codex/Codex.exe");
        std::fs::write(&desktop, "desktop fixture").unwrap();
        assert_eq!(desktop_executable(local, Target::CodexDesktop), Some(desktop));
    }
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
        let dispatched = crate::process::output(&mut cmd, Duration::from_secs(30)).unwrap();
        assert!(dispatched.status.success());
        let start = Instant::now();
        while !marker.exists() && start.elapsed() < Duration::from_secs(15) {
            std::thread::sleep(Duration::from_millis(100));
        }
        let content = std::fs::read_to_string(&marker);
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
            .as_chunks::<2>().0.iter()
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
