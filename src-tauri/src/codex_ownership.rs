use crate::{codex_auth, codex_config, error::Result, state::key_hash};
use serde::{Deserialize, Serialize};
use toml_edit::value;

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
struct Field {
    original: Option<String>,
    written: Option<String>,
}

impl Field {
    fn update(&mut self, current: Option<String>, next: Option<String>, first: bool) {
        if first || self.written != current { self.original = current; }
        self.written = next;
    }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Ownership {
    model: Field,
    effort: Field,
    auth_hash: Option<String>,
    legacy_relay_hash: Option<String>,
}

impl Ownership {
    /// 读取归属记录。文件损坏时退化为「没有记录」并返回 `true`：
    /// 空记录不认领任何字段或密钥，只会更保守，不会因此删错东西；随后的写入会重建记录。
    pub fn read(text: Option<&str>) -> (Self, bool) {
        match text.map(serde_json::from_str::<Self>) {
            None => (Self::default(), false),
            Some(Ok(record)) => (record, false),
            Some(Err(_)) => (Self::default(), true),
        }
    }

    /// 按原始字节读取；不是 UTF-8 也按损坏处理
    pub fn from_bytes(raw: Option<&[u8]>) -> (Self, bool) {
        match raw.map(std::str::from_utf8) {
            None => (Self::default(), false),
            Some(Ok(text)) => Self::read(Some(text)),
            Some(Err(_)) => (Self::default(), true),
        }
    }

    pub fn record(&mut self, first: bool, before: &str, after: &str) {
        self.model.update(codex_config::current_model(before), codex_config::current_model(after), first);
        self.effort.update(codex_config::current_reasoning_effort(before), codex_config::current_reasoning_effort(after), first);
    }

    pub fn record_auth(&mut self, key: &str) { self.auth_hash = Some(key_hash(key)); }

    pub fn observe_route(&mut self, config: &str, auth: Option<&str>) {
        // 只在确实读到密钥时更新，避免把之前记下的历史网关密钥抹掉
        if let Some(key) = codex_config::has_builtin_relay(config).then(|| codex_auth::current_api_key(auth)).flatten() {
            self.legacy_relay_hash = Some(key_hash(&key));
        }
    }

    pub fn is_legacy_relay_key(&self, auth: Option<&str>) -> bool {
        codex_auth::current_api_key(auth).is_some_and(|key| self.legacy_relay_hash.as_ref() == Some(&key_hash(&key)))
    }

    pub fn owns_auth(&self, auth: Option<&str>) -> bool {
        codex_auth::current_api_key(auth).is_some_and(|key| self.auth_hash.as_ref() == Some(&key_hash(&key)))
    }

    pub fn restore_fields(&self, config: &str) -> Result<String> {
        let mut doc = codex_config::parse(config)?;
        for (name, field) in [("model", &self.model), ("model_reasoning_effort", &self.effort)] {
            if doc.get(name).is_some_and(|item| item.as_str().is_none()) { continue; }
            if doc.get(name).and_then(|item| item.as_str()) == field.written.as_deref() {
                match &field.original {
                    Some(original) => doc[name] = value(original),
                    None => { doc.as_table_mut().remove(name); }
                }
            }
        }
        Ok(doc.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_switch_preserves_original_and_later_manual_edits() {
        let mut record = Ownership::default();
        let first = "model='relay1'\nmodel_reasoning_effort='high'";
        let second = "model='relay2'\nmodel_reasoning_effort='high'";
        record.record(true, "model='official'", first);
        record.record(false, first, second);
        assert_eq!(codex_config::current_model(&record.restore_fields(second).unwrap()).as_deref(), Some("official"));
        assert_eq!(codex_config::current_reasoning_effort(&record.restore_fields(second).unwrap()), None);
        assert_eq!(codex_config::current_model(&record.restore_fields("model='manual'").unwrap()).as_deref(), Some("manual"));
        record.record_auth("relay");
        assert!(!record.owns_auth(Some(r#"{"OPENAI_API_KEY":"official"}"#)));
    }

    #[test]
    fn corrupt_journal_degrades_to_an_empty_record() {
        let (record, corrupt) = Ownership::read(Some("{not json"));
        assert!(corrupt);
        assert!(!record.owns_auth(Some(r#"{"OPENAI_API_KEY":"relay"}"#)));
        assert_eq!(codex_config::current_model(&record.restore_fields("model='relay'").unwrap()).as_deref(), Some("relay"));
        assert!(!Ownership::read(None).1);
        assert!(!Ownership::read(Some("{}")).1);
    }

    #[test]
    fn observing_a_relay_without_a_key_keeps_the_legacy_record() {
        let mut record = Ownership::default();
        let relay = "openai_base_url='https://relay.invalid/v1'";
        let legacy = Some(r#"{"auth_mode":"apikey","OPENAI_API_KEY":"legacy"}"#);
        record.observe_route(relay, legacy);
        record.observe_route(relay, Some(r#"{"tokens":{"id_token":"x"}}"#));
        record.observe_route(relay, None);
        assert!(record.is_legacy_relay_key(legacy));
    }

    #[test]
    fn migrating_a_legacy_relay_does_not_make_its_key_owned() {
        let mut record = Ownership::default();
        let old = Some(r#"{"auth_mode":"apikey","OPENAI_API_KEY":"legacy"}"#);
        record.observe_route("openai_base_url='https://relay.invalid/v1'", old);
        assert!(record.is_legacy_relay_key(old));
        assert!(!record.owns_auth(old));
        assert!(!record.is_legacy_relay_key(Some(r#"{"OPENAI_API_KEY":"official"}"#)));
        assert!(!serde_json::to_string(&record).unwrap().contains("\"legacy\""));
    }
}
