use crate::error::{AppError, Result};
use serde_json::{Map, Value};

/// auth.json 的现状判断，决定要不要碰它
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum AuthState {
    /// 文件不存在
    Missing,
    /// 有官方 ChatGPT 登录 —— 默认绝不覆盖
    OfficialLogin,
    /// 只有 API Key，没有官方登录 —— 可以安全合并
    ApiKeyOnly,
    /// 存在但解析不了 —— 不碰
    Unparsable,
    /// Authentication is stored outside auth.json; do not infer account state.
    ExternalStore,
}

pub fn inspect(auth_text: Option<&str>) -> AuthState {
    let Some(text) = auth_text else {
        return AuthState::Missing;
    };
    if text.trim().is_empty() {
        return AuthState::Missing;
    }
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return AuthState::Unparsable;
    };
    let Some(obj) = v.as_object() else {
        return AuthState::Unparsable;
    };

    // tokens 存在，或 auth_mode 明确是 chatgpt —— 都算官方登录
    let has_tokens = obj.get("tokens").map(|t| !t.is_null()).unwrap_or(false);
    let chatgpt_mode = obj
        .get("auth_mode")
        .and_then(|m| m.as_str())
        .map(|m| m.eq_ignore_ascii_case("chatgpt"))
        .unwrap_or(false);

    if has_tokens || chatgpt_mode {
        AuthState::OfficialLogin
    } else if obj.get("auth_mode").is_some_and(|mode| {
        mode.as_str()
            .is_none_or(|s| !s.eq_ignore_ascii_case("apikey"))
    }) {
        AuthState::Unparsable
    } else if obj
        .get("OPENAI_API_KEY")
        .and_then(Value::as_str)
        .is_some_and(|k| !k.trim().is_empty())
    {
        AuthState::ApiKeyOnly
    } else {
        AuthState::Missing
    }
}

/// 合并式写入：只改 OPENAI_API_KEY 这一个键，其余原样保留。
///
/// 旧脚本是整文件覆盖，会把 auth_mode 等键一并抹掉 —— 这就是那个 bug 的修复。
pub fn merge_api_key(auth_text: Option<&str>, api_key: &str) -> Result<String> {
    let mut obj: Map<String, Value> = match auth_text {
        Some(t) if !t.trim().is_empty() => serde_json::from_str::<Value>(t)
            .map_err(|e| AppError::JsonParse(e.to_string()))?
            .as_object()
            .cloned()
            .ok_or_else(|| AppError::JsonParse("auth.json 顶层不是对象".into()))?,
        _ => Map::new(),
    };

    obj.insert(
        "OPENAI_API_KEY".to_string(),
        Value::String(api_key.to_string()),
    );

    // 文件本来就没有 auth_mode 时才补一个，绝不覆盖用户已有的值
    if !obj.contains_key("auth_mode") {
        obj.insert("auth_mode".to_string(), Value::String("apikey".into()));
    }

    serde_json::to_string_pretty(&Value::Object(obj))
        .map_err(|e| AppError::JsonParse(e.to_string()))
}

/// 读出 auth.json 里的 key，用来判断当前生效的是哪一把
pub fn current_api_key(auth_text: Option<&str>) -> Option<String> {
    serde_json::from_str::<Value>(auth_text?)
        .ok()?
        .get("OPENAI_API_KEY")?
        .as_str()
        .map(|s| s.to_string())
}

/// Explicitly authorized switch away from file-based ChatGPT authentication.
pub fn replace_with_api_key(auth_text: Option<&str>, api_key: &str) -> Result<String> {
    let merged = merge_api_key(auth_text, api_key)?;
    let mut value: Value = serde_json::from_str(&merged)?;
    let obj = value
        .as_object_mut()
        .expect("merge_api_key returns an object");
    obj.remove("tokens");
    obj.remove("last_refresh");
    obj.insert("auth_mode".into(), Value::String("apikey".into()));
    Ok(serde_json::to_string_pretty(&value)?)
}

/// 切回官方时，把我们写进去的第三方 key 清掉，其余键不动
pub fn strip_api_key(auth_text: Option<&str>) -> Result<Option<String>> {
    let Some(t) = auth_text else { return Ok(None) };
    if t.trim().is_empty() {
        return Ok(None);
    }
    let mut obj: Map<String, Value> = serde_json::from_str::<Value>(t)
        .map_err(|e| AppError::JsonParse(e.to_string()))?
        .as_object()
        .cloned()
        .ok_or_else(|| AppError::JsonParse("auth.json 顶层不是对象".into()))?;

    obj.remove("OPENAI_API_KEY");

    Ok(Some(
        serde_json::to_string_pretty(&Value::Object(obj))
            .map_err(|e| AppError::JsonParse(e.to_string()))?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_preserves_other_keys() {
        // 这正是用户机器上的真实形态
        let original = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-old"}"#;
        let out = merge_api_key(Some(original), "sk-new").unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["OPENAI_API_KEY"], "sk-new");
        assert_eq!(v["auth_mode"], "apikey", "auth_mode 必须保留");
    }

    #[test]
    fn merge_preserves_unknown_keys() {
        let original = r#"{"auth_mode":"apikey","last_refresh":"2026-01-01","custom":{"a":1}}"#;
        let out = merge_api_key(Some(original), "sk-new").unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["last_refresh"], "2026-01-01");
        assert_eq!(v["custom"]["a"], 1);
    }

    #[test]
    fn detects_official_login() {
        let with_tokens = r#"{"auth_mode":"chatgpt","tokens":{"access_token":"x"}}"#;
        assert_eq!(inspect(Some(with_tokens)), AuthState::OfficialLogin);

        let api_only = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-x"}"#;
        assert_eq!(inspect(Some(api_only)), AuthState::ApiKeyOnly);

        assert_eq!(inspect(None), AuthState::Missing);
        assert_eq!(inspect(Some("{not json")), AuthState::Unparsable);
        assert_eq!(inspect(Some("{}")), AuthState::Missing);
        assert_eq!(
            inspect(Some(r#"{"OPENAI_API_KEY":" "}"#)),
            AuthState::Missing
        );
    }

    #[test]
    fn explicit_api_switch_removes_official_tokens_but_preserves_other_metadata() {
        let original = r#"{"auth_mode":"chatgpt","tokens":{"access_token":"old"},"last_refresh":"yesterday","custom":42}"#;
        let text = replace_with_api_key(Some(original), "new").unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["auth_mode"], "apikey");
        assert_eq!(value["custom"], 42);
        assert!(value.get("tokens").is_none());
        assert!(value.get("last_refresh").is_none());
        assert_eq!(inspect(Some(&text)), AuthState::ApiKeyOnly);
    }
}
