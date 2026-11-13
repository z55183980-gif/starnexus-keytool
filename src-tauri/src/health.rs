//! 体检：用规则检查配置里常见的坑，不调用模型。
//!
//! 每条规则都是纯函数，输入是配置文件原文和用户在卡片上的选择，输出是问题列表。
//! 修复只引用已有的写入动作（重新写入 / 重建配置），这里不写任何文件。

use crate::claude_config::{
    KEY_AUTH_TOKEN, KEY_BASE_URL, KEY_DEFAULT_HAIKU, KEY_DEFAULT_OPUS, KEY_DEFAULT_SONNET,
    KEY_LEGACY_API_KEY, KEY_MODEL,
};
use crate::{codex_auth, codex_config};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Warn,
}

/// 问题行上的按钮。只能是已有的动作。
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Fix {
    /// 用卡片选中的 AI 密钥重新写入（apply_config）
    Rewrite,
    /// 备份后重建 Codex 最小配置（rebuild_minimal_config）
    Rebuild,
    /// 去密钥页
    Keys,
    /// 打开工具页的「更换模型」
    Model,
    /// 去设置页
    Settings,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub id: &'static str,
    pub level: Level,
    pub text: String,
    pub fix: Fix,
}

fn issue(id: &'static str, level: Level, text: &str, fix: Fix) -> Issue {
    Issue { id, level, text: text.to_string(), fix }
}

/// 卡片上选的是哪类服务。决定哪些规则有意义：
/// 选了 AI 密钥时，启动流程本来就会重写地址，地址类问题不必提前报。
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Service {
    Current,
    Official,
    Ai,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    Claude,
    Codex,
}

/// 连接测试的结果（`/models`，不计费）
pub enum Connection {
    /// 连不上或超时
    Network,
    /// 服务端返回了非 2xx
    Http(u16),
    /// 成功，附带可用模型
    Ok(Vec<String>),
}

fn host(url: &str) -> Option<String> {
    let rest = url
        .trim()
        .strip_prefix("https://")
        .or_else(|| url.trim().strip_prefix("http://"))?;
    let h = rest.split('/').next()?.to_ascii_lowercase();
    (!h.is_empty()).then_some(h)
}

fn same_url(a: &str, b: &str) -> bool {
    a.trim().trim_end_matches('/') == b.trim().trim_end_matches('/')
}

/// 指向了网关的域名，但路径不对（最典型的就是 /v1 多了或少了）
fn wrong_gateway_path(current: &str, expected: &str) -> bool {
    host(current).is_some() && host(current) == host(expected) && !same_url(current, expected)
}

fn non_empty(env: Option<&Value>, key: &str) -> bool {
    env.and_then(|e| e.get(key))
        .is_some_and(|v| v.as_str().is_none_or(|s| !s.trim().is_empty()))
}

pub fn check_claude(text: Option<&str>, anthropic_base: &str, service: Service) -> Vec<Issue> {
    let mut out = Vec::new();
    let Some(text) = text.filter(|t| !t.trim().is_empty()) else {
        return out;
    };
    let value = match serde_json::from_str::<Value>(text) {
        Ok(v) if v.is_object() && v.get("env").is_none_or(|e| e.is_object()) => v,
        _ => {
            out.push(issue("C1", Level::Error, "Claude 配置文件格式损坏，需手动修复", Fix::None));
            return out;
        }
    };
    if service == Service::Official {
        // 启动时会切回官方，AI 相关的配置问题都不再相关
        return out;
    }
    let env = value.get("env");
    let base = env.and_then(|e| e.get(KEY_BASE_URL)).and_then(|v| v.as_str());
    let on_gateway = base.is_some_and(|b| host(b).is_some() && host(b) == host(anthropic_base));

    if on_gateway
        && ![KEY_DEFAULT_OPUS, KEY_DEFAULT_SONNET, KEY_DEFAULT_HAIKU]
            .iter()
            .all(|k| non_empty(env, k))
    {
        out.push(issue("C2", Level::Error, "模型别名不全，切换模型会报 404", Fix::Rewrite));
    }
    if service == Service::Current {
        if base.is_some_and(|b| wrong_gateway_path(b, anthropic_base)) {
            out.push(issue("C3", Level::Error, "Claude 服务地址不对", Fix::Rewrite));
        }
        if on_gateway && non_empty(env, KEY_LEGACY_API_KEY) {
            out.push(issue("C4", Level::Warn, "同时设置了两种密钥，可能冲突", Fix::Rewrite));
        }
    }
    if value.get("apiKeyHelper").is_some()
        || [
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "CLAUDE_CODE_USE_FOUNDRY",
        ]
        .iter()
        .any(|k| non_empty(env, k))
    {
        out.push(issue("C5", Level::Warn, "有其他认证方式，可能不生效", Fix::None));
    }
    out
}

/// Claude 桌面版的第三方推理配置（它不读 settings.json，规则单独一套）。
/// `restart_pending`：配置里还是明文密钥且桌面版正在运行 —— 桌面版启动时会把明文加密，
/// 还是明文就说明写入后它没重新打开过，运行中的那个还在用旧配置。
pub fn check_claude_desktop(
    text: Option<&str>,
    anthropic_base: &str,
    service: Service,
    restart_pending: bool,
) -> Vec<Issue> {
    let mut out = Vec::new();
    let Some(text) = text.filter(|t| !t.trim().is_empty()) else {
        return out;
    };
    let Some(c) = crate::claude_desktop::current(text) else {
        out.push(issue("D1", Level::Error, "Claude 桌面版配置文件格式损坏，需手动修复", Fix::None));
        return out;
    };
    if service == Service::Official {
        return out;
    }
    let on_gateway = c.provider.as_deref() == Some("gateway")
        && c.base_url.as_deref().is_some_and(|b| host(b).is_some() && host(b) == host(anthropic_base));
    if service == Service::Current && on_gateway {
        if c.base_url.as_deref().is_some_and(|b| wrong_gateway_path(b, anthropic_base)) {
            out.push(issue("D2", Level::Error, "Claude 桌面版服务地址不对", Fix::Rewrite));
        }
        if c.other_credentials || c.credential_kind.as_deref().is_some_and(|k| k != "static") {
            out.push(issue("D3", Level::Warn, "Claude 桌面版用了其他认证方式，可能不生效", Fix::Rewrite));
        }
    }
    if restart_pending {
        out.push(issue(
            "D4",
            Level::Warn,
            "Claude 桌面版还没重新打开，新配置尚未生效：请完全退出（包括托盘图标）后再打开",
            Fix::None,
        ));
    }
    out
}

/// Claude 桌面版当前在用的密钥与模型（只在指向网关时）。
/// 密钥被桌面版加密后读不出原文，`resolve_sealed` 负责按本工具的写入记录认出是哪一把。
pub fn current_claude_desktop(
    text: Option<&str>,
    anthropic_base: &str,
    resolve_sealed: impl Fn(&str) -> Option<String>,
) -> Option<CurrentSetup> {
    let c = crate::claude_desktop::current(text?)?;
    if c.provider.as_deref() != Some("gateway") {
        return None;
    }
    let base = c.base_url.as_deref()?;
    if host(base).is_none() || host(base) != host(anthropic_base) {
        return None;
    }
    let raw = c.api_key?;
    let key = if crate::claude_desktop::is_sealed(&raw) { resolve_sealed(&raw)? } else { raw };
    Some(CurrentSetup {
        key,
        model: c.models.into_iter().next().unwrap_or_default(),
        aliases: Vec::new(),
    })
}

pub fn check_codex(text: Option<&str>, openai_base: &str, service: Service) -> Vec<Issue> {
    let mut out = Vec::new();
    let Some(text) = text.filter(|t| !t.trim().is_empty()) else {
        return out;
    };
    let doc = match codex_config::parse(text) {
        Ok(d) => d,
        Err(_) => {
            out.push(issue("X1", Level::Error, "Codex 配置文件格式损坏", Fix::Rebuild));
            return out;
        }
    };
    if service == Service::Official {
        return out;
    }
    if service == Service::Current
        && codex_config::current_pointing(text).is_some_and(|u| wrong_gateway_path(&u, openai_base))
    {
        out.push(issue("X2", Level::Error, "Codex 服务地址不对", Fix::Rewrite));
    }
    if doc.get("profile").is_some() {
        out.push(issue("X3", Level::Warn, "启用了 Codex profile，可能不生效", Fix::None));
    }
    out
}

pub const CLAUDE_ENV_VARS: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
];

pub const CODEX_ENV_VARS: &[&str] = &[
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "CODEX_API_KEY",
    "CODEX_ACCESS_TOKEN",
    "CODEX_ID_TOKEN",
    "CODEX_ID_TOKEN_FILE",
    "OPENAI_IDENTITY_TOKEN_FILE",
    "OPENAI_IDENTITY_TOKEN",
];

pub fn check_env(tool: Tool, is_set: impl Fn(&str) -> bool) -> Vec<Issue> {
    let vars = match tool {
        Tool::Claude => CLAUDE_ENV_VARS,
        Tool::Codex => CODEX_ENV_VARS,
    };
    if vars.iter().any(|v| is_set(v)) {
        vec![issue("E1", Level::Warn, "系统环境变量会覆盖这里的配置", Fix::None)]
    } else {
        Vec::new()
    }
}

pub fn check_connection(conn: &Connection, model: &str) -> Vec<Issue> {
    match conn {
        Connection::Network => vec![issue("K2", Level::Error, "连不上服务器", Fix::Settings)],
        Connection::Http(401 | 403) => {
            vec![issue("K1", Level::Error, "密钥无效或已停用", Fix::Keys)]
        }
        Connection::Http(_) => vec![issue("K4", Level::Warn, "服务暂时不可用，稍后再试", Fix::None)],
        Connection::Ok(models) => {
            let model = model.trim();
            if !model.is_empty() && !models.is_empty() && !models.iter().any(|m| m == model) {
                vec![issue("K3", Level::Error, "所选模型不可用，换一个", Fix::Model)]
            } else {
                Vec::new()
            }
        }
    }
}

/// 这个工具调用网关用的接口：Codex 走 Responses，Claude Code 走 Messages
pub fn required_endpoint(tool: Tool) -> &'static str {
    match tool {
        Tool::Codex => "responses",
        Tool::Claude => "messages",
    }
}

/// 模型在 /models 列表里，不代表这个工具用的接口上有渠道提供它。按网关目录核对；
/// 目录里没有这个模型、或不知道它支持哪些接口时不报（K3 已经管「不在列表里」）。
pub fn check_endpoint(tool: Tool, model: &str, catalog: &[crate::api::ModelInfo]) -> Vec<Issue> {
    let model = model.trim();
    let Some(info) = catalog.iter().find(|m| m.id == model) else {
        return Vec::new();
    };
    if info.endpoints.is_empty() || info.endpoints.iter().any(|e| e == required_endpoint(tool)) {
        return Vec::new();
    }
    let text = match tool {
        Tool::Codex => "所选模型不支持 Codex 使用的接口，换一个模型",
        Tool::Claude => "所选模型不支持 Claude Code 使用的接口，换一个模型",
    };
    vec![issue("K6", Level::Error, text, Fix::Model)]
}

/// 当前配置里实际在用的密钥和模型（只在指向网关时取）。
/// 「沿用当前配置」时用它测连接，才能发现配置里的模型这把密钥用不了。
pub struct CurrentSetup {
    pub key: String,
    pub model: String,
    /// (别名, 模型)，只有 Claude 有
    pub aliases: Vec<(&'static str, String)>,
}

pub fn current_claude(text: Option<&str>, anthropic_base: &str) -> Option<CurrentSetup> {
    let v: Value = serde_json::from_str(text?).ok()?;
    let env = v.get("env")?;
    let s = |k: &str| env.get(k).and_then(|x| x.as_str()).map(str::trim).unwrap_or("").to_string();
    let base = s(KEY_BASE_URL);
    if host(&base).is_none() || host(&base) != host(anthropic_base) {
        return None;
    }
    let key = s(KEY_AUTH_TOKEN);
    (!key.is_empty()).then(|| CurrentSetup {
        key,
        model: s(KEY_MODEL),
        aliases: vec![
            ("opus", s(KEY_DEFAULT_OPUS)),
            ("sonnet", s(KEY_DEFAULT_SONNET)),
            ("haiku", s(KEY_DEFAULT_HAIKU)),
        ],
    })
}

pub fn current_codex(
    config: Option<&str>,
    auth: Option<&str>,
    openai_base: &str,
    provider_id: &str,
) -> Option<CurrentSetup> {
    let config = config?;
    let url = codex_config::current_pointing(config)?;
    if host(&url).is_none() || host(&url) != host(openai_base) {
        return None;
    }
    let key = codex_config::current_bearer_token(config, provider_id)
        .or_else(|| codex_auth::current_api_key(auth))
        .filter(|k| !k.trim().is_empty())?;
    Some(CurrentSetup {
        key,
        model: codex_config::current_model(config).unwrap_or_default(),
        aliases: Vec::new(),
    })
}

/// 别名指向的模型这把密钥用不了：/model opus 之类的切换会失败
pub fn check_aliases(conn: &Connection, aliases: &[(&str, &str)]) -> Vec<Issue> {
    let Connection::Ok(models) = conn else {
        return Vec::new();
    };
    if models.is_empty() {
        return Vec::new();
    }
    let missing: Vec<&str> = aliases
        .iter()
        .filter(|(_, m)| !m.trim().is_empty() && !models.iter().any(|a| a == m.trim()))
        .map(|(label, _)| *label)
        .collect();
    if missing.is_empty() {
        return Vec::new();
    }
    vec![issue(
        "K5",
        Level::Error,
        &format!("别名 {} 对应的模型不可用", missing.join("、")),
        Fix::Settings,
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE_BASE: &str = "https://gateway.dkby.com";
    const CODEX_BASE: &str = "https://gateway.dkby.com/v1";

    fn ids(v: &[Issue]) -> Vec<&'static str> {
        v.iter().map(|i| i.id).collect()
    }

    fn claude(env: &str) -> String {
        format!(r#"{{"env":{env}}}"#)
    }

    const FULL: &str = r#"{"ANTHROPIC_BASE_URL":"https://gateway.dkby.com","ANTHROPIC_AUTH_TOKEN":"sk-x","ANTHROPIC_DEFAULT_OPUS_MODEL":"o","ANTHROPIC_DEFAULT_SONNET_MODEL":"s","ANTHROPIC_DEFAULT_HAIKU_MODEL":"h"}"#;

    fn desktop(provider: &str, url: &str, extra: &str) -> String {
        format!(r#"{{"inferenceProvider":"{provider}","inferenceGatewayBaseUrl":"{url}","inferenceGatewayApiKey":"sk-1"{extra}}}"#)
    }

    #[test]
    fn desktop_rules() {
        let ok = desktop("gateway", CLAUDE_BASE, r#","inferenceCredentialKind":"static""#);
        assert!(check_claude_desktop(Some(&ok), CLAUDE_BASE, Service::Current, false).is_empty());
        assert!(check_claude_desktop(None, CLAUDE_BASE, Service::Current, false).is_empty());
        assert_eq!(ids(&check_claude_desktop(Some("{x"), CLAUDE_BASE, Service::Ai, false)), ["D1"]);
        let v1 = desktop("gateway", "https://gateway.dkby.com/v1", "");
        assert_eq!(ids(&check_claude_desktop(Some(&v1), CLAUDE_BASE, Service::Current, false)), ["D2"]);
        // 选了 AI 密钥时启动会重写，地址问题不提前报
        assert!(check_claude_desktop(Some(&v1), CLAUDE_BASE, Service::Ai, false).is_empty());
        let helper = desktop("gateway", CLAUDE_BASE, r#","inferenceCredentialHelper":"x""#);
        assert_eq!(ids(&check_claude_desktop(Some(&helper), CLAUDE_BASE, Service::Current, false)), ["D3"]);
        assert_eq!(ids(&check_claude_desktop(Some(&ok), CLAUDE_BASE, Service::Ai, true)), ["D4"]);
        assert!(check_claude_desktop(Some(&ok), CLAUDE_BASE, Service::Official, true).is_empty());
    }

    #[test]
    fn reads_current_desktop_setup() {
        let plain = desktop("gateway", CLAUDE_BASE, r#","inferenceModels":["m1","m2"]"#);
        let c = current_claude_desktop(Some(&plain), CLAUDE_BASE, |_| None).unwrap();
        assert_eq!((c.key.as_str(), c.model.as_str()), ("sk-1", "m1"));
        let sealed = r#"{"inferenceProvider":"gateway","inferenceGatewayBaseUrl":"https://gateway.dkby.com","inferenceGatewayApiKey":"__sealed__:v1:AA"}"#;
        assert!(current_claude_desktop(Some(sealed), CLAUDE_BASE, |_| None).is_none());
        assert_eq!(current_claude_desktop(Some(sealed), CLAUDE_BASE, |_| Some("sk-w".into())).unwrap().key, "sk-w");
        let other = desktop("gateway", "https://relay.example", "");
        assert!(current_claude_desktop(Some(&other), CLAUDE_BASE, |_| Some("x".into())).is_none());
        let bedrock = desktop("bedrock", CLAUDE_BASE, "");
        assert!(current_claude_desktop(Some(&bedrock), CLAUDE_BASE, |_| None).is_none());
    }

    #[test]
    fn healthy_claude_has_no_issues() {
        assert!(check_claude(Some(&claude(FULL)), CLAUDE_BASE, Service::Current).is_empty());
        assert!(check_claude(None, CLAUDE_BASE, Service::Current).is_empty());
    }

    #[test]
    fn c1_broken_json() {
        let v = check_claude(Some("{ not json"), CLAUDE_BASE, Service::Ai);
        assert_eq!(ids(&v), ["C1"]);
        assert_eq!(v[0].fix, Fix::None);
        assert_eq!(ids(&check_claude(Some(r#"{"env":1}"#), CLAUDE_BASE, Service::Current)), ["C1"]);
    }

    #[test]
    fn c2_missing_alias() {
        let t = claude(r#"{"ANTHROPIC_BASE_URL":"https://gateway.dkby.com","ANTHROPIC_DEFAULT_OPUS_MODEL":"o"}"#);
        assert_eq!(ids(&check_claude(Some(&t), CLAUDE_BASE, Service::Ai)), ["C2"]);
        assert_eq!(ids(&check_claude(Some(&t), CLAUDE_BASE, Service::Current)), ["C2"]);
        // 切回官方时不提示
        assert!(check_claude(Some(&t), CLAUDE_BASE, Service::Official).is_empty());
        // 没指向网关时别名无所谓
        let other = claude(r#"{"ANTHROPIC_BASE_URL":"https://example.com"}"#);
        assert!(check_claude(Some(&other), CLAUDE_BASE, Service::Current).is_empty());
    }

    #[test]
    fn c3_extra_v1_only_when_keeping_current() {
        let t = claude(&FULL.replace("https://gateway.dkby.com\"", "https://gateway.dkby.com/v1\""));
        assert_eq!(ids(&check_claude(Some(&t), CLAUDE_BASE, Service::Current)), ["C3"]);
        // 选了 AI 密钥，启动时会重写地址
        assert!(check_claude(Some(&t), CLAUDE_BASE, Service::Ai).is_empty());
    }

    #[test]
    fn c4_legacy_api_key() {
        let t = claude(&FULL.replace('}', r#","ANTHROPIC_API_KEY":"sk-y"}"#));
        assert_eq!(ids(&check_claude(Some(&t), CLAUDE_BASE, Service::Current)), ["C4"]);
        assert!(check_claude(Some(&t), CLAUDE_BASE, Service::Ai).is_empty());
    }

    #[test]
    fn c5_other_auth() {
        let t = r#"{"apiKeyHelper":"x","env":{}}"#;
        assert_eq!(ids(&check_claude(Some(t), CLAUDE_BASE, Service::Ai)), ["C5"]);
        let t = claude(r#"{"CLAUDE_CODE_USE_BEDROCK":"1"}"#);
        assert_eq!(ids(&check_claude(Some(&t), CLAUDE_BASE, Service::Current)), ["C5"]);
    }

    #[test]
    fn x1_broken_toml() {
        let v = check_codex(Some("model = "), CODEX_BASE, Service::Official);
        assert_eq!(ids(&v), ["X1"]);
        assert_eq!(v[0].fix, Fix::Rebuild);
    }

    #[test]
    fn x2_missing_v1() {
        let t = "openai_base_url = \"https://gateway.dkby.com\"\n";
        assert_eq!(ids(&check_codex(Some(t), CODEX_BASE, Service::Current)), ["X2"]);
        assert!(check_codex(Some(t), CODEX_BASE, Service::Ai).is_empty());
        let ok = "openai_base_url = \"https://gateway.dkby.com/v1/\"\n";
        assert!(check_codex(Some(ok), CODEX_BASE, Service::Current).is_empty());
        // 指向别的服务不是我们的问题
        let other = "openai_base_url = \"https://example.com\"\n";
        assert!(check_codex(Some(other), CODEX_BASE, Service::Current).is_empty());
    }

    #[test]
    fn x3_profile() {
        let t = "profile = \"work\"\n";
        assert_eq!(ids(&check_codex(Some(t), CODEX_BASE, Service::Ai)), ["X3"]);
        assert!(check_codex(Some(t), CODEX_BASE, Service::Official).is_empty());
    }

    #[test]
    fn e1_env_override() {
        assert_eq!(ids(&check_env(Tool::Claude, |v| v == "ANTHROPIC_API_KEY")), ["E1"]);
        assert!(check_env(Tool::Codex, |v| v == "ANTHROPIC_API_KEY").is_empty());
        assert_eq!(ids(&check_env(Tool::Codex, |v| v == "OPENAI_BASE_URL")), ["E1"]);
    }

    #[test]
    fn k5_aliases_not_available() {
        let conn = Connection::Ok(vec!["claude-opus-5-5".into(), "claude-sonnet-5-5".into()]);
        let v = check_aliases(&conn, &[("opus", "claude-opus-4-8"), ("sonnet", "claude-sonnet-5-5"), ("haiku", "claude-haiku-4-5")]);
        assert_eq!(ids(&v), ["K5"]);
        assert_eq!(v[0].text, "别名 opus、haiku 对应的模型不可用");
        // 留空的别名不算；全都可用就不报
        assert!(check_aliases(&conn, &[("opus", "claude-opus-5-5"), ("haiku", "")]).is_empty());
        // 连接失败时由 K1/K2 报，这里不重复
        assert!(check_aliases(&Connection::Network, &[("opus", "x")]).is_empty());
        assert!(check_aliases(&Connection::Ok(vec![]), &[("opus", "x")]).is_empty());
    }

    #[test]
    fn reads_current_claude_setup() {
        let t = claude(r#"{"ANTHROPIC_BASE_URL":"https://gateway.dkby.com","ANTHROPIC_AUTH_TOKEN":"sk-x","ANTHROPIC_MODEL":"m","ANTHROPIC_DEFAULT_OPUS_MODEL":"o"}"#);
        let c = current_claude(Some(&t), CLAUDE_BASE).unwrap();
        assert_eq!((c.key.as_str(), c.model.as_str()), ("sk-x", "m"));
        assert_eq!(c.aliases[0], ("opus", "o".to_string()));
        assert_eq!(c.aliases[2], ("haiku", String::new()));
        // 不指向网关、没有密钥、文件损坏都不取
        assert!(current_claude(Some(&claude(r#"{"ANTHROPIC_BASE_URL":"https://x.com","ANTHROPIC_AUTH_TOKEN":"sk-x"}"#)), CLAUDE_BASE).is_none());
        assert!(current_claude(Some(&claude(r#"{"ANTHROPIC_BASE_URL":"https://gateway.dkby.com"}"#)), CLAUDE_BASE).is_none());
        assert!(current_claude(Some("{"), CLAUDE_BASE).is_none());
    }

    #[test]
    fn reads_current_codex_setup() {
        let cfg = "model = \"gpt-6-sol\"\nopenai_base_url = \"https://gateway.dkby.com/v1\"\n";
        let c = current_codex(Some(cfg), Some(r#"{"OPENAI_API_KEY":"sk-a"}"#), CODEX_BASE, "starnexus_keytool").unwrap();
        assert_eq!((c.key.as_str(), c.model.as_str()), ("sk-a", "gpt-6-sol"));
        assert!(current_codex(Some(cfg), None, CODEX_BASE, "starnexus_keytool").is_none());
        let other = "openai_base_url = \"https://api.openai.com/v1\"\n";
        assert!(current_codex(Some(other), Some(r#"{"OPENAI_API_KEY":"sk-a"}"#), CODEX_BASE, "starnexus_keytool").is_none());
    }

    #[test]
    fn connection_rules() {
        assert_eq!(ids(&check_connection(&Connection::Network, "m")), ["K2"]);
        assert_eq!(ids(&check_connection(&Connection::Http(401), "m")), ["K1"]);
        assert_eq!(ids(&check_connection(&Connection::Http(403), "m")), ["K1"]);
        assert_eq!(ids(&check_connection(&Connection::Http(503), "m")), ["K4"]);
        let models = Connection::Ok(vec!["a".into(), "b".into()]);
        assert_eq!(ids(&check_connection(&models, "c")), ["K3"]);
        assert!(check_connection(&models, "a").is_empty());
        assert!(check_connection(&models, "").is_empty());
        assert!(check_connection(&Connection::Ok(vec![]), "c").is_empty());
    }

    #[test]
    fn endpoint_mismatch_is_k6() {
        use crate::api::ModelInfo;
        let cat = vec![
            ModelInfo { id: "gpt-5.6-sol".into(), endpoints: vec!["chat.completions".into()] },
            ModelInfo { id: "gpt-6-sol".into(), endpoints: vec!["chat.completions".into(), "responses".into()] },
            ModelInfo { id: "claude-opus-5-5".into(), endpoints: vec!["messages".into()] },
            ModelInfo { id: "unknown".into(), endpoints: vec![] },
        ];
        assert_eq!(ids(&check_endpoint(Tool::Codex, "gpt-5.6-sol", &cat)), ["K6"]);
        assert!(check_endpoint(Tool::Codex, "gpt-6-sol", &cat).is_empty());
        assert_eq!(ids(&check_endpoint(Tool::Claude, "gpt-6-sol", &cat)), ["K6"]);
        assert!(check_endpoint(Tool::Claude, "claude-opus-5-5", &cat).is_empty());
        assert!(check_endpoint(Tool::Codex, "unknown", &cat).is_empty());
        assert!(check_endpoint(Tool::Codex, "not-listed", &cat).is_empty());
    }

}
