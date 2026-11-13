use crate::atomic::read_text_optional;
use crate::claude_config::{self, SettingsState};
use crate::claude_desktop;
use crate::codex_auth::{self, AuthState};
use crate::codex_config;
use crate::error::Result;
use crate::paths;
use serde::Serialize;

/// config.toml 的健康状况。
/// 解析不了的时候绝不能写入 —— 宁可让用户手动修，也不能覆盖掉他的东西。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigState {
    Missing,
    Ok,
    /// 存在但 TOML 语法有误，附带错误信息
    Broken(String),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexEnv {
    /// Codex 目录是否存在
    pub home_exists: bool,
    pub home: String,
    pub config_state: ConfigState,
    pub auth_state: AuthState,
    /// 当前 config.toml 指向的 base_url
    pub current_base_url: Option<String>,
    pub current_model: Option<String>,
    /// 用户可能手改过推理强度，界面要回填而不是用默认值盖掉
    pub current_reasoning_effort: Option<String>,
    /// 旧脚本留下的内伤清单
    pub residue: Vec<String>,
    /// 是否已经由本工具配置过
    pub managed_by_us: bool,
    /// 当前生效的是哪一把（能和已保存的某把 Key 对上时才有值）
    pub active_profile_id: Option<String>,
    /// 配置指向本网关、带着一把 Key，但这把 Key 没保存在本工具：可以一键收进密钥列表
    pub unsaved_gateway_key: bool,
    pub configuration_status: ConfigurationStatus,
    pub status_message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeEnv {
    pub home_exists: bool,
    pub home: String,
    pub settings_state: SettingsState,
    pub current_base_url: Option<String>,
    pub current_model: Option<String>,
    pub managed_by_us: bool,
    pub active_profile_id: Option<String>,
    /// 同 CodexEnv::unsaved_gateway_key
    pub unsaved_gateway_key: bool,
    pub configuration_status: ConfigurationStatus,
    pub status_message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    pub codex: CodexEnv,
    /// Claude Code CLI（~/.claude/settings.json）
    pub claude: ClaudeEnv,
    /// Claude 桌面版的第三方推理配置（它不读 settings.json）
    pub claude_desktop: ClaudeEnv,
}

pub fn scan(provider_id: &str, openai_base_url: &str) -> Result<Environment> {
    scan_selected(provider_id, openai_base_url, true, true)
}

/// 只读用户同意过的那个工具的配置；另一个返回「未读取」占位，不碰它的文件。
pub fn scan_selected(provider_id: &str, openai_base_url: &str, claude: bool, codex: bool) -> Result<Environment> {
    // 指向 AI 网关或任何一把其他服务密钥的地址，都要按密钥核对是不是本工具写的
    let (custom_codex, custom_claude) = crate::state::custom_base_urls().unwrap_or_default();
    let mut codex_urls = vec![openai_base_url.trim_end_matches('/').to_string()];
    codex_urls.extend(custom_codex);
    let mut claude_urls = vec![claude_config::derive_anthropic_base_url(openai_base_url)];
    claude_urls.extend(custom_claude);
    Ok(Environment {
        codex: if codex { scan_codex(provider_id, &codex_urls)? } else { codex_not_read()? },
        claude_desktop: if claude { scan_claude_desktop(&claude_urls)? } else { claude_desktop_not_read()? },
        claude: if claude { scan_claude(&claude_urls)? } else { claude_not_read()? },
    })
}

const NOT_READ: &str = "尚未读取（用户还没同意读取这个工具的配置）";

fn codex_not_read() -> Result<CodexEnv> {
    Ok(CodexEnv {
        home_exists: false,
        home: paths::codex_home()?.to_string_lossy().to_string(),
        config_state: ConfigState::Missing,
        auth_state: AuthState::Missing,
        current_base_url: None,
        current_model: None,
        current_reasoning_effort: None,
        residue: Vec::new(),
        managed_by_us: false,
        active_profile_id: None,
        unsaved_gateway_key: false,
        configuration_status: ConfigurationStatus::Unknown,
        status_message: NOT_READ.into(),
    })
}

fn claude_not_read() -> Result<ClaudeEnv> {
    Ok(ClaudeEnv {
        home_exists: false,
        home: paths::claude_home()?.to_string_lossy().to_string(),
        settings_state: SettingsState::Missing,
        current_base_url: None,
        current_model: None,
        managed_by_us: false,
        active_profile_id: None,
        unsaved_gateway_key: false,
        configuration_status: ConfigurationStatus::Unknown,
        status_message: NOT_READ.into(),
    })
}

fn scan_codex(provider_id: &str, expected_url: &[String]) -> Result<CodexEnv> {
    let home = paths::codex_home()?;
    let home_exists = home.is_dir();

    let cfg_text = read_text_optional(&paths::config_path()?)?;

    let config_state = match &cfg_text {
        None => ConfigState::Missing,
        Some(t) => match codex_config::parse(t) {
            Ok(_) => ConfigState::Ok,
            Err(e) => ConfigState::Broken(e.to_string()),
        },
    };

    let auth_text = read_text_optional(&paths::auth_path()?)?;
    let auth_state = codex_auth::inspect(auth_text.as_deref());

    let (current_base_url, current_model, current_reasoning_effort, residue) = match &cfg_text {
        Some(t) if matches!(config_state, ConfigState::Ok) => (
            codex_config::current_pointing(t),
            codex_config::current_model(t),
            codex_config::current_reasoning_effort(t),
            codex_config::detect_residue(t),
        ),
        _ => (None, None, None, Vec::new()),
    };

    let mut assessment = assess_codex(
        cfg_text.as_deref(),
        auth_text.as_deref(),
        provider_id,
        expected_url,
    );
    if has_environment_override(&[
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "CODEX_API_KEY",
        "CODEX_ACCESS_TOKEN",
        "CODEX_ID_TOKEN",
        "CODEX_ID_TOKEN_FILE",
        "OPENAI_IDENTITY_TOKEN_FILE",
        "OPENAI_IDENTITY_TOKEN",
    ]) {
        assessment = Assessment::unknown(
            "检测到当前进程的认证或服务地址环境变量；无法仅根据配置文件确认使用哪把密钥",
        );
    }
    let active_profile_id = crate::state::match_profile(assessment.key.as_deref())?;
    let managed_by_us = active_profile_id.is_some();
    let unsaved_gateway_key = assessment.key.as_deref().is_some_and(|k| !k.trim().is_empty()) && !managed_by_us;
    assessment.finish(managed_by_us);
    let auth_state = if cfg_text
        .as_deref()
        .and_then(|t| codex_config::parse(t).ok())
        .and_then(|d| {
            d.get("cli_auth_credentials_store")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        })
        .is_some_and(|store| store != "file")
    {
        AuthState::ExternalStore
    } else {
        auth_state
    };

    Ok(CodexEnv {
        home_exists,
        home: home.to_string_lossy().to_string(),
        config_state,
        auth_state,
        current_base_url,
        current_model,
        current_reasoning_effort,
        residue,
        managed_by_us,
        active_profile_id,
        unsaved_gateway_key,
        configuration_status: assessment.status,
        status_message: assessment.message,
    })
}

fn scan_claude(anthropic_base_url: &[String]) -> Result<ClaudeEnv> {
    let home = paths::claude_home()?;
    let home_exists = home.is_dir();

    let text = read_text_optional(&paths::claude_settings_path()?)?;
    let settings_state = claude_config::inspect(text.as_deref());

    let (current_base_url, current_model) = match (&text, &settings_state) {
        (Some(t), SettingsState::Ok) => (
            claude_config::current_base_url(t),
            claude_config::current_model(t),
        ),
        _ => (None, None),
    };

    let mut assessment = assess_claude(text.as_deref(), anthropic_base_url);
    if has_environment_override(&[
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_API_KEY",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
    ]) {
        assessment = Assessment::unknown(
            "检测到当前进程的 Claude 认证或服务地址环境变量；无法仅根据配置文件确认使用哪把密钥",
        );
    }
    let active_profile_id = crate::state::match_profile(assessment.key.as_deref())?;
    let managed_by_us = active_profile_id.is_some();
    let unsaved_gateway_key = assessment.key.as_deref().is_some_and(|k| !k.trim().is_empty()) && !managed_by_us;
    assessment.finish(managed_by_us);

    Ok(ClaudeEnv {
        home_exists,
        home: home.to_string_lossy().to_string(),
        settings_state,
        current_base_url,
        current_model,
        managed_by_us,
        active_profile_id,
        unsaved_gateway_key,
        configuration_status: assessment.status,
        status_message: assessment.message,
    })
}

fn claude_desktop_not_read() -> Result<ClaudeEnv> {
    Ok(ClaudeEnv {
        home: claude_desktop::data_dir()?.to_string_lossy().to_string(),
        ..claude_not_read()?
    })
}

fn scan_claude_desktop(expected: &[String]) -> Result<ClaudeEnv> {
    let home = claude_desktop::data_dir()?;
    let (path, text) = match claude_desktop::applied()? {
        claude_desktop::Applied::Config(path) => {
            let text = read_text_optional(&path)?;
            (Some(path), text)
        }
        claude_desktop::Applied::None => (None, None),
        claude_desktop::Applied::Unusable(reason) => {
            return Ok(ClaudeEnv {
                home_exists: home.is_dir(),
                home: home.to_string_lossy().to_string(),
                settings_state: SettingsState::Ok,
                configuration_status: ConfigurationStatus::Unknown,
                status_message: reason,
                ..claude_not_read()?
            })
        }
    };
    let settings_state = claude_desktop::inspect(text.as_deref());
    let current = text.as_deref().and_then(claude_desktop::current);
    let mut assessment = assess_claude_desktop(text.as_deref(), expected);
    // 明文：直接按密钥对；密文：只能认「最近一次是我们写的那把」
    let active_profile_id = match assessment.key.as_deref() {
        Some(k) if claude_desktop::is_sealed(k) => crate::state::match_sealed_desktop_key(k)?,
        k => crate::state::match_profile(k)?,
    };
    let managed_by_us = active_profile_id.is_some();
    let unsaved_gateway_key = assessment
        .key
        .as_deref()
        .is_some_and(|k| !k.trim().is_empty() && !claude_desktop::is_sealed(k))
        && !managed_by_us;
    if !managed_by_us && assessment.key.as_deref().is_some_and(claude_desktop::is_sealed) {
        assessment.message =
            "网关地址已配置，密钥已被 Claude 桌面版加密保存，无法确认是哪一把；启动时会重新写入所选密钥".into();
    }
    assessment.finish(managed_by_us);
    Ok(ClaudeEnv {
        home_exists: home.is_dir(),
        home: path.unwrap_or(home).to_string_lossy().to_string(),
        settings_state,
        current_base_url: current.as_ref().and_then(|c| c.base_url.clone()),
        current_model: current.and_then(|c| c.models.into_iter().next()),
        managed_by_us,
        active_profile_id,
        unsaved_gateway_key,
        configuration_status: assessment.status,
        status_message: assessment.message,
    })
}

fn assess_claude_desktop(text: Option<&str>, expected: &[String]) -> Assessment {
    let Some(text) = text else {
        return Assessment::new(ConfigurationStatus::Official, "未启用第三方推理，使用 Claude 官方登录");
    };
    let Some(c) = claude_desktop::current(text) else {
        return Assessment::new(ConfigurationStatus::Broken, "配置文件格式损坏，未确认配置状态");
    };
    match c.provider.as_deref() {
        None => Assessment::new(ConfigurationStatus::Official, "未启用第三方推理，使用 Claude 官方登录"),
        Some("gateway") => {
            if !c.base_url.as_deref().is_some_and(|u| any_url(u, expected)) {
                return Assessment::new(ConfigurationStatus::Custom, "网关地址指向其他服务；未使用本工具的配置");
            }
            if c.other_credentials || c.credential_kind.as_deref().is_some_and(|k| k != "static") {
                return Assessment::unknown("使用了静态 API Key 以外的凭据来源，未确认密钥");
            }
            Assessment::candidate(c.api_key)
        }
        Some(_) => Assessment::new(ConfigurationStatus::Custom, "使用其他第三方推理提供方；未使用本工具的配置"),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigurationStatus {
    Unconfigured,
    Configured,
    Official,
    Custom,
    Unknown,
    Broken,
}

struct Assessment {
    status: ConfigurationStatus,
    message: String,
    key: Option<String>,
}
impl Assessment {
    fn new(status: ConfigurationStatus, message: &str) -> Self {
        Self {
            status,
            message: message.into(),
            key: None,
        }
    }
    fn unknown(message: &str) -> Self {
        Self::new(ConfigurationStatus::Unknown, message)
    }
    fn candidate(key: Option<String>) -> Self {
        Self {
            status: ConfigurationStatus::Unknown,
            message: "网关地址已配置，但凭据未匹配本地密钥；未验证客户端运行状态".into(),
            key,
        }
    }
    fn finish(&mut self, matched: bool) {
        if matched {
            self.status = ConfigurationStatus::Configured;
            self.message =
                "用户配置文件已匹配此密钥；运行中的客户端尚未验证，重新打开后生效".into();
        }
    }
}
fn same_url(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}
/// 配置里的地址是不是认得的那些之一（AI 网关，或某把其他服务密钥的地址）
fn any_url(url: &str, expected: &[String]) -> bool {
    expected.iter().any(|e| same_url(url, e))
}
fn has_environment_override(names: &[&str]) -> bool {
    names
        .iter()
        .any(|name| std::env::var(name).is_ok_and(|v| !v.trim().is_empty()))
}

fn assess_codex(
    config: Option<&str>,
    auth: Option<&str>,
    provider_id: &str,
    expected: &[String],
) -> Assessment {
    let doc = match codex_config::parse(config.unwrap_or("")) {
        Ok(d) => d,
        Err(_) => {
            return Assessment::new(
                ConfigurationStatus::Broken,
                "配置文件格式损坏，未确认配置状态",
            )
        }
    };
    if doc.get("profile").is_some() {
        return Assessment::unknown("配置选择了 Codex profile，需在客户端确认实际使用的配置");
    }
    let provider = doc
        .get("model_provider")
        .and_then(|v| v.as_str())
        .unwrap_or("openai");
    if provider != "openai" && provider != provider_id {
        return Assessment::new(
            ConfigurationStatus::Custom,
            "已选择其他服务提供方；未使用本工具的网关配置",
        );
    }
    if provider == provider_id {
        let table = doc.get("model_providers").and_then(|v| v.get(provider_id));
        let Some(table) = table else {
            return Assessment::unknown("所选服务提供方缺少配置");
        };
        if table.get("env_key").is_some()
            || table.get("auth").is_some()
            || table.get("gateway_oauth").is_some()
            || table.get("requires_openai_auth").and_then(|v| v.as_bool()) == Some(true)
        {
            return Assessment::unknown("所选服务提供方使用其他认证来源，未确认密钥");
        }
        if !table
            .get("base_url")
            .and_then(|v| v.as_str())
            .is_some_and(|url| any_url(url, expected))
        {
            return Assessment::new(
                ConfigurationStatus::Custom,
                "所选服务提供方的地址与当前网关不同",
            );
        }
        return Assessment::candidate(
            table
                .get("experimental_bearer_token")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
        );
    }
    if doc
        .get("model_providers")
        .and_then(|v| v.get("openai"))
        .is_some()
    {
        return Assessment::unknown("内置服务提供方存在额外配置，需在客户端确认认证来源");
    }
    if doc
        .get("cli_auth_credentials_store")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s != "file")
    {
        return Assessment::unknown(
            "Codex 使用系统密钥库或其他认证存储；未读取其凭据，不能根据 auth.json 判断登录",
        );
    }
    let base = doc.get("openai_base_url").and_then(|v| v.as_str());
    let auth_state = codex_auth::inspect(auth);
    if base.is_none() || base.is_some_and(|u| same_url(u, "https://api.openai.com/v1")) {
        return if auth_state == AuthState::OfficialLogin {
            Assessment::new(
                ConfigurationStatus::Official,
                "配置使用官方服务，检测到本地 ChatGPT 登录记录；未验证会话有效性",
            )
        } else if config.is_none() && auth_state == AuthState::Missing {
            Assessment::new(
                ConfigurationStatus::Unconfigured,
                "尚未找到用户配置或登录记录；这不代表客户端未安装",
            )
        } else {
            Assessment::new(
                ConfigurationStatus::Official,
                "配置使用官方默认服务；未验证登录或运行状态",
            )
        };
    }
    if !base.is_some_and(|u| any_url(u, expected)) {
        return Assessment::new(ConfigurationStatus::Custom, "配置指向其他服务地址");
    }
    if auth_state != AuthState::ApiKeyOnly {
        return Assessment::unknown("网关地址已配置，但认证不是可确认的 API Key 模式");
    }
    Assessment::candidate(codex_auth::current_api_key(auth))
}

fn assess_claude(text: Option<&str>, expected: &[String]) -> Assessment {
    let Some(text) = text else {
        return Assessment::new(
            ConfigurationStatus::Unconfigured,
            "尚未找到用户配置文件；这不代表客户端未安装",
        );
    };
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return Assessment::new(ConfigurationStatus::Broken, "配置文件格式损坏"),
    };
    if !value.is_object() || value.get("env").is_some_and(|v| !v.is_object()) {
        return Assessment::new(ConfigurationStatus::Broken, "配置文件结构无效");
    }
    let env = value.get("env");
    if value.get("apiKeyHelper").is_some()
        || [
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "CLAUDE_CODE_USE_FOUNDRY",
        ]
        .iter()
        .any(|key| {
            env.and_then(|v| v.get(key))
                .is_some_and(|v| v.as_str().is_none_or(|s| !s.is_empty()))
        })
    {
        return Assessment::unknown("存在其他认证来源或云服务配置，需在客户端确认");
    }
    let base = claude_config::current_base_url(text);
    if base.is_none() {
        return Assessment::new(
            ConfigurationStatus::Official,
            "配置使用官方默认服务；未验证登录或运行状态",
        );
    }
    if !base.as_deref().is_some_and(|url| any_url(url, expected)) {
        return Assessment::new(ConfigurationStatus::Custom, "配置指向其他服务地址");
    }
    Assessment::candidate(claude_config::current_auth_token(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    const URL: &str = "https://gateway.dkby.com/v1";
    const AUTH: &str = r#"{"OPENAI_API_KEY":"saved-file-key","auth_mode":"apikey"}"#;

    #[test]
    fn selected_bearer_wins_over_stale_auth_file() {
        let cfg = format!(
            r#"model_provider = "starnexus_keytool"
[model_providers.starnexus_keytool]
base_url = "{URL}"
experimental_bearer_token = "selected-key"
"#
        );
        assert_eq!(
            assess_codex(Some(&cfg), Some(AUTH), "starnexus_keytool", &[URL.to_string()])
                .key
                .as_deref(),
            Some("selected-key")
        );
    }
    #[test]
    fn inactive_provider_and_wrong_gateway_never_match_saved_key() {
        for provider in ["openai", "other"] {
            let cfg = format!(
                r#"model_provider = "{provider}"
openai_base_url = "https://other.example/v1"
[model_providers.starnexus_keytool]
base_url = "{URL}"
experimental_bearer_token = "unused-key"
"#
            );
            assert!(assess_codex(Some(&cfg), Some(AUTH), "starnexus_keytool", &[URL.to_string()])
                .key
                .is_none());
        }
    }
    #[test]
    fn keyring_auth_and_broken_config_are_not_claimed_as_active() {
        let cfg = format!("openai_base_url = \"{URL}\"\ncli_auth_credentials_store = \"keyring\"");
        let a = assess_codex(Some(&cfg), Some(AUTH), "starnexus_keytool", &[URL.to_string()]);
        assert_eq!(a.status, ConfigurationStatus::Unknown);
        assert!(a.key.is_none());
        assert_eq!(
            assess_codex(Some("invalid = ["), Some(AUTH), "starnexus_keytool", &[URL.to_string()]).status,
            ConfigurationStatus::Broken
        );
    }
    #[test]
    fn official_auth_does_not_use_stale_api_key() {
        let cfg = format!("openai_base_url = \"{URL}\"");
        let auth = r#"{"auth_mode":"chatgpt","tokens":{},"OPENAI_API_KEY":"stale"}"#;
        assert!(assess_codex(Some(&cfg), Some(auth), "starnexus_keytool", &[URL.to_string()])
            .key
            .is_none());
    }
    #[test]
    fn claude_requires_matching_url_and_unambiguous_auth() {
        // 其他服务密钥的地址也在认得的名单里时，同样取出密钥去核对
        let custom = r#"{"env":{"ANTHROPIC_BASE_URL":"https://relay.example/","ANTHROPIC_AUTH_TOKEN":"relay-key"}}"#;
        let urls = ["https://gateway.dkby.com".to_string(), "https://relay.example".to_string()];
        assert_eq!(assess_claude(Some(custom), &urls).key.as_deref(), Some("relay-key"));
        let cfg = r#"{"env":{"ANTHROPIC_BASE_URL":"https://other.example","ANTHROPIC_AUTH_TOKEN":"saved"}}"#;
        assert!(assess_claude(Some(cfg), &["https://gateway.dkby.com".to_string()])
            .key
            .is_none());
        let cfg = r#"{"env":{"ANTHROPIC_BASE_URL":"https://gateway.dkby.com/","ANTHROPIC_AUTH_TOKEN":"saved"}}"#;
        assert_eq!(
            assess_claude(Some(cfg), &["https://gateway.dkby.com".to_string()])
                .key
                .as_deref(),
            Some("saved")
        );
        let cfg = cfg.replace(
            "\"ANTHROPIC_AUTH_TOKEN\"",
            "\"ANTHROPIC_API_KEY\":\"other\",\"ANTHROPIC_AUTH_TOKEN\"",
        );
        assert!(assess_claude(Some(&cfg), &["https://gateway.dkby.com".to_string()])
            .key
            .is_none());
    }
    #[test]
    fn missing_files_do_not_imply_installed_or_logged_in() {
        assert_eq!(
            assess_codex(None, None, "starnexus_keytool", &[URL.to_string()]).status,
            ConfigurationStatus::Unconfigured
        );
        assert_eq!(
            assess_claude(None, &[URL.to_string()]).status,
            ConfigurationStatus::Unconfigured
        );
    }
}
