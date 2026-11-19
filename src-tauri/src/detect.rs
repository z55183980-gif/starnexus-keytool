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
    pub builtin_relay: bool,
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
    /// 配置里当前生效的密钥（掩码）。不论是 AI 还是第三方，读得到才有值
    pub current_key_masked: Option<String>,
    /// 当前生效的地址和密钥都读得到、且没保存在本工具：可以加入密钥列表
    pub adoptable: bool,
    pub configuration_status: ConfigurationStatus,
    pub status_message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeEnv {
    pub configured_models: Vec<String>,
    pub home_exists: bool,
    pub home: String,
    pub settings_state: SettingsState,
    pub current_base_url: Option<String>,
    pub current_model: Option<String>,
    pub managed_by_us: bool,
    pub active_profile_id: Option<String>,
    /// 同 CodexEnv::unsaved_gateway_key
    pub unsaved_gateway_key: bool,
    /// 同 CodexEnv::current_key_masked
    pub current_key_masked: Option<String>,
    /// 同 CodexEnv::adoptable
    pub adoptable: bool,
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
    scan_selected_for(provider_id, openai_base_url, claude, codex, None)
}

pub fn scan_selected_for(provider_id: &str, openai_base_url: &str, claude: bool, codex: bool, preferred: Option<&str>) -> Result<Environment> {
    scan_targets(provider_id, openai_base_url, claude, claude, codex, preferred)
}

pub fn scan_targets(provider_id: &str, openai_base_url: &str, claude: bool, desktop: bool, codex: bool, preferred: Option<&str>) -> Result<Environment> {
    // 指向 AI 网关或任何一把其他服务密钥的地址，都要按密钥核对是不是本工具写的
    let (custom_codex, custom_claude) = crate::state::custom_base_urls().unwrap_or_default();
    let mut codex_urls = vec![openai_base_url.trim_end_matches('/').to_string()];
    codex_urls.extend(custom_codex);
    let mut claude_urls = vec![claude_config::derive_anthropic_base_url(openai_base_url)];
    claude_urls.extend(custom_claude);
    Ok(Environment {
        codex: if codex {
            // 读不了（权限、编码、密钥状态等）不等于文件损坏：标为「未能识别」，不锁死启动；
            // 保存时会给出具体原因。真正的 TOML 损坏在 scan_codex 内部判定，并提供重建入口。
            match scan_codex(provider_id, &codex_urls, openai_base_url, preferred) {
                Ok(env) => env,
                Err(error) => CodexEnv {
                    configuration_status: ConfigurationStatus::Unknown,
                    status_message: format!("无法读取 Codex 配置：{error}"),
                    ..codex_not_read()?
                },
            }
        } else { codex_not_read()? },
        claude_desktop: if desktop { isolated_claude(scan_claude_desktop(&claude_urls, openai_base_url, preferred), claude_desktop_not_read)? } else { claude_desktop_not_read()? },
        claude: if claude { isolated_claude(scan_claude(&claude_urls, openai_base_url, preferred), claude_not_read)? } else { claude_not_read()? },
    })
}

const NOT_READ: &str = "尚未读取（用户还没同意读取这个工具的配置）";

/// 一方读取失败不影响另一方。读不了不等于损坏：标为「未能识别」并说明原因，不锁死启动。
fn isolated_claude(result: Result<ClaudeEnv>, empty: fn() -> Result<ClaudeEnv>) -> Result<ClaudeEnv> {
    match result {
        Ok(env) => Ok(env),
        Err(error) => Ok(ClaudeEnv { configuration_status: ConfigurationStatus::Unknown, status_message: format!("无法读取 Claude 配置：{error}"), ..empty()? }),
    }
}

fn codex_not_read() -> Result<CodexEnv> {
    Ok(CodexEnv {
        builtin_relay: false,
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
        current_key_masked: None,
        adoptable: false,
        configuration_status: ConfigurationStatus::Unknown,
        status_message: NOT_READ.into(),
    })
}

fn claude_not_read() -> Result<ClaudeEnv> {
    Ok(ClaudeEnv {
        configured_models: Vec::new(),
        home_exists: false,
        home: paths::claude_home()?.to_string_lossy().to_string(),
        settings_state: SettingsState::Missing,
        current_base_url: None,
        current_model: None,
        managed_by_us: false,
        active_profile_id: None,
        unsaved_gateway_key: false,
        current_key_masked: None,
        adoptable: false,
        configuration_status: ConfigurationStatus::Unknown,
        status_message: NOT_READ.into(),
    })
}

fn scan_codex(provider_id: &str, expected_url: &[String], gateway: &str, preferred: Option<&str>) -> Result<CodexEnv> {
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
    let overridden = has_environment_override(&[
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "CODEX_API_KEY",
        "CODEX_ACCESS_TOKEN",
        "CODEX_ID_TOKEN",
        "CODEX_ID_TOKEN_FILE",
        "OPENAI_IDENTITY_TOKEN_FILE",
        "OPENAI_IDENTITY_TOKEN",
    ]);
    if overridden {
        assessment = Assessment::unknown(
            "检测到当前进程的认证或服务地址环境变量；无法仅根据配置文件确认使用哪把密钥",
        );
    }
    // 环境变量会盖过配置文件：那时读出来的未必是真正生效的，不提供加入
    let live = if overridden { None } else { live_codex(cfg_text.as_deref(), auth_text.as_deref()) };
    let active_profile_id = crate::state::match_profile_for(assessment.key.as_deref(), current_base_url.as_deref(), true, gateway, preferred)?;
    let managed_by_us = active_profile_id.is_some();
    let unsaved_gateway_key = assessment.key.as_deref().is_some_and(|k| !k.trim().is_empty()) && !managed_by_us;
    let (current_key_masked, adoptable) = live_summary(live.as_ref(), managed_by_us);
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
        builtin_relay: cfg_text.as_deref().is_some_and(codex_config::has_builtin_relay),
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
        current_key_masked,
        adoptable,
        configuration_status: assessment.status,
        status_message: assessment.message,
    })
}

fn scan_claude(anthropic_base_url: &[String], gateway: &str, preferred: Option<&str>) -> Result<ClaudeEnv> {
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
    let overridden = has_environment_override(&[
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_API_KEY",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
    ]);
    let live = if overridden { None } else { live_claude(text.as_deref()) };
    if overridden {
        assessment = Assessment::unknown(
            "检测到当前进程的 Claude 认证或服务地址环境变量；无法仅根据配置文件确认使用哪把密钥",
        );
    }
    let active_profile_id = crate::state::match_profile_for(assessment.key.as_deref(), current_base_url.as_deref(), false, gateway, preferred)?;
    let managed_by_us = active_profile_id.is_some();
    let unsaved_gateway_key = assessment.key.as_deref().is_some_and(|k| !k.trim().is_empty()) && !managed_by_us;
    let (current_key_masked, adoptable) = live_summary(live.as_ref(), managed_by_us);
    assessment.finish(managed_by_us);

    Ok(ClaudeEnv {
        configured_models: {
            let value = text.as_deref().and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok());
            [claude_config::KEY_DEFAULT_OPUS, claude_config::KEY_DEFAULT_SONNET, claude_config::KEY_DEFAULT_HAIKU].iter().map(|name| value.as_ref().and_then(|value| value.get("env")?.get(*name)?.as_str()).unwrap_or_default().to_owned()).collect()
        },
        home_exists,
        home: home.to_string_lossy().to_string(),
        settings_state,
        current_base_url,
        current_model,
        managed_by_us,
        active_profile_id,
        unsaved_gateway_key,
        current_key_masked,
        adoptable,
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

fn scan_claude_desktop(expected: &[String], gateway: &str, preferred: Option<&str>) -> Result<ClaudeEnv> {
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
    // 明文：直接按密钥对；密文：只认本工具写入后、条目/地址/模型都没变时由桌面版加密的那把
    let key = match (assessment.key.as_deref(), path.as_deref(), current.as_ref()) {
        (Some(k), Some(config), Some(c)) if claude_desktop::is_sealed(k) => {
            let config = config.to_string_lossy();
            let entry = crate::state::DesktopEntry { path: &config, base_url: c.base_url.as_deref(), models: &c.models };
            crate::state::match_sealed_desktop_key(k, &entry)?
                .map(|id| crate::state::load_profile_key(&id)).transpose()?.flatten()
        }
        (Some(k), _, _) if claude_desktop::is_sealed(k) => None,
        (k, _, _) => k.map(str::to_string),
    };
    let active_profile_id = crate::state::match_profile_for(key.as_deref(), current.as_ref().and_then(|c| c.base_url.as_deref()), false, gateway, preferred)?;
    let managed_by_us = active_profile_id.is_some();
    let unsaved_gateway_key = assessment
        .key
        .as_deref()
        .is_some_and(|k| !k.trim().is_empty() && !claude_desktop::is_sealed(k))
        && !managed_by_us;
    let (current_key_masked, adoptable) = live_summary(text.as_deref().and_then(live_claude_desktop).as_ref(), managed_by_us);
    if !managed_by_us && assessment.key.as_deref().is_some_and(claude_desktop::is_sealed) {
        assessment.message =
            "桌面版已加密保存密钥，无法确认是哪一把；保存配置会重新写入所选密钥。".into();
    }
    assessment.finish(managed_by_us);
    Ok(ClaudeEnv {
        configured_models: current.as_ref().map(|entry| entry.models.clone()).unwrap_or_default(),
        home_exists: home.is_dir(),
        home: path.unwrap_or(home).to_string_lossy().to_string(),
        settings_state,
        current_base_url: current.as_ref().and_then(|c| c.base_url.clone()),
        current_model: current.and_then(|c| c.models.into_iter().next()),
        managed_by_us,
        active_profile_id,
        unsaved_gateway_key,
        current_key_masked,
        adoptable,
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

/// 配置文件里当前生效的服务地址、密钥和模型，不论是 AI 网关还是第三方服务。
/// 只在地址和静态密钥都能从配置文件里直接读到时才有值（系统密钥库、环境变量、云服务等读不到的返回 None）。
#[derive(Debug, Clone, PartialEq)]
pub struct LiveCredential {
    pub base_url: String,
    pub key: String,
    pub model: String,
}

fn non_empty(s: Option<&str>) -> Option<String> {
    s.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// 掩码与「能否加入」：已经对上本工具里某把密钥的，不再提供加入
fn live_summary(live: Option<&LiveCredential>, managed_by_us: bool) -> (Option<String>, bool) {
    match live {
        Some(c) => (Some(crate::state::mask(&c.key)), !managed_by_us),
        None => (None, false),
    }
}

/// Codex：config.toml 选中的服务提供方 + 它的密钥（provider 段里的 bearer token，或 auth.json 的 API Key）
pub fn live_codex(config: Option<&str>, auth: Option<&str>) -> Option<LiveCredential> {
    let text = config?;
    let doc = codex_config::parse(text).ok()?;
    if doc.get("profile").is_some() {
        return None;
    }
    if doc
        .get("cli_auth_credentials_store")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s != "file")
    {
        return None;
    }
    let auth_key = || {
        if codex_auth::inspect(auth) != AuthState::ApiKeyOnly {
            return None;
        }
        non_empty(codex_auth::current_api_key(auth).as_deref())
    };
    let provider = doc.get("model_provider").and_then(|v| v.as_str()).unwrap_or("openai");
    let (base_url, key) = if provider == "openai" {
        let base = non_empty(doc.get("openai_base_url").and_then(|v| v.as_str()))?;
        if same_url(&base, "https://api.openai.com/v1") {
            return None;
        }
        (base, auth_key()?)
    } else {
        let table = doc.get("model_providers").and_then(|v| v.get(provider))?;
        if table.get("env_key").is_some() || table.get("auth").is_some() || table.get("gateway_oauth").is_some() {
            return None;
        }
        let base = non_empty(table.get("base_url").and_then(|v| v.as_str()))?;
        let key = match non_empty(table.get("experimental_bearer_token").and_then(|v| v.as_str())) {
            Some(k) => k,
            None if table.get("requires_openai_auth").and_then(|v| v.as_bool()) == Some(true) => auth_key()?,
            None => return None,
        };
        (base, key)
    };
    Some(LiveCredential { base_url, key, model: codex_config::current_model(text).unwrap_or_default() })
}

/// Claude Code：settings.json 的 env 里写了服务地址和 ANTHROPIC_AUTH_TOKEN
pub fn live_claude(text: Option<&str>) -> Option<LiveCredential> {
    let text = text?;
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let env = value.get("env");
    if value.get("apiKeyHelper").is_some()
        || ["CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX", "CLAUDE_CODE_USE_FOUNDRY"]
            .iter()
            .any(|k| env.and_then(|v| v.get(k)).is_some_and(|v| v.as_str().is_none_or(|s| !s.is_empty())))
    {
        return None;
    }
    Some(LiveCredential {
        base_url: non_empty(claude_config::current_base_url(text).as_deref())?,
        key: non_empty(claude_config::current_auth_token(text).as_deref())?,
        model: claude_config::current_model(text).unwrap_or_default(),
    })
}

/// Claude 桌面版：第三方推理（gateway）+ 明文静态密钥。被桌面版加密保存的读不出原文，返回 None
pub fn live_claude_desktop(text: &str) -> Option<LiveCredential> {
    let c = claude_desktop::current(text)?;
    if c.provider.as_deref() != Some("gateway")
        || c.other_credentials
        || c.credential_kind.as_deref().is_some_and(|k| k != "static")
    {
        return None;
    }
    let key = non_empty(c.api_key.as_deref()).filter(|k| !claude_desktop::is_sealed(k))?;
    Some(LiveCredential {
        base_url: non_empty(c.base_url.as_deref())?,
        key,
        model: c.models.into_iter().next().unwrap_or_default(),
    })
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
    #[test]
    fn live_codex_reads_third_party_provider_and_api_key_modes() {
        let cfg = r#"model = "relay-model"
model_provider = "relay"
[model_providers.relay]
base_url = "https://relay.example/v1"
experimental_bearer_token = "relay-key"
"#;
        assert_eq!(
            live_codex(Some(cfg), None),
            Some(LiveCredential { base_url: "https://relay.example/v1".into(), key: "relay-key".into(), model: "relay-model".into() })
        );
        let cfg = "openai_base_url = \"https://relay.example/v1\"\n";
        assert_eq!(live_codex(Some(cfg), Some(AUTH)).map(|c| c.key), Some("saved-file-key".into()));
    }
    #[test]
    fn live_codex_skips_official_and_unreadable_credentials() {
        assert!(live_codex(Some("model = \"gpt\"\n"), Some(AUTH)).is_none());
        assert!(live_codex(None, Some(AUTH)).is_none());
        let env_key = r#"model_provider = "relay"
[model_providers.relay]
base_url = "https://relay.example/v1"
env_key = "RELAY_KEY"
"#;
        assert!(live_codex(Some(env_key), None).is_none());
        let keyring = "cli_auth_credentials_store = \"keyring\"\nopenai_base_url = \"https://relay.example/v1\"\n";
        assert!(live_codex(Some(keyring), Some(AUTH)).is_none());
    }
    #[test]
    fn live_claude_reads_third_party_token_but_not_cloud_providers() {
        let cfg = r#"{"env":{"ANTHROPIC_BASE_URL":"https://relay.example","ANTHROPIC_AUTH_TOKEN":"relay-key","ANTHROPIC_MODEL":"m"}}"#;
        assert_eq!(
            live_claude(Some(cfg)),
            Some(LiveCredential { base_url: "https://relay.example".into(), key: "relay-key".into(), model: "m".into() })
        );
        assert!(live_claude(Some(r#"{"env":{"ANTHROPIC_BASE_URL":"https://relay.example"}}"#)).is_none());
        assert!(live_claude(Some(r#"{"env":{"ANTHROPIC_BASE_URL":"https://relay.example","ANTHROPIC_AUTH_TOKEN":"k","CLAUDE_CODE_USE_BEDROCK":"1"}}"#)).is_none());
        assert!(live_claude(Some("{}")).is_none());
    }
    #[test]
    fn live_claude_desktop_needs_plain_static_gateway_key() {
        let plain = r#"{"inferenceProvider":"gateway","inferenceGatewayBaseUrl":"https://relay.example","inferenceGatewayApiKey":"relay-key","inferenceModels":["m"]}"#;
        assert_eq!(live_claude_desktop(plain).map(|c| (c.key, c.model)), Some(("relay-key".into(), "m".into())));
        let sealed = plain.replace("relay-key", "__sealed__:v1:abc");
        assert!(live_claude_desktop(&sealed).is_none());
        assert!(live_claude_desktop("{}").is_none());
    }
    #[test]
    fn saved_keys_are_not_offered_for_adoption() {
        let live = LiveCredential { base_url: "https://relay.example".into(), key: "relay-key-123456".into(), model: String::new() };
        assert_eq!(live_summary(Some(&live), false), (Some(crate::state::mask("relay-key-123456")), true));
        assert_eq!(live_summary(Some(&live), true).1, false);
        assert_eq!(live_summary(None, false), (None, false));
    }
}
