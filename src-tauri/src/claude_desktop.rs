//! Claude 桌面版的第三方推理（Gateway）配置。
//!
//! 桌面版不读 `~/.claude/settings.json`：它在自己的数据目录 `Claude-3p/configLibrary`
//! 里按 `_meta.json` 的 `appliedId` 读一份 JSON 配置（设置页「Configure third-party
//! inference」改的就是它）。
//!
//! - 密钥可以写明文：桌面版下次保存或启动时会自己加密成 `__sealed__:v1:…`（用户级 DPAPI，
//!   带它自己的参数，本工具解不开），加密不了时也照常使用明文。
//! - 运行中的桌面版不会重读这个文件，要完全退出后重新打开才生效。
//! - 只改我们管的几个键，用户在设置页里填的其他项一律保留。

use crate::claude_config::SettingsState;
use crate::error::{AppError, Result};
use serde_json::{Map, Value};
use std::path::PathBuf;

pub const SEALED_PREFIX: &str = "__sealed__:v1:";
pub const KEY_PROVIDER: &str = "inferenceProvider";
pub const KEY_BASE_URL: &str = "inferenceGatewayBaseUrl";
pub const KEY_API_KEY: &str = "inferenceGatewayApiKey";
pub const KEY_CREDENTIAL_KIND: &str = "inferenceCredentialKind";
pub const KEY_MODELS: &str = "inferenceModels";
/// 我们写的键；切回官方时只删这些
const OWNED: [&str; 5] = [
    KEY_PROVIDER,
    KEY_BASE_URL,
    KEY_API_KEY,
    KEY_CREDENTIAL_KIND,
    KEY_MODELS,
];
/// 静态 Key 之外的凭据来源；留着会和我们写的 Key 冲突
const OTHER_CREDENTIALS: [&str; 2] = ["inferenceCredentialHelper", "inferenceCredentialHelperArgs"];

pub fn data_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_USER_DATA_DIR").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    #[cfg(windows)]
    if let Some(local) = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(local).join("Claude-3p"));
    }
    let home = dirs::home_dir().ok_or_else(|| AppError::Other("无法确定用户主目录".into()))?;
    #[cfg(target_os = "macos")]
    return Ok(home.join("Library/Application Support/Claude-3p"));
    #[cfg(not(target_os = "macos"))]
    Ok(home.join(".config/Claude-3p"))
}

pub fn library_dir() -> Result<PathBuf> {
    Ok(data_dir()?.join("configLibrary"))
}

pub fn meta_path() -> Result<PathBuf> {
    Ok(library_dir()?.join("_meta.json"))
}

fn valid_id(id: &str) -> bool {
    id.len() == 36
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || b == b'-')
}

/// `_meta.json` 当前选中的那份配置
#[derive(Debug, PartialEq)]
pub enum Applied {
    /// 配置库还没建（桌面版没进过第三方推理设置）
    None,
    Config(PathBuf),
    /// 有配置库，但不是一份本地配置在生效（损坏、指向别处、由组织下发等）
    Unusable(String),
}

pub fn parse_meta(text: Option<&str>) -> Applied {
    let Some(text) = text.filter(|t| !t.trim().is_empty()) else {
        return Applied::None;
    };
    let Ok(meta) = serde_json::from_str::<Value>(text) else {
        return Applied::Unusable("Claude 桌面版的配置库索引（_meta.json）格式损坏".into());
    };
    if meta.get("hybridPointer").is_some_and(|v| !v.is_null()) {
        return Applied::Unusable("Claude 桌面版当前使用组织下发的配置，本工具不改它".into());
    }
    let id = meta.get("appliedId").and_then(Value::as_str).unwrap_or("");
    let listed = meta
        .get("entries")
        .and_then(Value::as_array)
        .is_some_and(|e| {
            e.iter()
                .any(|x| x.get("id").and_then(Value::as_str) == Some(id))
        });
    if !valid_id(id) || !listed {
        return Applied::Unusable(
            "Claude 桌面版没有选中任何一份本地配置，请先在它的第三方推理设置里保存一次".into(),
        );
    }
    match library_dir() {
        Ok(dir) => Applied::Config(dir.join(format!("{id}.json"))),
        Err(e) => Applied::Unusable(e.to_string()),
    }
}

pub fn applied() -> Result<Applied> {
    Ok(parse_meta(
        crate::atomic::read_text_optional(&meta_path()?)?.as_deref(),
    ))
}

fn new_id() -> Result<String> {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).map_err(|e| AppError::Other(format!("生成配置编号失败：{e}")))?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    ))
}

/// 写入目标：已有选中的配置就写它；配置库还没建就新建一份 Default 并选中（和桌面版自己建的一样）
pub fn target_for_write() -> Result<(PathBuf, Option<String>)> {
    match applied()? {
        Applied::Config(path) => Ok((path, None)),
        Applied::Unusable(reason) => Err(AppError::Other(reason)),
        Applied::None => {
            let id = new_id()?;
            let meta = serde_json::json!({ "appliedId": id, "entries": [{ "id": id, "name": "Default" }] });
            Ok((
                library_dir()?.join(format!("{id}.json")),
                Some(serde_json::to_string_pretty(&meta)?),
            ))
        }
    }
}

pub fn is_sealed(value: &str) -> bool {
    value.trim_start().starts_with(SEALED_PREFIX)
}

fn parse_object(text: Option<&str>) -> std::result::Result<Map<String, Value>, String> {
    match text.filter(|t| !t.trim().is_empty()) {
        None => Ok(Map::new()),
        Some(t) => match serde_json::from_str::<Value>(t) {
            Ok(Value::Object(m)) => Ok(m),
            Ok(_) => Err("顶层不是 JSON 对象".into()),
            Err(e) => Err(e.to_string()),
        },
    }
}

pub fn inspect(text: Option<&str>) -> SettingsState {
    match text {
        None => SettingsState::Missing,
        Some(_) => match parse_object(text) {
            Ok(_) => SettingsState::Ok,
            Err(e) => SettingsState::Broken(e),
        },
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Current {
    pub provider: Option<String>,
    pub base_url: Option<String>,
    /// 原样：可能是明文，也可能是 `__sealed__:v1:` 密文
    pub api_key: Option<String>,
    pub credential_kind: Option<String>,
    pub other_credentials: bool,
    pub models: Vec<String>,
}

fn string(m: &Map<String, Value>, key: &str) -> Option<String> {
    m.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub fn current(text: &str) -> Option<Current> {
    let m = parse_object(Some(text)).ok()?;
    // 设置页存成数组；MDM 等渠道可能是 JSON 字符串，两种都认
    let models_value = match m.get(KEY_MODELS) {
        Some(Value::String(s)) => serde_json::from_str::<Value>(s).ok(),
        other => other.cloned(),
    };
    let models = models_value
        .as_ref()
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|x| match x {
                    Value::String(s) => Some(s.clone()),
                    Value::Object(o) => o.get("name").and_then(Value::as_str).map(str::to_string),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Current {
        provider: string(&m, KEY_PROVIDER),
        base_url: string(&m, KEY_BASE_URL),
        api_key: string(&m, KEY_API_KEY),
        credential_kind: string(&m, KEY_CREDENTIAL_KIND),
        other_credentials: OTHER_CREDENTIALS
            .iter()
            .any(|k| m.get(*k).is_some_and(|v| !v.is_null())),
        models,
    })
}

/// 去重、去空，保持顺序；第一个是桌面版的默认模型
pub fn model_list(models: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in models.iter().map(|m| m.trim()).filter(|m| !m.is_empty()) {
        if !out.iter().any(|x| x == m) {
            out.push(m.to_string());
        }
    }
    out
}

pub fn merge(
    text: Option<&str>,
    base_url: &str,
    api_key: &str,
    models: &[String],
) -> Result<String> {
    let mut m = parse_object(text).map_err(|e| {
        AppError::JsonParse(format!(
            "Claude 桌面版的配置文件有格式问题：{e}。原文件没有被改动"
        ))
    })?;
    m.insert(KEY_PROVIDER.into(), Value::from("gateway"));
    m.insert(
        KEY_BASE_URL.into(),
        Value::from(base_url.trim_end_matches('/')),
    );
    m.insert(KEY_API_KEY.into(), Value::from(api_key.trim()));
    m.insert(KEY_CREDENTIAL_KIND.into(), Value::from("static"));
    for k in OTHER_CREDENTIALS {
        m.remove(k);
    }
    // 没指定模型 = 交给桌面版按网关的 /v1/models 自动发现
    if models.is_empty() {
        m.remove(KEY_MODELS);
    } else {
        m.insert(KEY_MODELS.into(), Value::from(models.to_vec()));
    }
    Ok(serde_json::to_string_pretty(&Value::Object(m))?)
}

/// 切回官方：删掉第三方推理的几项；没有可删的返回 None
pub fn revert(text: Option<&str>) -> Result<Option<String>> {
    let Some(text) = text else { return Ok(None) };
    let mut m = parse_object(Some(text))
        .map_err(|e| AppError::JsonParse(format!("Claude 桌面版的配置文件有格式问题：{e}")))?;
    let before = m.len();
    for k in OWNED.iter().chain(OTHER_CREDENTIALS.iter()) {
        m.remove(*k);
    }
    if m.len() == before {
        return Ok(None);
    }
    Ok(Some(serde_json::to_string_pretty(&Value::Object(m))?))
}

/// 桌面版主程序是否在运行（只认桌面版的安装位置，不把 Claude Code CLI 的 claude.exe 算进去）
pub fn running() -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let script = "@(Get-Process -Name claude -ErrorAction SilentlyContinue | Where-Object { $_.Path -match '\\\\WindowsApps\\\\Claude_|\\\\AnthropicClaude\\\\|\\\\Programs\\\\Claude\\\\' }).Count";
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let ps = PathBuf::from(root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
        return std::process::Command::new(ps)
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .creation_flags(0x08000000)
            .output()
            .ok()
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .parse::<u32>()
                    .ok()
            })
            .is_some_and(|n| n > 0);
    }
    #[cfg(target_os = "macos")]
    {
        return std::process::Command::new("/usr/bin/pgrep")
            .args(["-f", "/Claude.app/Contents/MacOS/Claude"])
            .output()
            .is_ok_and(|o| o.status.success());
    }
    #[allow(unreachable_code)]
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER: &str = r#"{
  "inferenceGatewayBaseUrl": "https://old.example",
  "inferenceGatewayApiKey": "__sealed__:v1:AAAA",
  "builtinBrowserEnabled": true,
  "inferenceProvider": "gateway",
  "inferenceCredentialKind": "static",
  "inferenceCredentialHelper": "C:\\helper.exe"
}"#;

    #[test]
    fn merge_writes_gateway_keeps_user_settings_and_drops_conflicting_credentials() {
        let models = model_list(&[
            "claude-opus-5-5",
            "claude-opus-5-5",
            "",
            "claude-sonnet-5-5",
        ]);
        let out = merge(Some(USER), "https://gateway.dkby.com/", "sk-new", &models).unwrap();
        let c = current(&out).unwrap();
        assert_eq!(c.provider.as_deref(), Some("gateway"));
        assert_eq!(c.base_url.as_deref(), Some("https://gateway.dkby.com"));
        assert_eq!(c.api_key.as_deref(), Some("sk-new"));
        assert_eq!(c.credential_kind.as_deref(), Some("static"));
        assert!(!c.other_credentials);
        assert_eq!(c.models, vec!["claude-opus-5-5", "claude-sonnet-5-5"]);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["builtinBrowserEnabled"], Value::Bool(true));
    }

    #[test]
    fn empty_model_list_leaves_discovery_on() {
        let with = merge(None, "https://x", "k", &model_list(&["m"])).unwrap();
        let without = merge(Some(&with), "https://x", "k", &[]).unwrap();
        assert!(current(&without).unwrap().models.is_empty());
        assert!(!without.contains(KEY_MODELS));
    }

    #[test]
    fn reads_sealed_keys_and_string_encoded_model_lists() {
        let c = current(r#"{"inferenceGatewayApiKey":"__sealed__:v1:Zm9v","inferenceModels":"[\"a\",{\"name\":\"b\"}]"}"#).unwrap();
        assert!(is_sealed(c.api_key.as_deref().unwrap()));
        assert_eq!(c.models, vec!["a", "b"]);
    }

    #[test]
    fn broken_file_is_never_overwritten() {
        assert!(matches!(inspect(Some("{oops")), SettingsState::Broken(_)));
        assert!(merge(Some("{oops"), "https://x", "k", &[]).is_err());
        assert!(merge(Some("[1]"), "https://x", "k", &[]).is_err());
    }

    #[test]
    fn revert_removes_only_inference_keys() {
        let out = revert(Some(USER)).unwrap().unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v, serde_json::json!({ "builtinBrowserEnabled": true }));
        assert_eq!(revert(Some(&out)).unwrap(), None);
        assert_eq!(revert(None).unwrap(), None);
    }

    #[test]
    fn meta_must_select_a_listed_local_config() {
        let id = "8eb03636-36f8-4d79-9b9a-3fe9f40135df";
        let ok = format!(r#"{{"appliedId":"{id}","entries":[{{"id":"{id}","name":"Default"}}]}}"#);
        assert!(
            matches!(parse_meta(Some(&ok)), Applied::Config(p) if p.ends_with(format!("{id}.json")))
        );
        assert_eq!(parse_meta(None), Applied::None);
        assert!(matches!(
            parse_meta(Some(r#"{"appliedId":"../x","entries":[]}"#)),
            Applied::Unusable(_)
        ));
        assert!(matches!(
            parse_meta(Some(r#"{"hybridPointer":{"a":1}}"#)),
            Applied::Unusable(_)
        ));
        assert!(matches!(parse_meta(Some("{")), Applied::Unusable(_)));
        assert!(valid_id(&new_id().unwrap()));
    }
}
