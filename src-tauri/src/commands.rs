use crate::atomic::{read_text_optional, TwoFileTxn};
use crate::claude_config::{self, ClaudeSettings, SettingsState};
use crate::claude_desktop;
use crate::codex_auth::{self, AuthState};
use crate::codex_config::{self, AuthMode, ProviderConfig};
use crate::detect::{self, Environment};
use crate::error::{AppError, Result};
use crate::gateway::{self, SessionState};
use crate::{api, backup, health, paths, state};
use serde::{Deserialize, Serialize};
use std::fs;

pub(crate) async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tauri::async_runtime::spawn_blocking(work).await.map_err(|e| AppError::Other(e.to_string()))?
}

pub const PROVIDER_ID: &str = "starnexus_keytool";
pub const DISPLAY_NAME: &str = "AI";
// Keep PROVIDER_ID stable so existing managed configs remain reversible.

#[tauri::command]
pub async fn scan_environment(base_url: Option<String>, tools: Option<Vec<String>>, preferred_profile_id: Option<String>) -> Result<Environment> {
    blocking(move || scan_environment_blocking(base_url, tools, preferred_profile_id)).await
}

pub fn scan_environment_blocking(base_url: Option<String>, tools: Option<Vec<String>>, preferred_profile_id: Option<String>) -> Result<Environment> {
    let _guard = backup::lock()?;
    let openai_base = base_url.unwrap_or_else(|| api::builtin_defaults().base_url);
    // tools：用户同意读取的工具；不传就两个都读（老版本前端、内部调用）
    let wants = |t: &str| tools.as_ref().is_none_or(|list| list.iter().any(|x| x == t));
    detect::scan_targets(PROVIDER_ID, &openai_base, wants("claude") || wants("claude-cli"), wants("claude") || wants("claude-desktop"), wants("codex"), preferred_profile_id.as_deref())
}

#[tauri::command]
pub async fn load_app_state() -> Result<state::AppState> {
    blocking(load_app_state_blocking).await
}

pub fn load_app_state_blocking() -> Result<state::AppState> {
    state::load()
}

/// 界面切换语言：记下来（下次启动直接用），并同步窗口标题和托盘
#[tauri::command]
pub async fn set_ui_language(app: tauri::AppHandle, lang: String) -> Result<()> {
    blocking(move || set_ui_language_blocking(app, lang)).await
}

pub fn set_ui_language_blocking(app: tauri::AppHandle, lang: String) -> Result<()> {
    let parsed = crate::ui_lang::parse(&lang).ok_or_else(|| AppError::Other("不支持的语言".into()))?;
    crate::ui_lang::set(parsed);
    state::update(|s| s.ui_lang = lang.clone())?;
    crate::apply_native_language(&app);
    Ok(())
}

/// 记下用户已同意读取本机配置：之后启动时直接读取，不再显示首次读取的遮罩
#[tauri::command]
pub async fn mark_config_read(tool: String) -> Result<()> {
    blocking(move || mark_config_read_blocking(tool)).await
}

pub fn mark_config_read_blocking(tool: String) -> Result<()> {
    if tool != "claude" && tool != "codex" {
        return Err(AppError::Other("请先选择工具".into()));
    }
    state::update(|s| {
        if !s.config_read_tools.contains(&tool) {
            s.config_read_tools.push(tool);
        }
    })
}

/// 密钥列表。只返回元数据和掩码，完整密钥不出后端。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileView {
    pub id: String,
    pub name: String,
    pub masked_key: String,
    pub has_key: bool,
    pub codex_model: String,
    pub codex_effort: String,
    pub claude_model: String,
    pub gateway_token_id: Option<i64>,
    /// 其他服务（不是 AI）的地址；都空就是 AI 密钥
    pub codex_base_url: String,
    pub claude_base_url: String,
    pub custom: bool,
}

#[tauri::command]
pub async fn list_profiles() -> Result<Vec<ProfileView>> {
    blocking(list_profiles_blocking).await
}

pub fn list_profiles_blocking() -> Result<Vec<ProfileView>> {
    state::load()?
        .profiles
        .into_iter()
        .map(|p| {
            let key = state::load_profile_key(&p.id)?;
            let custom = p.is_custom();
            Ok(ProfileView {
                masked_key: key.as_deref().map(state::mask).unwrap_or_default(),
                has_key: key.is_some(),
                custom,
                id: p.id,
                name: p.name,
                codex_model: p.codex_model,
                codex_effort: p.codex_effort,
                claude_model: p.claude_model,
                gateway_token_id: p.gateway_token_id,
                codex_base_url: p.codex_base_url,
                claude_base_url: p.claude_base_url,
            })
        })
        .collect()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveProfileRequest {
    /// 留空表示新建
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// 留空表示不改动已存的密钥
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub codex_model: String,
    #[serde(default)]
    pub codex_effort: String,
    #[serde(default)]
    pub claude_model: String,
    /// 缺省保留原地址；显式空字符串表示切回 AI。
    #[serde(default)]
    pub codex_base_url: Option<String>,
    #[serde(default)]
    pub claude_base_url: Option<String>,
}

/// 其他服务的地址：只认 http(s)，去掉首尾空格和末尾斜杠
fn clean_base_url(raw: &str, tool: &str) -> Result<String> {
    let url = raw.trim().trim_end_matches('/');
    if url.is_empty() {
        return Ok(String::new());
    }
    let ok = reqwest::Url::parse(url)
        .is_ok_and(|u| matches!(u.scheme(), "https" | "http") && u.host_str().is_some_and(|h| !h.is_empty()));
    if !ok {
        return Err(AppError::Other(format!("{} 地址不正确，请填 https:// 开头的完整地址", tool)));
    }
    Ok(url.to_string())
}

#[tauri::command]
pub async fn save_profile(req: SaveProfileRequest) -> Result<String> {
    blocking(move || save_profile_blocking(req)).await
}

pub fn save_profile_blocking(req: SaveProfileRequest) -> Result<String> {
    crate::updates::ensure_supported()?;
    let is_new = req.id.trim().is_empty();
    if is_new && req.api_key.trim().is_empty() {
        return Err(AppError::Other("请先填上 API Key".into()));
    }
    let old = state::find_profile(&req.id)?.unwrap_or_default();
    let (codex_base_url, claude_base_url) = profile_urls(&req, &old)?;
    // 手动填了新 Key 就断开与网关令牌的关联 —— 它已经不是那把了
    let manual_key = !req.api_key.trim().is_empty();
    let keep_link = if manual_key {
        None
    } else {
        state::load()?
            .profiles
            .iter()
            .find(|x| x.id == req.id)
            .and_then(|x| x.gateway_token_id)
    };

    let p = state::Profile {
        id: req.id.clone(),
        name: req.name,
        codex_model: req.codex_model,
        codex_effort: req.codex_effort,
        claude_model: req.claude_model,
        gateway_token_id: keep_link,
        gateway_account: if manual_key {
            None
        } else {
            state::load()?
                .profiles
                .iter()
                .find(|p| p.id == req.id)
                .and_then(|p| p.gateway_account.clone())
        },
        codex_base_url,
        claude_base_url,
    };
    let key = if req.api_key.trim().is_empty() {
        None
    } else {
        Some(req.api_key.as_str())
    };
    Ok(state::upsert_profile(p, key)?.id)
}

fn profile_urls(req: &SaveProfileRequest, old: &state::Profile) -> Result<(String, String)> {
    Ok((
        clean_base_url(req.codex_base_url.as_deref().unwrap_or(&old.codex_base_url), "Codex")?,
        clean_base_url(req.claude_base_url.as_deref().unwrap_or(&old.claude_base_url), "Claude Code")?,
    ))
}

#[cfg(test)]
mod review_tests {
    use super::*;

    #[test]
    fn blocking_work_does_not_stall_async_executor() {
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        runtime.block_on(async {
            let worker = tokio::spawn(blocking(move || {
                started_tx.send(()).unwrap();
                release_rx.recv_timeout(std::time::Duration::from_secs(5))
                    .map_err(|_| AppError::Other("async executor was blocked".into()))?;
                Ok(())
            }));
            started_rx.await.unwrap();
            assert!(!worker.is_finished(), "executor must progress while blocking work is pending");
            release_tx.send(()).unwrap();
            worker.await.unwrap().unwrap();
        });
    }

    #[test]
    fn model_only_update_keeps_custom_endpoints_but_explicit_empty_clears_them() {
        let old = state::Profile { codex_base_url: "https://relay.example/v1".into(), claude_base_url: "https://relay.example".into(), ..Default::default() };
        let req: SaveProfileRequest = serde_json::from_str(r#"{"id":"custom","name":"relay","codexModel":"new-model"}"#).unwrap();
        assert_eq!(profile_urls(&req, &old).unwrap(), (old.codex_base_url.clone(), old.claude_base_url.clone()));
        let req: SaveProfileRequest = serde_json::from_str(r#"{"name":"relay","codexBaseUrl":"","claudeBaseUrl":""}"#).unwrap();
        assert_eq!(profile_urls(&req, &old).unwrap(), (String::new(), String::new()));
        let req: SaveProfileRequest = serde_json::from_str(r#"{"name":"relay","codexBaseUrl":"https://new.example/v1/"}"#).unwrap();
        assert_eq!(profile_urls(&req, &old).unwrap(), ("https://new.example/v1".into(), old.claude_base_url));
    }

}

#[tauri::command]
pub async fn delete_profile(id: String) -> Result<()> {
    blocking(move || delete_profile_blocking(id)).await
}

pub fn delete_profile_blocking(id: String) -> Result<()> {
    crate::updates::ensure_supported()?;
    state::delete_profile(&id)
}

#[tauri::command]
pub async fn get_remote_config(ignore_proxy: bool) -> Result<api::RemoteConfig> {
    Ok(crate::updates::get_client_policy(ignore_proxy).await?.config)
}

#[tauri::command]
pub async fn test_connection(
    base_url: String,
    api_key: String,
    ignore_proxy: bool,
) -> Result<api::TestResult> {
    crate::updates::ensure_supported()?;
    api::list_models(&base_url, &api_key, ignore_proxy).await
}

// Account sessions remain in memory and connect only to the fixed StarNexus site.
#[tauri::command]
pub async fn gateway_login(sessions: tauri::State<'_, SessionState>, username: String, password: String, ignore_proxy: bool) -> Result<gateway::LoginResult> {
    crate::updates::ensure_supported()?;
    sessions.reset();
    let s = sessions.get_or_init(ignore_proxy)?;
    gateway::login(&s, &username, &password).await
}

#[tauri::command]
pub async fn gateway_login_2fa(sessions: tauri::State<'_, SessionState>, code: String) -> Result<gateway::LoginResult> {
    crate::updates::ensure_supported()?;
    let s = sessions.get_or_init(false)?;
    gateway::login_2fa(&s, &code).await
}

#[tauri::command]
pub fn gateway_current_user(sessions: tauri::State<'_, SessionState>) -> Option<String> {
    sessions
        .get_or_init(false)
        .ok()
        .and_then(|s| s.current_user())
}

#[tauri::command]
pub fn gateway_logout(sessions: tauri::State<'_, SessionState>) {
    if let Ok(s) = sessions.get_or_init(false) {
        gateway::logout(&s);
    }
    sessions.reset();
}

/// 给界面看的一把 Key：在网关的信息 + 本地是否已经关联过
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenView {
    pub id: i64,
    pub name: String,
    pub usable: bool,
    pub note: String,
    /// 已经导入过就是本地那条的名字，界面据此显示「更新」而不是勾选
    pub linked_profile: Option<String>,
}

/// 登录后，本地密钥在控制台里的状态。只覆盖从当前账号导入过的密钥；手动粘贴的不在其中。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyStatus {
    pub profile_id: String,
    pub usable: bool,
    /// 控制台里已经找不到这个令牌（被删除）
    pub missing: bool,
    pub note: String,
}

#[tauri::command]
pub async fn gateway_key_status(
    sessions: tauri::State<'_, SessionState>,
) -> Result<Vec<KeyStatus>> {
    crate::updates::ensure_supported()?;
    let s = sessions.get_or_init(false)?;
    let list = gateway::list_tokens(&s).await?;
    let account = s.account_scope()?;
    Ok(state::load()?
        .profiles
        .into_iter()
        .filter(|p| p.gateway_account.as_deref() == Some(account.as_str()))
        .filter_map(|p| {
            let id = p.gateway_token_id?;
            let t = list.iter().find(|t| t.id == id);
            Some(KeyStatus {
                profile_id: p.id,
                usable: t.is_some_and(|t| t.usable),
                missing: t.is_none(),
                note: t.map(|t| t.note.clone()).unwrap_or_default(),
            })
        })
        .collect())
}

#[tauri::command]
pub async fn gateway_list_tokens(
    sessions: tauri::State<'_, SessionState>,
) -> Result<Vec<TokenView>> {
    crate::updates::ensure_supported()?;
    let s = sessions.get_or_init(false)?;
    let list = gateway::list_tokens(&s).await?;
    let account = s.account_scope()?;
    list.into_iter()
        .map(|t| {
            Ok(TokenView {
                linked_profile: state::find_by_token_id(t.id, &account)?.map(|p| p.name),
                id: t.id,
                name: t.name,
                usable: t.usable,
                note: t.note,
            })
        })
        .collect()
}

/// 取回某把 Key 的明文，直接存进指定的密钥库条目。
/// 明文只在后端停留，不返回给前端。
#[tauri::command]
pub async fn gateway_adopt_token(
    sessions: tauri::State<'_, SessionState>,
    profile_id: String,
    token_id: i64,
) -> Result<String> {
    crate::updates::ensure_supported()?;
    let s = sessions.get_or_init(false)?;
    let key = gateway::fetch_key(&s, token_id).await?;
    if profile_id.trim().is_empty() {
        return Err(AppError::Other("请先选择要存到哪一条".into()));
    }
    let mut p = state::load()?
        .profiles
        .into_iter()
        .find(|p| p.id == profile_id)
        .ok_or_else(|| AppError::Other("找不到这把密钥，请刷新后重试".into()))?;
    p.gateway_token_id = Some(token_id);
    p.gateway_account = Some(s.account_scope()?);
    state::upsert_profile(p, Some(&key))?;
    Ok(state::mask(&key))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub profile_ids: Vec<String>,
    pub created: Vec<String>,
    /// 已关联过，值换成了最新的
    pub updated: Vec<String>,
    /// 已关联且密钥没变，什么都没做
    pub unchanged: Vec<String>,
    pub skipped: Vec<String>,
}

/// 一次登录批量导入多把 Key：每把存成一条，名字沿用 Key 的名字。
/// 明文只在后端停留，直接进系统密钥库。
#[tauri::command]
pub async fn gateway_import_tokens(
    sessions: tauri::State<'_, SessionState>,
    token_ids: Vec<i64>,
) -> Result<ImportResult> {
    crate::updates::ensure_supported()?;
    if token_ids.is_empty() {
        return Err(AppError::Other("请先勾选要导入的 Key".into()));
    }
    let s = sessions.get_or_init(false)?;
    let all = gateway::list_tokens(&s).await?;
    let account = s.account_scope()?;

    let mut out = ImportResult {
        profile_ids: Vec::new(),
        created: Vec::new(),
        updated: Vec::new(),
        unchanged: Vec::new(),
        skipped: Vec::new(),
    };

    for id in token_ids {
        let Some(t) = all.iter().find(|t| t.id == id) else {
            continue;
        };
        if !t.usable {
            out.skipped.push(t.name.clone());
            continue;
        }
        let key = match gateway::fetch_key(&s, id).await {
            Ok(k) => k,
            Err(_) => {
                out.skipped.push(t.name.clone());
                continue;
            }
        };

        // 这把 Key 之前导过：就地换成最新的值，不重复新建。
        // 服务端重置 Key 后，用户点一次「更新」就恢复可用。
        if let Some(mut existing) = state::find_by_token_id(id, &account)? {
            let unchanged = state::load_profile_key(&existing.id)?.as_deref() == Some(key.as_str());
            existing.gateway_token_id = Some(id);
            existing.gateway_account = Some(account.clone());
            match state::upsert_profile(existing.clone(), Some(&key)) {
                Ok(saved) => {
                    out.profile_ids.push(saved.id);
                    if unchanged {
                        out.unchanged.push(existing.name);
                    } else {
                        out.updated.push(existing.name);
                    }
                }
                Err(_) => out.skipped.push(t.name.clone()),
            }
            continue;
        }

        let st = state::load()?;
        // 名字撞了就在后面加个序号，别让用户看到两个同名条目
        let mut name = t.name.clone();
        let mut n = 2;
        while st.profiles.iter().any(|p| p.name == name) {
            name = format!("{} {}", t.name, n);
            n += 1;
        }

        let profile = state::Profile {
            id: String::new(),
            name: name.clone(),
            codex_model: String::new(),
            codex_effort: String::new(),
            claude_model: String::new(),
            gateway_token_id: Some(id),
            gateway_account: Some(account.clone()),
            ..Default::default()
        };
        match state::upsert_profile(profile, Some(&key)) {
            Ok(saved) => {
                out.profile_ids.push(saved.id);
                out.created.push(name);
            }
            Err(_) => out.skipped.push(t.name.clone()),
        }
    }

    Ok(out)
}

/// 这把密钥能用的模型，以及每个模型支持的接口（读网关目录；目录读不到时退回 /models，接口未知）。
/// 只返回文本模型：图像、视频、语音模型对 Codex / Claude Code 没用。
/// 密钥从系统密钥库取，不经过前端。
#[tauri::command]
pub async fn list_profile_models(
    profile_id: String,
    base_url: String,
    ignore_proxy: bool,
) -> Result<Vec<api::ModelInfo>> {
    crate::updates::ensure_supported()?;
    let key = state::load_profile_key(&profile_id)?
        .ok_or_else(|| AppError::Other("这一条还没填 API Key".into()))?;
    // 其他服务：没有 AI 的模型目录，读它的 /models（优先 Codex 地址；只填了 Claude 地址就试 <地址>/v1）
    if let Some(p) = state::find_profile(&profile_id)?.filter(|p| p.is_custom()) {
        let base = p
            .openai_base(&base_url)
            .or_else(|| p.anthropic_base(&base_url).map(|u| format!("{}/v1", u.trim_end_matches("/v1"))))
            .unwrap_or_default();
        let r = api::list_models(&base, &key, ignore_proxy).await?;
        if !r.ok {
            return Err(AppError::Other(r.message));
        }
        return Ok(r.models.into_iter().map(|id| api::ModelInfo { id, endpoints: Vec::new() }).collect());
    }
    if let Some(list) = api::list_catalog(&base_url, &key, ignore_proxy).await? {
        let text = ["responses", "messages", "chat.completions"];
        let models: Vec<_> = list
            .into_iter()
            .filter(|m| m.endpoints.is_empty() || m.endpoints.iter().any(|e| text.contains(&e.as_str())))
            .collect();
        if !models.is_empty() {
            return Ok(models);
        }
    }
    let r = api::list_models(&base_url, &key, ignore_proxy).await?;
    if !r.ok {
        return Err(AppError::Other(r.message));
    }
    Ok(r.models.into_iter().map(|id| api::ModelInfo { id, endpoints: Vec::new() }).collect())
}

#[tauri::command]
pub async fn list_backups() -> Result<Vec<backup::BackupEntry>> {
    blocking(list_backups_blocking).await
}

pub fn list_backups_blocking() -> Result<Vec<backup::BackupEntry>> {
    backup::list()
}

#[tauri::command]
pub async fn restore_backup(id: String) -> Result<()> {
    blocking(move || restore_backup_blocking(id)).await
}

pub fn restore_backup_blocking(id: String) -> Result<()> {
    backup::restore(&id)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyRequest {
    /// 要启用哪一把
    pub profile_id: String,
    /// 带 /v1 的网关地址，Codex 用。Claude Code 的地址由它推导。
    pub base_url: String,
    pub wire_api: String,
    pub ignore_proxy: bool,

    // —— Codex ——
    pub configure_codex: bool,
    #[serde(default)]
    pub codex_desktop: bool,
    pub model: String,
    pub reasoning_effort: String,
    pub cleanup_residue: bool,
    /// 用户明确勾选了「同时写入 auth.json（会覆盖官方登录）」
    pub overwrite_official_auth: bool,
    pub conservative: bool,

    // —— Claude Code ——
    pub configure_claude: bool,
    pub claude_model: String,
    pub claude_opus_model: String,
    pub claude_sonnet_model: String,
    pub claude_haiku_model: String,

    // —— Claude 桌面版（第三方推理配置，和 CLI 的 settings.json 是两份）——
    #[serde(default)]
    pub configure_claude_desktop: bool,
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ApplyResult {
    pub backup_id: String,
    pub wrote_codex_config: bool,
    pub wrote_codex_auth: bool,
    pub wrote_claude_settings: bool,
    pub wrote_claude_desktop: bool,
    /// 写了 Claude 桌面版，但用户在桌面版里选的是官方登录：配置不会生效，前端弹窗说明
    pub official_login_chosen: bool,
    pub paths: Vec<String>,
    pub warnings: Vec<String>,
}

/// 一键配置。整个工具的主流程。
///
/// Codex 和 Claude Code 在同一个事务里写 —— 任何一个失败就全部还原，
/// 不留"一半配好一半没配"的中间态。
#[tauri::command]
pub async fn apply_config(req: ApplyRequest) -> Result<ApplyResult> {
    blocking(move || apply_config_blocking(req)).await
}

pub fn apply_config_blocking(req: ApplyRequest) -> Result<ApplyResult> {
    crate::updates::ensure_supported()?;
    let cli_version = (req.configure_codex && !req.conservative && !req.codex_desktop)
        .then(|| crate::launcher::cli_version(crate::launcher::Target::CodexCli).ok().map(|(version, _)| version)).flatten();
    let _guard = backup::lock()?;
    crate::updates::ensure_supported()?;
    let mut out = ApplyResult::default();

    let api_key = state::load_profile_key(&req.profile_id)?
        .ok_or_else(|| AppError::Other("这一条还没填 API Key".into()))?;
    if !req.configure_codex && !req.configure_claude && !req.configure_claude_desktop {
        return Err(AppError::Other("请先选择要配置哪个工具".into()));
    }

    // 地址跟着密钥走：AI 密钥用网关；其他服务用它自己填的地址
    let profile = state::find_profile(&req.profile_id)?.unwrap_or_default();
    let openai_base = if req.configure_codex {
        profile
            .openai_base(&req.base_url)
            .ok_or_else(|| AppError::Other("这把密钥没有填 Codex 地址，不能给 Codex 用".into()))?
    } else {
        String::new()
    };
    let anthropic_base = if req.configure_claude || req.configure_claude_desktop {
        profile
            .anthropic_base(&req.base_url)
            .ok_or_else(|| AppError::Other("这把密钥没有填 Claude Code 地址，不能给 Claude Code 用".into()))?
    } else {
        String::new()
    };
    let display_name = if profile.is_custom() { profile.name.clone() } else { DISPLAY_NAME.to_string() };

    // ——— 先把所有校验做完，再动任何一个文件 ———

    let codex_cfg_path = paths::config_path()?;
    let codex_auth_path = paths::auth_path()?;
    let claude_path = paths::claude_settings_path()?;

    let existing_codex = if req.configure_codex {
        let snapshot = read_text_optional(&codex_cfg_path)?;
        let t = snapshot.as_deref().unwrap_or_default();
        // 硬规则：解析不了就绝不写入。宁可让用户手动修，也不能覆盖他的东西。
        if !t.trim().is_empty() {
            if let Err(e) = codex_config::parse(t) {
                return Err(AppError::TomlParse(format!(
                    "{}。你的原文件没有被改动，可以自己修好它，或者用「备份原文件，重新生成」",
                    e
                )));
            }
        }
        Some(snapshot)
    } else {
        None
    };

    let existing_claude = if req.configure_claude {
        let t = read_text_optional(&claude_path)?;
        if let SettingsState::Broken(e) = claude_config::inspect(t.as_deref()) {
            return Err(AppError::JsonParse(format!(
                "Claude Code 的配置文件有格式问题：{}。你的原文件没有被改动，请先修好它",
                e
            )));
        }
        Some(t)
    } else {
        None
    };

    let existing_desktop = if req.configure_claude_desktop {
        let meta = read_text_optional(&claude_desktop::meta_path()?)?;
        let (path, new_meta) = claude_desktop::target_for_write(meta.as_deref())?;
        let t = read_text_optional(&path)?;
        if let SettingsState::Broken(e) = claude_desktop::inspect(t.as_deref()) {
            return Err(AppError::JsonParse(format!(
                "Claude 桌面版的配置文件有格式问题：{}。原文件没有被改动，请先修好它",
                e
            )));
        }
        Some((path, meta, new_meta, t))
    } else {
        None
    };

    // 目录不存在就建，但要告诉用户
    if req.configure_codex {
        let home = paths::codex_home()?;
        if !home.is_dir() {
            fs::create_dir_all(&home).map_err(|e| {
                AppError::CodexHomeMissing(format!("{}（创建失败：{}）", home.display(), e))
            })?;
            out.warnings.push(format!(
                "没找到 Codex 的设置文件夹，已经帮你建好：{}",
                home.display()
            ));
        }
    }
    if req.configure_claude {
        let home = paths::claude_home()?;
        if !home.is_dir() {
            fs::create_dir_all(&home)
                .map_err(|e| AppError::Other(format!("创建 {} 失败：{}", home.display(), e)))?;
            out.warnings.push(format!(
                "没找到 Claude Code 的设置文件夹，已经帮你建好：{}",
                home.display()
            ));
        }
    }

    // Stage every change first; back up the exact write scope before commit.
    let mut txn = TwoFileTxn::new();

    // ——— Codex ———
    if let Some(snapshot) = existing_codex {
        let existing = snapshot.as_deref().unwrap_or_default();
        let auth_text = read_text_optional(&codex_auth_path)?;
        let auth_state = codex_auth::inspect(auth_text.as_deref());
        let journal = paths::ownership_path()?;
        let previous = crate::atomic::read_optional(&journal)?;
        let (mut ownership, journal_corrupt) = crate::codex_ownership::Ownership::from_bytes(previous.as_deref());
        if journal_corrupt {
            out.warnings.push(JOURNAL_CORRUPT.into());
        }
        ownership.observe_route(existing, auth_text.as_deref());

        let external_store = codex_uses_external_auth_store(existing);
        let keep_official = external_store
            || (auth_state == AuthState::OfficialLogin && !req.overwrite_official_auth);
        let foreign_key = auth_state == AuthState::ApiKeyOnly && !ownership.owns_auth(auth_text.as_deref())
            && !(codex_config::has_builtin_relay(existing) && codex_config::current_pointing(existing).as_deref() == Some(openai_base.as_str())
                && codex_auth::current_api_key(auth_text.as_deref()).as_deref() == Some(api_key.as_str()));
        let auth_mode = codex_config::select_auth_mode(cli_version.as_deref(), req.conservative || req.codex_desktop,
            foreign_key || keep_official || auth_state == AuthState::Unparsable);

        let provider = ProviderConfig {
            provider_id: PROVIDER_ID.into(),
            display_name: display_name.clone(),
            base_url: openai_base.clone(),
            wire_api: req.wire_api.clone(),
            model: req.model.clone(),
            reasoning_effort: req.reasoning_effort.clone(),
            api_key: api_key.clone(),
            auth_mode,
        };

        if external_store {
            out.warnings
                .push("认证由系统密钥库或其他存储管理；已保留原认证，仅配置独立的网关凭据".into());
        }
        let new_cfg = codex_config::apply(existing, &provider, req.cleanup_residue)?;
        txn.stage_text(&codex_cfg_path, snapshot.as_deref(), Some(&new_cfg));
        out.wrote_codex_config = true;
        out.paths.push(codex_cfg_path.to_string_lossy().to_string());

        match auth_state {
            AuthState::OfficialLogin if auth_mode == AuthMode::BearerToken => {
                out.warnings
                    .push("已经帮你保住了 ChatGPT 官方登录，以后随时能切回去".into());
            }
            AuthState::OfficialLogin => {
                out.warnings
                    .push("按你的选择，已经退出了 ChatGPT 官方登录".into());
            }
            AuthState::Unparsable => {
                out.warnings
                    .push("登录信息文件有格式问题，没有改动它，密钥已改放在配置文件里".into());
            }
            _ => {}
        }

        if auth_mode == AuthMode::OpenAiBuiltin {
            ownership.record_auth(&api_key);
            let merged = if req.overwrite_official_auth {
                codex_auth::replace_with_api_key(auth_text.as_deref(), &api_key)?
            } else {
                codex_auth::merge_api_key(auth_text.as_deref(), &api_key)?
            };
            txn.stage_text(&codex_auth_path, auth_text.as_deref(), Some(&merged));
            out.wrote_codex_auth = true;
            out.paths
                .push(codex_auth_path.to_string_lossy().to_string());
        }
        ownership.record(previous.is_none() || journal_corrupt, existing, &new_cfg);
        txn.stage(&journal, previous, Some(serde_json::to_vec(&ownership)?));
    }

    // ——— Claude Code ———
    if let Some(existing) = existing_claude {
        let settings = ClaudeSettings {
            base_url: anthropic_base.clone(),
            auth_token: api_key.clone(),
            model: req.claude_model.clone(),
            opus_model: req.claude_opus_model.clone(),
            sonnet_model: req.claude_sonnet_model.clone(),
            haiku_model: req.claude_haiku_model.clone(),
        };
        let merged = claude_config::merge(existing.as_deref(), &settings)?;
        txn.stage_text(&claude_path, existing.as_deref(), Some(&merged));
        out.wrote_claude_settings = true;
        out.paths.push(claude_path.to_string_lossy().to_string());
    }

    // ——— Claude 桌面版 ———
    // 写入后桌面版会把明文密钥加密；记下这次写入的条目，之后才能认出那段密文
    let mut desktop_written: Option<(String, claude_desktop::Current)> = None;
    if let Some((path, meta_before, new_meta, existing)) = existing_desktop {
        let models = claude_desktop::model_list(&[
            &req.claude_model,
            &req.claude_opus_model,
            &req.claude_sonnet_model,
            &req.claude_haiku_model,
        ]);
        let merged = claude_desktop::merge(existing.as_deref(), &anthropic_base, &api_key, &models)?;
        desktop_written = claude_desktop::current(&merged).map(|current| (path.to_string_lossy().to_string(), current));
        if existing.as_deref() != Some(merged.as_str()) {
            txn.stage_text(&path, existing.as_deref(), Some(&merged));
            if let Some(meta) = new_meta {
                txn.stage_text(&claude_desktop::meta_path()?, meta_before.as_deref(), Some(&meta));
            } else {
                txn.guard_text(&claude_desktop::meta_path()?, meta_before.as_deref());
            }
            out.wrote_claude_desktop = true;
            out.paths.push(path.to_string_lossy().to_string());
            // 重启提示和官方登录提示由前端弹窗处理（写入后查询运行中的桌面版）
            out.official_login_chosen = claude_desktop::official_login_chosen();
        }
    }

    if !txn.paths().is_empty() {
        out.backup_id = backup::create_for_paths(&txn.paths())?;
        txn.commit()?;
    }
    if let Some((config, current)) = desktop_written {
        let entry = state::DesktopEntry { path: &config, base_url: current.base_url.as_deref(), models: &current.models };
        if let Err(e) = state::remember_desktop_key(&req.profile_id, &api_key, &entry) {
            out.warnings.push(format!("配置已写入，但记录桌面版密钥失败：{}", e));
        }
    }

    // 模型偏好跟着密钥走，全局只记这几项
    if let Err(e) = state::update(|st| {
        st.ignore_proxy = req.ignore_proxy;
        st.cleanup_residue = req.cleanup_residue;
        st.last_configured_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        if let Some(prof) = st.profiles.iter_mut().find(|x| x.id == req.profile_id) {
            if req.configure_codex {
                prof.codex_model = req.model.clone();
                prof.codex_effort = req.reasoning_effort.clone();
            }
            if req.configure_claude || req.configure_claude_desktop {
                prof.claude_model = req.claude_model.clone();
            }
        }
    }) {
        out.warnings
            .push(format!("配置已写入，但偏好保存失败：{}", e));
    }

    Ok(out)
}

/// 把两个工具都切回官方，并清掉我们写进去的密钥
#[tauri::command]
pub async fn revert_to_official(codex: bool, claude: bool, claude_desktop: Option<bool>) -> Result<String> {
    blocking(move || revert_to_official_blocking(codex, claude, claude_desktop)).await
}

pub fn revert_to_official_blocking(codex: bool, claude: bool, claude_desktop: Option<bool>) -> Result<String> {
    let claude_desktop = claude_desktop.unwrap_or(false);
    let _guard = backup::lock()?;
    if !codex && !claude && !claude_desktop {
        return Err(AppError::Other("请先选择工具".into()));
    }
    let mut txn = TwoFileTxn::new();

    if codex {
        let cfg_path = paths::config_path()?;
        if let Some(existing) = read_text_optional(&cfg_path)? {
            codex_config::parse(&existing)?;
            let new_cfg = codex_config::revert_to_official(&existing, PROVIDER_ID)?;
            let journal = paths::ownership_path()?;
            let previous = crate::atomic::read_optional(&journal)?;
            // 记录损坏时按「没有记录」处理：不恢复字段、不认领任何密钥，只会更保守
            let (ownership, journal_corrupt) = crate::codex_ownership::Ownership::from_bytes(previous.as_deref());
            let restored = if previous.is_some() && !journal_corrupt { ownership.restore_fields(&new_cfg)? } else { new_cfg };
            txn.stage_text(&cfg_path, Some(&existing), Some(&restored));

            // 只有在没有官方登录的情况下才动 auth.json
            let auth_path = paths::auth_path()?;
            let auth_text = read_text_optional(&auth_path)?;
            if codex_auth::inspect(auth_text.as_deref()) == AuthState::ApiKeyOnly {
                let relay_key = codex_config::has_builtin_relay(&existing) || ownership.is_legacy_relay_key(auth_text.as_deref());
                // 旧版本写入、没有归属记录的网关密钥：与本工具里保存的某把密钥相同才认作本工具写入
                let saved_relay_key = relay_key && match codex_auth::current_api_key(auth_text.as_deref()) {
                    Some(key) => state::profile_with_key(&key)?.is_some(),
                    None => false,
                };
                if ownership.owns_auth(auth_text.as_deref()) || saved_relay_key {
                    let stripped = codex_auth::strip_api_key(auth_text.as_deref())?;
                    txn.stage_text(&auth_path, auth_text.as_deref(), stripped.as_deref().filter(|text| text.trim() != "{}"));
                } else if relay_key {
                    return Err(AppError::Other("auth.json 里的 API Key 不是本工具保存的任何一把密钥，无法确认归属。为避免删错密钥或将网关密钥发往官方，请先备份并在 Codex 中移除第三方凭据，再切回官方。".into()));
                }
            }
            if previous.is_some() { txn.stage(&journal, previous, None); }
        }
    }

    if claude {
        let claude_path = paths::claude_settings_path()?;
        let text = read_text_optional(&claude_path)?;
        if let SettingsState::Broken(e) = claude_config::inspect(text.as_deref()) {
            return Err(AppError::JsonParse(e));
        }
        if let SettingsState::Ok = claude_config::inspect(text.as_deref()) {
            if let Some(reverted) = claude_config::revert(text.as_deref())? {
                txn.stage_text(&claude_path, text.as_deref(), Some(&reverted));
            }
        }
    }

    if claude_desktop {
        let meta = read_text_optional(&claude_desktop::meta_path()?)?;
        if let claude_desktop::Applied::Config(path) = claude_desktop::parse_meta(meta.as_deref()) {
            txn.guard_text(&claude_desktop::meta_path()?, meta.as_deref());
            let text = read_text_optional(&path)?;
            if let Some(reverted) = claude_desktop::revert(text.as_deref())? {
                txn.stage_text(&path, text.as_deref(), Some(&reverted));
            }
        }
    }

    if txn.paths().is_empty() {
        return Ok(String::new());
    }
    let backup_id = backup::create_for_paths(&txn.paths())?;
    txn.commit()?;
    if claude_desktop {
        let _ = state::forget_desktop_key();
    }
    Ok(backup_id)
}

/// config.toml 坏到没法解析时的最后出路：备份后重建一份最小可用配置
#[tauri::command]
pub async fn rebuild_minimal_config(req: ApplyRequest) -> Result<ApplyResult> {
    blocking(move || rebuild_minimal_config_blocking(req)).await
}

pub fn rebuild_minimal_config_blocking(req: ApplyRequest) -> Result<ApplyResult> {
    crate::updates::ensure_supported()?;
    if req.configure_claude && !req.configure_codex && !req.configure_claude_desktop {
        return repair_claude_env_blocking(req);
    }
    if !req.configure_codex || req.configure_claude || req.configure_claude_desktop {
        return Err(AppError::Other("重建配置仅支持 Codex 和 Claude Code".into()));
    }
    let _guard = backup::lock()?;
    crate::updates::ensure_supported()?;
    let api_key = state::load_profile_key(&req.profile_id)?
        .ok_or_else(|| AppError::Other("这一条还没填 API Key".into()))?;

    let mut out = ApplyResult {
        warnings: vec!["已经重新生成了一份干净的配置，原文件在备份里".into()],
        ..Default::default()
    };

    let cfg_path = paths::config_path()?;
    // A broken config cannot establish the original credential-store policy.
    let original = read_text_optional(&cfg_path)?;
    if original.as_deref().is_none_or(|text| codex_config::parse(text).is_ok()) {
        return Err(AppError::Other("当前配置未损坏，请刷新后使用常规写入。".into()));
    }
    // Rebuild with an independent provider and keep every existing login untouched.
    let auth_mode = AuthMode::BearerToken;
    let profile = state::find_profile(&req.profile_id)?.unwrap_or_default();
    let base_url = profile
        .openai_base(&req.base_url)
        .ok_or_else(|| AppError::Other("这把密钥没有填 Codex 地址，不能给 Codex 用".into()))?;
    let provider = ProviderConfig {
        provider_id: PROVIDER_ID.into(),
        display_name: if profile.is_custom() { profile.name.clone() } else { DISPLAY_NAME.into() },
        base_url,
        wire_api: req.wire_api,
        model: req.model,
        reasoning_effort: req.reasoning_effort,
        api_key: api_key.clone(),
        auth_mode,
    };

    // 从空文档生成；提交前会先备份原文件。
    let new_cfg = codex_config::apply("", &provider, false)?;

    let mut txn = TwoFileTxn::new();
    txn.stage_text(&cfg_path, original.as_deref(), Some(&new_cfg));
    let journal = paths::ownership_path()?;
    let previous = crate::atomic::read_optional(&journal)?;
    let (mut ownership, journal_corrupt) = crate::codex_ownership::Ownership::from_bytes(previous.as_deref());
    if journal_corrupt {
        out.warnings.push(JOURNAL_CORRUPT.into());
    }
    ownership.record(true, "", &new_cfg);
    txn.stage(&journal, previous, Some(serde_json::to_vec(&ownership)?));
    out.wrote_codex_config = true;
    out.paths.push(cfg_path.to_string_lossy().to_string());

    out.backup_id = backup::create_for_paths(&txn.paths())?;
    txn.commit()?;
    Ok(out)
}

const JOURNAL_CORRUPT: &str = "Codex 归属记录已损坏，已按无记录处理并重新建立；切回官方时不会自动删除无法确认归属的密钥";

/// Claude Code 的 settings.json 里 `env` 不是对象时的显式修复：
/// 备份后去掉损坏的 env，再按常规合并写入所选密钥；其余设置原样保留。
fn repair_claude_env_blocking(req: ApplyRequest) -> Result<ApplyResult> {
    let _guard = backup::lock()?;
    crate::updates::ensure_supported()?;
    let api_key = state::load_profile_key(&req.profile_id)?
        .ok_or_else(|| AppError::Other("这一条还没填 API Key".into()))?;
    let profile = state::find_profile(&req.profile_id)?.unwrap_or_default();
    let anthropic_base = profile
        .anthropic_base(&req.base_url)
        .ok_or_else(|| AppError::Other("这把密钥没有填 Claude Code 地址，不能给 Claude Code 用".into()))?;
    let path = paths::claude_settings_path()?;
    let original = read_text_optional(&path)?;
    let repaired = claude_config::without_malformed_env(original.as_deref())?;
    let settings = ClaudeSettings {
        base_url: anthropic_base,
        auth_token: api_key,
        model: req.claude_model,
        opus_model: req.claude_opus_model,
        sonnet_model: req.claude_sonnet_model,
        haiku_model: req.claude_haiku_model,
    };
    let merged = claude_config::merge(Some(&repaired), &settings)?;
    let mut txn = TwoFileTxn::new();
    txn.stage_text(&path, original.as_deref(), Some(&merged));
    let mut out = ApplyResult {
        warnings: vec!["原来的 env 不是对象，已替换为只含本工具设置的 env；原文件在备份里，其余设置未改动".into()],
        wrote_claude_settings: true,
        paths: vec![path.to_string_lossy().to_string()],
        ..Default::default()
    };
    out.backup_id = backup::create_for_paths(&txn.paths())?;
    txn.commit()?;
    Ok(out)
}

fn codex_uses_external_auth_store(config: &str) -> bool {
    codex_config::parse(config)
        .ok()
        .and_then(|doc| {
            doc.get("cli_auth_credentials_store")
                .and_then(|v| v.as_str())
                .map(|s| s != "file")
        })
        .unwrap_or(false)
}

// ——————————— 体检 ———————————

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthRequest {
    pub tool: health::Tool,
    /// "current" | "official" | 某把密钥的 id（与启动卡片上的选择一致）
    pub service: String,
    /// 这把密钥给该工具配的模型，用来检查是否在可用列表里
    #[serde(default)]
    pub model: String,
    /// 带 /v1 的网关地址
    pub base_url: String,
    pub ignore_proxy: bool,
    /// 选了 AI 密钥时，启动会写入的三个 Claude 别名（来自高级设置）
    #[serde(default)]
    pub opus_model: String,
    #[serde(default)]
    pub sonnet_model: String,
    #[serde(default)]
    pub haiku_model: String,
    /// "desktop" | "cli"。Claude 桌面版和 CLI 读的是两份配置，体检分开查；Codex 两者共用一份
    #[serde(default)]
    pub mode: String,
}

impl HealthRequest {
    fn claude_desktop(&self) -> bool {
        self.tool == health::Tool::Claude && self.mode == "desktop"
    }
}

/// 连接测试成功的结果按「密钥 + 地址 + 代理设置」缓存，App 关掉就清空。
/// 失败不缓存：用户修好密钥或网络后，下次刷新会立刻重测。
#[derive(Default)]
pub struct HealthCache(std::sync::Mutex<std::collections::HashMap<u64, Vec<String>>>);

/// 用某把密钥测一次连接。成功的结果按「密钥 + 地址 + 代理设置」缓存。
async fn connection_for(
    key: &str,
    openai_base: &str,
    ignore_proxy: bool,
    cache: &HealthCache,
) -> health::Connection {
    use health::Connection;
    use std::hash::{Hash, Hasher};
    let fingerprint = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (key, openai_base, ignore_proxy).hash(&mut h);
        h.finish()
    };
    if let Some(models) = cache.0.lock().ok().and_then(|c| c.get(&fingerprint).cloned()) {
        return Connection::Ok(models);
    }
    match api::list_models(openai_base, key, ignore_proxy).await {
        Err(_) => Connection::Network,
        Ok(r) if !r.ok => Connection::Http(r.status),
        Ok(r) => {
            if let Ok(mut c) = cache.0.lock() {
                c.insert(fingerprint, r.models.clone());
            }
            Connection::Ok(r.models)
        }
    }
}

/// 网关目录（每个模型支持哪些接口），按「密钥 + 地址 + 代理」缓存。读不到时给空列表，体检就不做接口核对。
async fn catalog_for(key: &str, openai_base: &str, ignore_proxy: bool, cache: &HealthCache) -> Vec<api::ModelInfo> {
    use std::hash::{Hash, Hasher};
    let fingerprint = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        ("catalog", key, openai_base, ignore_proxy).hash(&mut h);
        h.finish()
    };
    // 缓存里每行存「模型\t接口,接口」
    if let Some(rows) = cache.0.lock().ok().and_then(|c| c.get(&fingerprint).cloned()) {
        return rows
            .iter()
            .filter_map(|r| r.split_once('\t'))
            .map(|(id, eps)| api::ModelInfo {
                id: id.to_string(),
                endpoints: eps.split(',').filter(|e| !e.is_empty()).map(String::from).collect(),
            })
            .collect();
    }
    let Ok(Some(list)) = api::list_catalog(openai_base, key, ignore_proxy).await else {
        return Vec::new();
    };
    if let Ok(mut c) = cache.0.lock() {
        c.insert(fingerprint, list.iter().map(|m| format!("{}\t{}", m.id, m.endpoints.join(","))).collect());
    }
    list
}

fn service_kind(service: &str) -> health::Service {
    match service {
        "current" => health::Service::Current,
        "official" => health::Service::Official,
        _ => health::Service::Ai,
    }
}

/// 体检的全部规则。用于启动前检查和体检卡片，不调用模型。
pub async fn run_health(req: &HealthRequest, cache: &HealthCache) -> Result<Vec<health::Issue>> {
    crate::updates::ensure_supported()?;
    use health::{Service, Tool};
    let openai_base = req.base_url.trim_end_matches('/').to_string();
    let anthropic_base = claude_config::derive_anthropic_base_url(&openai_base);
    let service = service_kind(&req.service);

    let local_req = req.clone();
    let local_base = openai_base.clone();
    let (mut issues, current, profile, selected_key) = blocking(move || {
        let _guard = backup::lock()?;
        let req = &local_req;
        let openai_base = local_base;
        let result = match req.tool {
            Tool::Claude if req.claude_desktop() => match claude_desktop::applied()? {
                claude_desktop::Applied::Config(path) => {
                    let text = read_text_optional(&path)?;
                    let restart_pending = service != Service::Official
                        && text
                            .as_deref()
                            .and_then(claude_desktop::current)
                            .and_then(|c| c.api_key)
                            .is_some_and(|k| !claude_desktop::is_sealed(&k))
                        && claude_desktop::running();
                    (
                        health::check_claude_desktop(
                            text.as_deref(),
                            &anthropic_base,
                            service,
                            restart_pending,
                            claude_desktop::official_login_chosen(),
                        ),
                        health::current_claude_desktop(text.as_deref(), &anthropic_base, |sealed, current| {
                            let config = path.to_string_lossy();
                            let entry = state::DesktopEntry { path: &config, base_url: current.base_url.as_deref(), models: &current.models };
                            state::match_sealed_desktop_key(sealed, &entry)
                                .ok()
                                .flatten()
                                .and_then(|id| state::load_profile_key(&id).ok().flatten())
                        }),
                    )
                }
                claude_desktop::Applied::None => (Vec::new(), None),
                claude_desktop::Applied::Unusable(reason) => (
                    vec![health::Issue {
                        id: "D5",
                        level: health::Level::Warn,
                        text: reason,
                        fix: health::Fix::None,
                    }],
                    None,
                ),
            },
            Tool::Claude => {
                let text = read_text_optional(&paths::claude_settings_path()?)?;
                (
                    health::check_claude(text.as_deref(), &anthropic_base, service),
                    health::current_claude(text.as_deref(), &anthropic_base),
                )
            }
            Tool::Codex => {
                let text = read_text_optional(&paths::config_path()?)?;
                let auth = read_text_optional(&paths::auth_path()?)?;
                (
                    health::check_codex(text.as_deref(), &openai_base, service),
                    health::current_codex(text.as_deref(), auth.as_deref(), &openai_base, PROVIDER_ID),
                )
            }
        };
        let profile = if service == Service::Ai { state::find_profile(&req.service)? } else { None };
        let key = if service == Service::Ai { state::load_profile_key(&req.service)? } else { None };
        Ok((result.0, result.1, profile, key))
    }).await?;

    if service == Service::Ai {
        let selection_issues = health::check_selected_profile(req.tool, profile.as_ref(), selected_key.as_deref());
        if !selection_issues.is_empty() {
            issues.extend(selection_issues);
            return Ok(issues);
        }
    }

    // 测哪把密钥、对照哪些模型：选了 AI 密钥就是它和启动时会写入的模型；
    // 沿用当前配置就是配置文件里实际在用的那把（只在指向网关时）。密钥都不经过前端。
    // 选的是其他服务的密钥：配置规则照常查（别名等 AI 专属的规则只在指向网关时才报），
    // 连接只测 Codex 地址的 /models；Claude 那边各家接口不一，不替它测
    let custom = profile.filter(|p| p.is_custom());
    if let Some(p) = custom {
        if req.tool == Tool::Codex {
            if let (Some(base), Some(key)) = (p.openai_base(&openai_base), selected_key) {
                let conn = connection_for(&key, &base, req.ignore_proxy, cache).await;
                issues.extend(health::check_connection(&conn, &req.model));
            }
        }
        if !req.claude_desktop() {
            issues.extend(health::check_env(req.tool, env_is_set));
        }
        return Ok(issues);
    }

    let target = match service {
        Service::Ai => selected_key.map(|key| health::CurrentSetup {
            key,
            model: req.model.clone(),
            aliases: vec![
                ("opus", req.opus_model.clone()),
                ("sonnet", req.sonnet_model.clone()),
                ("haiku", req.haiku_model.clone()),
            ],
        }),
        Service::Current => current,
        Service::Official => None,
    };
    if let Some(t) = target {
        let conn = connection_for(&t.key, &openai_base, req.ignore_proxy, cache).await;
        let before = issues.len();
        issues.extend(health::check_connection(&conn, &t.model));
        // 模型在列表里、连接正常时，再按网关目录核对这个工具用的接口上有没有它（不计费，结果缓存）
        if issues.len() == before && !t.model.trim().is_empty() && matches!(conn, health::Connection::Ok(_)) {
            let catalog = catalog_for(&t.key, &openai_base, req.ignore_proxy, cache).await;
            issues.extend(health::check_endpoint(req.tool, &t.model, &catalog));
        }
        if req.tool == Tool::Claude {
            let aliases: Vec<(&str, &str)> = t.aliases.iter().map(|(l, m)| (*l, m.as_str())).collect();
            issues.extend(health::check_aliases(&conn, &aliases));
        }
    }

    // ANTHROPIC_* 等环境变量只影响 CLI；桌面版按自己的配置走
    if !req.claude_desktop() {
        issues.extend(health::check_env(req.tool, env_is_set));
    }
    Ok(issues)
}

fn env_is_set(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| !v.trim().is_empty())
}

#[tauri::command]
pub async fn health_check(
    req: HealthRequest,
    cache: tauri::State<'_, HealthCache>,
) -> Result<Vec<health::Issue>> {
    run_health(&req, &cache).await
}

/// 会覆盖配置文件的环境变量里，哪些已经设置了。只返回变量名，不返回值。
#[tauri::command]
pub fn env_overrides() -> Vec<String> {
    health::CLAUDE_ENV_VARS
        .iter()
        .chain(health::CODEX_ENV_VARS)
        .filter(|name| env_is_set(name))
        .map(|name| name.to_string())
        .collect()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptRequest {
    pub tool: health::Tool,
    pub base_url: String,
    pub ignore_proxy: bool,
    /// Claude 桌面版：读它自己的第三方推理配置，而不是 settings.json
    #[serde(default)]
    pub desktop: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptResult {
    pub id: String,
    pub name: String,
    /// 这把密钥本来就在列表里，没有新建
    pub existed: bool,
}

/// 读出配置文件里当前生效的地址和密钥（AI 网关或第三方都可以）
fn live_credential(tool: health::Tool, desktop: bool) -> Result<Option<detect::LiveCredential>> {
    use health::Tool;
    Ok(match (tool, desktop) {
        (Tool::Codex, _) => {
            let text = read_text_optional(&paths::config_path()?)?;
            let auth = read_text_optional(&paths::auth_path()?)?;
            detect::live_codex(text.as_deref(), auth.as_deref())
        }
        (Tool::Claude, false) => detect::live_claude(read_text_optional(&paths::claude_settings_path()?)?.as_deref()),
        (Tool::Claude, true) => match claude_desktop::applied()? {
            claude_desktop::Applied::Config(path) => read_text_optional(&path)?.as_deref().and_then(detect::live_claude_desktop),
            _ => None,
        },
    })
}

/// 收录时怎么存：指向 AI 网关的存成 AI 密钥；其他地址存成「其他服务」密钥，地址记在对应工具上
fn adopted_profile(tool: health::Tool, desktop: bool, live: &detect::LiveCredential, gateway_openai: &str) -> (state::Profile, bool) {
    use health::Tool;
    let codex = matches!(tool, Tool::Codex);
    let gateway = if codex { gateway_openai.to_string() } else { claude_config::derive_anthropic_base_url(gateway_openai) };
    let is_gateway = health::host(&live.base_url).is_some() && health::host(&live.base_url) == health::host(&gateway);
    let base = live.base_url.trim().trim_end_matches('/').to_string();
    let name = if is_gateway {
        match (tool, desktop) {
            (Tool::Codex, _) => crate::ui_lang::pick("Codex 配置里的密钥", "Codex 設定裡的密鑰", "Key from Codex config", "Ключ из конфигурации Codex"),
            (Tool::Claude, false) => crate::ui_lang::pick("Claude 配置里的密钥", "Claude 設定裡的密鑰", "Key from Claude config", "Ключ из конфигурации Claude"),
            (Tool::Claude, true) => crate::ui_lang::pick("Claude 桌面版配置里的密钥", "Claude 桌面版設定裡的密鑰", "Key from Claude desktop config", "Ключ из конфигурации Claude Desktop"),
        }
        .to_string()
    } else {
        health::host(&base).unwrap_or_else(|| base.clone())
    };
    let profile = state::Profile {
        id: String::new(),
        name,
        codex_model: if codex { live.model.clone() } else { String::new() },
        codex_effort: String::new(),
        claude_model: if codex { String::new() } else { live.model.clone() },
        gateway_token_id: None,
        gateway_account: None,
        codex_base_url: if !is_gateway && codex { base.clone() } else { String::new() },
        claude_base_url: if !is_gateway && !codex { base } else { String::new() },
    };
    (profile, is_gateway)
}

/// 把配置文件里正在用、但没保存在本工具的密钥收进密钥列表：AI 网关的，或第三方服务的。
/// 密钥只在后端读出、测试、存进系统凭据库，不经过前端；测试明确失败（密钥无效、连不上）就不收录。
#[tauri::command]
pub async fn adopt_current_key(req: AdoptRequest) -> Result<AdoptResult> {
    crate::updates::ensure_supported()?;
    let gateway_openai = req.base_url.trim_end_matches('/').to_string();
    let live = live_credential(req.tool, req.desktop)?
        .ok_or_else(|| AppError::Other("当前配置里没有可以加入的密钥".into()))?;

    for p in state::load()?.profiles {
        if state::load_profile_key(&p.id)?.as_deref() == Some(live.key.as_str()) {
            return Ok(AdoptResult { id: p.id, name: p.name, existed: true });
        }
    }

    let (profile, is_gateway) = adopted_profile(req.tool, req.desktop, &live, &gateway_openai);
    // 测试用的 OpenAI 兼容地址：AI 用网关；第三方 Codex 用它自己的地址，Claude 的 Anthropic 地址补上 /v1
    let test_base = if is_gateway {
        gateway_openai.clone()
    } else if profile.codex_base_url.is_empty() {
        format!("{}/v1", profile.claude_base_url.trim_end_matches("/v1"))
    } else {
        profile.codex_base_url.clone()
    };
    let test = api::list_models(&test_base, &live.key, req.ignore_proxy)
        .await
        .map_err(|_| AppError::Network("连不上服务器，没有收录，稍后再试".into()))?;
    if !test.ok {
        if matches!(test.status, 401 | 403) {
            return Err(AppError::Other("配置里的这把密钥无效或已停用，没有收录".into()));
        }
        // 第三方服务不一定提供 /models：客户端正在用它，只要不是明确的认证失败就收录
        if is_gateway {
            return Err(AppError::Other(format!("服务暂时不可用（{}），没有收录，稍后再试", test.status)));
        }
    }

    let saved = state::upsert_profile(profile, Some(&live.key))?;
    Ok(AdoptResult { id: saved.id, name: saved.name, existed: false })
}
#[cfg(test)]
mod adopt_tests {
    use super::*;
    const GATEWAY: &str = "https://gateway.dkby.com/v1";

    fn live(base: &str) -> detect::LiveCredential {
        detect::LiveCredential { base_url: base.into(), key: "k".into(), model: "m".into() }
    }

    #[test]
    fn gateway_keys_are_saved_as_zai_profiles() {
        let (p, gateway) = adopted_profile(health::Tool::Codex, false, &live("https://gateway.dkby.com/v1/"), GATEWAY);
        assert!(gateway);
        assert!(!p.is_custom());
        assert_eq!(p.codex_model, "m");
        let (p, gateway) = adopted_profile(health::Tool::Claude, true, &live(&claude_config::derive_anthropic_base_url(GATEWAY)), GATEWAY);
        assert!(gateway);
        assert!(!p.is_custom());
        assert_eq!(p.claude_model, "m");
    }

    #[test]
    fn third_party_keys_keep_their_endpoint_on_the_matching_tool() {
        let (p, gateway) = adopted_profile(health::Tool::Codex, false, &live("https://relay.example/v1/"), GATEWAY);
        assert!(!gateway);
        assert_eq!((p.name.as_str(), p.codex_base_url.as_str(), p.claude_base_url.as_str()), ("relay.example", "https://relay.example/v1", ""));
        let (p, gateway) = adopted_profile(health::Tool::Claude, false, &live("https://relay.example"), GATEWAY);
        assert!(!gateway);
        assert_eq!((p.codex_base_url.as_str(), p.claude_base_url.as_str(), p.claude_model.as_str()), ("", "https://relay.example", "m"));
    }
}
