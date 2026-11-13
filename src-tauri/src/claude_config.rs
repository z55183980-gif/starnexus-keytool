//! Claude Code CLI 的配置写入。
//!
//! 参照 cc-switch 的做法写 `~/.claude/settings.json` 的 `env` 块，
//! 但避开了它两个已知 bug：
//!   - issue #1790 / #1104：只写 BASE_URL + AUTH_TOKEN，漏了 ANTHROPIC_MODEL
//!   - issue #2525：把 key 写进 apiKey 字段却留空 env.ANTHROPIC_AUTH_TOKEN，导致 401
//! 我们把三个键一次写齐，且只合并不覆盖。

use crate::error::{AppError, Result};
use serde::Serialize;
use serde_json::{Map, Value};

/// 我们管辖的 env 键，其余一律不碰
pub const KEY_BASE_URL: &str = "ANTHROPIC_BASE_URL";
pub const KEY_AUTH_TOKEN: &str = "ANTHROPIC_AUTH_TOKEN";
pub const KEY_MODEL: &str = "ANTHROPIC_MODEL";
/// 别名解析：用户在 Claude Code 里打 /model opus 时解析到哪个模型。
/// 不设的话会解析成 Anthropic 官方的模型 ID，网关没有就直接 404。
pub const KEY_DEFAULT_OPUS: &str = "ANTHROPIC_DEFAULT_OPUS_MODEL";
pub const KEY_DEFAULT_SONNET: &str = "ANTHROPIC_DEFAULT_SONNET_MODEL";
/// haiku 别名，同时也是后台杂事用的模型
pub const KEY_DEFAULT_HAIKU: &str = "ANTHROPIC_DEFAULT_HAIKU_MODEL";
/// 已废弃，官方文档标了 [DEPRECATED]，由 DEFAULT_HAIKU 取代。只清理不写入。
pub const KEY_DEPRECATED_SMALL_FAST: &str = "ANTHROPIC_SMALL_FAST_MODEL";
/// 走 x-api-key 头，给 Anthropic 官方 API 用。
/// 官方文档：和 AUTH_TOKEN 只能设一个，同时存在时谁生效取决于端点和版本。
pub const KEY_LEGACY_API_KEY: &str = "ANTHROPIC_API_KEY";

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum SettingsState {
    Missing,
    Ok,
    /// 存在但 JSON 解析不了 —— 绝不写入
    Broken(String),
}

#[derive(Debug, Clone)]
pub struct ClaudeSettings {
    pub base_url: String,
    pub auth_token: String,
    pub model: String,
    /// opus / sonnet / haiku 三个别名各自解析到的网关模型，留空则不写该项
    pub opus_model: String,
    pub sonnet_model: String,
    pub haiku_model: String,
}

pub fn inspect(text: Option<&str>) -> SettingsState {
    let Some(t) = text else {
        return SettingsState::Missing;
    };
    if t.trim().is_empty() {
        return SettingsState::Missing;
    }
    match serde_json::from_str::<Value>(t) {
        Ok(v) if v.is_object() => SettingsState::Ok,
        Ok(_) => SettingsState::Broken("settings.json 顶层不是对象".into()),
        Err(e) => SettingsState::Broken(e.to_string()),
    }
}

/// 合并式写入：只动 env 里我们管的几个键。
///
/// 用户的 permissions、hooks、statusLine、model 等一律原样保留 ——
/// 这些东西丢了比配置失败更糟，用户往往察觉不到。
pub fn merge(text: Option<&str>, s: &ClaudeSettings) -> Result<String> {
    let mut root: Map<String, Value> = match text {
        Some(t) if !t.trim().is_empty() => serde_json::from_str::<Value>(t)
            .map_err(|e| AppError::JsonParse(e.to_string()))?
            .as_object()
            .cloned()
            .ok_or_else(|| AppError::JsonParse("settings.json 顶层不是对象".into()))?,
        _ => Map::new(),
    };

    let mut env: Map<String, Value> = root
        .get("env")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    env.insert(KEY_BASE_URL.into(), Value::String(s.base_url.clone()));
    env.insert(KEY_AUTH_TOKEN.into(), Value::String(s.auth_token.clone()));
    // 模型留空（其他服务没选模型）就不写，由 Claude Code 用它自己的默认模型
    if s.model.trim().is_empty() {
        env.remove(KEY_MODEL);
    } else {
        env.insert(KEY_MODEL.into(), Value::String(s.model.clone()));
    }

    for (key, val) in [
        (KEY_DEFAULT_OPUS, &s.opus_model),
        (KEY_DEFAULT_SONNET, &s.sonnet_model),
        (KEY_DEFAULT_HAIKU, &s.haiku_model),
    ] {
        if val.trim().is_empty() {
            env.remove(key);
        } else {
            env.insert(key.into(), Value::String(val.clone()));
        }
    }

    // 废弃变量：留着不起作用，还会让人以为设置生效了
    env.remove(KEY_DEPRECATED_SMALL_FAST);
    // 和 AUTH_TOKEN 只能存在一个，否则谁生效不确定
    env.remove(KEY_LEGACY_API_KEY);

    root.insert("env".into(), Value::Object(env));

    serde_json::to_string_pretty(&Value::Object(root))
        .map_err(|e| AppError::JsonParse(e.to_string()))
}

/// 切回官方：只摘掉我们写的几个 env 键，其余一概不动
pub fn revert(text: Option<&str>) -> Result<Option<String>> {
    let Some(t) = text else { return Ok(None) };
    if t.trim().is_empty() {
        return Ok(None);
    }

    let mut root: Map<String, Value> = serde_json::from_str::<Value>(t)
        .map_err(|e| AppError::JsonParse(e.to_string()))?
        .as_object()
        .cloned()
        .ok_or_else(|| AppError::JsonParse("settings.json 顶层不是对象".into()))?;

    if let Some(env) = root.get_mut("env").and_then(|v| v.as_object_mut()) {
        for k in [
            KEY_BASE_URL,
            KEY_AUTH_TOKEN,
            KEY_MODEL,
            KEY_DEFAULT_OPUS,
            KEY_DEFAULT_SONNET,
            KEY_DEFAULT_HAIKU,
            KEY_DEPRECATED_SMALL_FAST,
        ] {
            env.remove(k);
        }
        let empty = env.is_empty();
        if empty {
            root.remove("env");
        }
    }

    Ok(Some(
        serde_json::to_string_pretty(&Value::Object(root))
            .map_err(|e| AppError::JsonParse(e.to_string()))?,
    ))
}

/// 读出当前指向，用于界面显示现状
pub fn current_base_url(text: &str) -> Option<String> {
    serde_json::from_str::<Value>(text)
        .ok()?
        .get("env")?
        .get(KEY_BASE_URL)?
        .as_str()
        .map(|s| s.to_string())
}

/// 读出当前生效的密钥，用来判断是哪一把
pub fn current_auth_token(text: &str) -> Option<String> {
    serde_json::from_str::<Value>(text)
        .ok()?
        .get("env")?
        .get(KEY_AUTH_TOKEN)?
        .as_str()
        .map(|s| s.to_string())
}

pub fn current_model(text: &str) -> Option<String> {
    serde_json::from_str::<Value>(text)
        .ok()?
        .get("env")?
        .get(KEY_MODEL)?
        .as_str()
        .map(|s| s.to_string())
}

/// Claude Code 自己拼 `/v1/messages`，所以 base_url 要的是网关根，
/// 而 Codex 自己拼 `/responses`，要的是带 `/v1` 的。
/// 同一个网关两个工具地址不同 —— 这个由代码推导，绝不让用户手填。
pub fn derive_anthropic_base_url(openai_base_url: &str) -> String {
    let trimmed = openai_base_url.trim_end_matches('/');
    trimmed
        .strip_suffix("/v1")
        .unwrap_or(trimmed)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s() -> ClaudeSettings {
        ClaudeSettings {
            base_url: "https://gateway.dkby.com".into(),
            auth_token: "sk-test".into(),
            model: "claude-opus-4-8".into(),
            opus_model: "claude-opus-4-8".into(),
            sonnet_model: "claude-sonnet-4-6".into(),
            haiku_model: "claude-haiku-4-5-20251001".into(),
        }
    }

    #[test]
    fn derives_base_url_without_v1() {
        // （顺带）模型留空时不写 ANTHROPIC_MODEL，旧值也清掉
        let mut empty = s();
        empty.model = String::new();
        let merged = merge(Some(r#"{"env":{"ANTHROPIC_MODEL":"old"}}"#), &empty).unwrap();
        assert!(!merged.contains("ANTHROPIC_MODEL"));
        assert_eq!(
            derive_anthropic_base_url("https://gateway.dkby.com/v1"),
            "https://gateway.dkby.com"
        );
        assert_eq!(
            derive_anthropic_base_url("https://gateway.dkby.com/v1/"),
            "https://gateway.dkby.com"
        );
        // 本来就没有 /v1 的原样返回
        assert_eq!(
            derive_anthropic_base_url("https://gateway.dkby.com"),
            "https://gateway.dkby.com"
        );
        // 不能误伤路径里别处的 v1
        assert_eq!(
            derive_anthropic_base_url("https://gateway.dkby.com/v1/gw/v1"),
            "https://gateway.dkby.com/v1/gw"
        );
    }

    #[test]
    fn writes_all_three_keys() {
        // cc-switch issue #1790/#1104：漏写 ANTHROPIC_MODEL。我们必须三个都写。
        let out = merge(None, &s()).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["env"][KEY_BASE_URL], "https://gateway.dkby.com");
        assert_eq!(v["env"][KEY_AUTH_TOKEN], "sk-test");
        assert_eq!(v["env"][KEY_MODEL], "claude-opus-4-8");
        assert_eq!(v["env"][KEY_DEFAULT_OPUS], "claude-opus-4-8");
        assert_eq!(v["env"][KEY_DEFAULT_SONNET], "claude-sonnet-4-6");
        assert_eq!(v["env"][KEY_DEFAULT_HAIKU], "claude-haiku-4-5-20251001");
    }

    #[test]
    fn preserves_user_settings() {
        let original = r#"{
          "permissions": {"allow": ["Bash(npm run test:*)"]},
          "hooks": {"Stop": [{"matcher": "", "hooks": []}]},
          "statusLine": {"type": "command", "command": "mystatus"},
          "env": {"MY_OWN_VAR": "keepme"}
        }"#;
        let out = merge(Some(original), &s()).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();

        assert!(v["permissions"]["allow"].is_array(), "permissions 必须保留");
        assert!(v["hooks"]["Stop"].is_array(), "hooks 必须保留");
        assert_eq!(v["statusLine"]["command"], "mystatus");
        assert_eq!(v["env"]["MY_OWN_VAR"], "keepme", "用户自己的环境变量必须保留");
        assert_eq!(v["env"][KEY_BASE_URL], "https://gateway.dkby.com");
    }

    #[test]
    fn clears_legacy_api_key() {
        // cc-switch issue #2525：两个 key 并存导致 401
        let original = r#"{"env":{"ANTHROPIC_API_KEY":"sk-old"}}"#;
        let out = merge(Some(original), &s()).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["env"].get(KEY_LEGACY_API_KEY).is_none());
        assert_eq!(v["env"][KEY_AUTH_TOKEN], "sk-test");
    }

    #[test]
    fn omits_alias_when_empty() {
        let mut cfg = s();
        cfg.haiku_model = String::new();
        let out = merge(None, &cfg).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["env"].get(KEY_DEFAULT_HAIKU).is_none());
    }

    /// 回归：ANTHROPIC_SMALL_FAST_MODEL 已被官方标记废弃，
    /// 写了也不生效，还会让用户误以为设置成功。任何情况下都要清掉。
    #[test]
    fn strips_deprecated_small_fast_model() {
        let original = r#"{"env":{"ANTHROPIC_SMALL_FAST_MODEL":"claude-3-5-haiku-20241022"}}"#;
        let out = merge(Some(original), &s()).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["env"].get(KEY_DEPRECATED_SMALL_FAST).is_none());
        assert_eq!(v["env"][KEY_DEFAULT_HAIKU], "claude-haiku-4-5-20251001");
    }

    /// 别名要指向网关自己的模型，否则用户打 /model opus 会解析到官方 ID 而 404
    #[test]
    fn writes_alias_models() {
        let out = merge(None, &s()).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["env"][KEY_DEFAULT_OPUS], "claude-opus-4-8");
        assert_eq!(v["env"][KEY_DEFAULT_SONNET], "claude-sonnet-4-6");
        assert_eq!(v["env"][KEY_DEFAULT_HAIKU], "claude-haiku-4-5-20251001");
    }

    #[test]
    fn merge_is_idempotent() {
        let once = merge(None, &s()).unwrap();
        let twice = merge(Some(&once), &s()).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn revert_keeps_user_env() {
        let configured = merge(Some(r#"{"env":{"MY_OWN_VAR":"keepme"}}"#), &s()).unwrap();
        let out = revert(Some(&configured)).unwrap().unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();

        assert!(v["env"].get(KEY_BASE_URL).is_none());
        assert!(v["env"].get(KEY_AUTH_TOKEN).is_none());
        assert_eq!(v["env"]["MY_OWN_VAR"], "keepme");
    }

    #[test]
    fn broken_json_detected() {
        assert!(matches!(
            inspect(Some("{not json")),
            SettingsState::Broken(_)
        ));
        assert_eq!(inspect(None), SettingsState::Missing);
        assert_eq!(inspect(Some(r#"{"a":1}"#)), SettingsState::Ok);
        assert!(merge(Some("{not json"), &s()).is_err());
    }
}
