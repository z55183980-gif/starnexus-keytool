use crate::error::{AppError, Result};
use crate::paths;
use serde::{Deserialize, Serialize};
use std::fs;
use std::sync::Mutex;

static STATE_LOCK: Mutex<()> = Mutex::new(());

const KEYRING_SERVICE: &str = "com.dkby.starnexus.keytool";
/// 旧版只存一把 key 时用的条目名，用于一次性迁移
const LEGACY_KEYRING_USER: &str = "api_key";

/// 一把密钥的配置。用户可能有多把 Key（主号/测试号、不同套餐），
/// 每把各自记住自己的模型偏好 —— 因为不同套餐能用的模型本来就不一样。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub codex_model: String,
    #[serde(default)]
    pub codex_effort: String,
    #[serde(default)]
    pub claude_model: String,
    /// 这把 Key 来自网关的哪个令牌。
    /// 有了它，登录导入时才能认出"这把已经导过"，从而变成更新而不是重复新建。
    #[serde(default)]
    pub gateway_token_id: Option<i64>,
    #[serde(default)]
    pub gateway_account: Option<String>,
    /// 其他服务（不是 AI）的地址。两个都空 = AI 密钥，走 AI 网关。
    /// 填了哪个，这把密钥就能给哪个工具用：Codex 用 OpenAI 兼容地址（通常以 /v1 结尾），
    /// Claude Code 用 Anthropic 兼容地址。
    #[serde(default)]
    pub codex_base_url: String,
    #[serde(default)]
    pub claude_base_url: String,
}

impl Profile {
    /// 其他服务的密钥（自己填了地址），不是 AI 的
    pub fn is_custom(&self) -> bool {
        !self.codex_base_url.trim().is_empty() || !self.claude_base_url.trim().is_empty()
    }

    /// 给 Codex 用的 OpenAI 兼容地址。AI 密钥用网关地址；其他服务没填就是不支持 Codex（None）
    pub fn openai_base(&self, gateway: &str) -> Option<String> {
        let url = if self.is_custom() { self.codex_base_url.trim() } else { gateway.trim() };
        (!url.is_empty()).then(|| url.trim_end_matches('/').to_string())
    }

    /// 给 Claude Code 用的 Anthropic 兼容地址。AI 密钥由网关地址推出；其他服务没填就是不支持 Claude（None）
    pub fn anthropic_base(&self, gateway: &str) -> Option<String> {
        if self.is_custom() {
            let url = self.claude_base_url.trim();
            return (!url.is_empty()).then(|| url.trim_end_matches('/').to_string());
        }
        Some(crate::claude_config::derive_anthropic_base_url(gateway))
    }
}

/// 按 id 找一把密钥的配置（不含 Key 本身）
pub fn find_profile(id: &str) -> Result<Option<Profile>> {
    Ok(load()?.profiles.into_iter().find(|p| p.id == id))
}

/// 其他服务填过的地址（Codex 的、Claude 的）。检测配置时，指向这些地址的也算「可能是本工具写的」，
/// 再按密钥核对是哪一把。
pub fn custom_base_urls() -> Result<(Vec<String>, Vec<String>)> {
    let profiles = load()?.profiles;
    let pick = |f: fn(&Profile) -> &String| -> Vec<String> {
        profiles.iter().map(f).map(|u| u.trim().trim_end_matches('/').to_string()).filter(|u| !u.is_empty()).collect()
    };
    Ok((pick(|p| &p.codex_base_url), pick(|p| &p.claude_base_url)))
}

/// 本工具自己的状态。注意：API Key 不在这里 —— 每把 key 各自进系统密钥库。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    #[serde(default)]
    pub legacy_migrated: bool,
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub ignore_proxy: bool,
    #[serde(default)]
    pub cleanup_residue: bool,
    #[serde(default)]
    pub last_configured_at: String,
    /// 旧版的「同意读取」：一次同意两个工具。新版按工具记在 config_read_tools，这个只为兼容读取
    #[serde(default)]
    pub config_read_consent: bool,
    /// 用户同意读取配置的工具（claude / codex）。没同意的工具不扫描它的配置文件
    #[serde(default)]
    pub config_read_tools: Vec<String>,
    /// 界面语言（zh-CN / zh-HK / en）；空 = 跟随系统
    #[serde(default)]
    pub ui_lang: String,
    /// 最近一次写进 Claude 桌面版的是哪把密钥。桌面版会把明文密钥加密保存，
    /// 本工具解不开，只能靠这条记录认出「这段密文就是我们写的那把」。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_desktop_key: Option<DesktopKeyRecord>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopKeyRecord {
    pub profile_id: String,
    /// 写入时那把密钥的 SHA-256；密钥后来被换掉就不再算匹配
    pub key_hash: String,
    /// 第一次看到的密文；之后密文变了说明有人在桌面版里改过密钥
    #[serde(default)]
    pub sealed: Option<String>,
}

pub fn key_hash(key: &str) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(key.trim().as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn remember_desktop_key(profile_id: &str, key: &str) -> Result<()> {
    let record = DesktopKeyRecord {
        profile_id: profile_id.into(),
        key_hash: key_hash(key),
        sealed: None,
    };
    update(|st| st.claude_desktop_key = Some(record))
}

pub fn forget_desktop_key() -> Result<()> {
    update(|st| st.claude_desktop_key = None)
}

/// 桌面版里的密文是不是我们最近写的那把：是就返回密钥编号。
/// 写入后第一次见到的密文认作那把密钥加密后的样子，并记下来。
pub fn match_sealed_desktop_key(sealed: &str) -> Result<Option<String>> {
    let Some(record) = load()?.claude_desktop_key else {
        return Ok(None);
    };
    let Some(key) = load_profile_key(&record.profile_id)? else {
        return Ok(None);
    };
    if key_hash(&key) != record.key_hash {
        return Ok(None);
    }
    match record.sealed.as_deref() {
        Some(s) if s == sealed => Ok(Some(record.profile_id)),
        Some(_) => Ok(None),
        None => {
            let sealed = sealed.to_string();
            update(|st| {
                if let Some(r) = st.claude_desktop_key.as_mut() {
                    r.sealed = Some(sealed);
                }
            })?;
            Ok(Some(record.profile_id))
        }
    }
}

pub fn load() -> Result<AppState> {
    let _guard = STATE_LOCK
        .lock()
        .map_err(|_| AppError::Other("密钥状态锁不可用，请重启".into()))?;
    load_inner()
}

fn load_inner() -> Result<AppState> {
    let mut st = read_state(&paths::state_path()?)?;
    if st.profiles.is_empty() && !st.legacy_migrated {
        if let Some(legacy) = read_credential(LEGACY_KEYRING_USER)? {
            let p = Profile {
                id: new_id(),
                name: "我的密钥".into(),
                codex_model: String::new(),
                codex_effort: String::new(),
                claude_model: String::new(),
                gateway_token_id: None,
                gateway_account: None,
                ..Default::default()
            };
            st.profiles.push(p.clone());
            st.legacy_migrated = true;
            change_key_and_save(&SystemKeys, &p.id, Some(&legacy), || save(&st))?;
            // Never remove the only legacy copy before both new stores are durable.
            delete_credential(LEGACY_KEYRING_USER)?;
        }
    }
    Ok(st)
}

fn read_state(path: &std::path::Path) -> Result<AppState> {
    match crate::atomic::read_text_optional(path)? {
        None => Ok(AppState::default()),
        Some(text) => serde_json::from_str(&text).map_err(|_| {
            AppError::Other("本地密钥列表文件损坏，已停止修改；请先恢复 state.json".into())
        }),
    }
}

pub fn save(s: &AppState) -> Result<()> {
    let dir = paths::app_dir()?;
    fs::create_dir_all(&dir)?;
    crate::atomic::write_atomic(&paths::state_path()?, &serde_json::to_string_pretty(s)?)
}

pub fn update(f: impl FnOnce(&mut AppState)) -> Result<()> {
    let _guard = STATE_LOCK
        .lock()
        .map_err(|_| AppError::Other("密钥状态锁不可用，请重启".into()))?;
    let mut st = load_inner()?;
    f(&mut st);
    save(&st)
}

pub fn new_id() -> String {
    format!("p{}", crate::atomic::unique_id())
}

fn keyring_user(profile_id: &str) -> String {
    format!("profile-{}", profile_id)
}

fn read_credential(user: &str) -> Result<Option<String>> {
    match keyring::Entry::new(KEYRING_SERVICE, user)?.get_password() {
        Ok(k) => Ok(Some(k).filter(|k| !k.trim().is_empty())),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn delete_credential(user: &str) -> Result<()> {
    match keyring::Entry::new(KEYRING_SERVICE, user)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Full keys stay in the backend. Missing credentials and unavailable storage differ.
pub fn load_profile_key(profile_id: &str) -> Result<Option<String>> {
    read_credential(&keyring_user(profile_id))
}

trait KeyStore {
    fn get(&self, id: &str) -> Result<Option<String>>;
    fn set(&self, id: &str, key: Option<&str>) -> Result<()>;
}
struct SystemKeys;
impl KeyStore for SystemKeys {
    fn get(&self, id: &str) -> Result<Option<String>> {
        load_profile_key(id)
    }
    fn set(&self, id: &str, key: Option<&str>) -> Result<()> {
        match key {
            Some(k) => keyring::Entry::new(KEYRING_SERVICE, &keyring_user(id))?
                .set_password(k)
                .map_err(Into::into),
            None => delete_credential(&keyring_user(id)),
        }
    }
}

/// Compensate a credential change if its metadata cannot be saved. Never report
/// success when the keyring write or read-back verification failed.
fn change_key_and_save(
    store: &impl KeyStore,
    id: &str,
    next: Option<&str>,
    save: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let previous = store.get(id)?;
    let result = (|| {
        store.set(id, next)?;
        if store.get(id)?.as_deref() != next {
            return Err(AppError::Keyring("密钥保存后校验失败".into()));
        }
        save()
    })();
    if let Err(error) = result {
        if store.set(id, previous.as_deref()).is_err()
            || store.get(id).map(|v| v != previous).unwrap_or(true)
        {
            return Err(AppError::Keyring(format!(
                "{}；原密钥恢复失败，请重新导入该密钥",
                error
            )));
        }
        return Err(error);
    }
    Ok(())
}

/// 取一个没被占用的默认名。
/// 不能用「个数+1」：删掉中间某个后再新建会和现存的重名。
pub fn next_default_name(profiles: &[Profile]) -> String {
    for n in 1.. {
        let candidate = format!("{} {}", crate::ui_lang::pick("密钥", "密鑰", "Key"), n);
        if !profiles.iter().any(|p| p.name == candidate) {
            return candidate;
        }
    }
    unreachable!()
}

/// 新增或更新一条。key 传 None 表示不改动已存的 key。
pub fn upsert_profile(mut p: Profile, key: Option<&str>) -> Result<Profile> {
    let _guard = STATE_LOCK
        .lock()
        .map_err(|_| AppError::Other("密钥状态锁不可用，请重启".into()))?;
    let mut st = load_inner()?;

    let is_new = p.id.trim().is_empty();
    if is_new {
        p.id = new_id();
    } else if !st.profiles.iter().any(|old| old.id == p.id) {
        return Err(AppError::Other("找不到这把密钥，请刷新后重试".into()));
    }

    if p.name.trim().is_empty() {
        p.name = if is_new {
            next_default_name(&st.profiles)
        } else {
            // 改已有条目时把名字清空了，多半不是想改名 —— 保留原来的
            st.profiles
                .iter()
                .find(|x| x.id == p.id)
                .map(|x| x.name.clone())
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| next_default_name(&st.profiles))
        };
    }

    // 手动改过 Key 就不再算作来自某个网关令牌，避免之后误判成"已关联"
    if p.gateway_token_id.is_none() {
        if let Some(old) = st.profiles.iter().find(|x| x.id == p.id) {
            if key.is_none() {
                p.gateway_token_id = old.gateway_token_id;
                p.gateway_account = old.gateway_account.clone();
            }
        }
    }

    match st.profiles.iter_mut().find(|x| x.id == p.id) {
        Some(existing) => *existing = p.clone(),
        None => st.profiles.push(p.clone()),
    }
    match key.map(str::trim).filter(|k| !k.is_empty()) {
        Some(k) => change_key_and_save(&SystemKeys, &p.id, Some(k), || save(&st))?,
        None => save(&st)?,
    }
    Ok(p)
}

pub fn delete_profile(id: &str) -> Result<()> {
    let _guard = STATE_LOCK
        .lock()
        .map_err(|_| AppError::Other("密钥状态锁不可用，请重启".into()))?;
    let mut st = load_inner()?;
    let before = st.profiles.len();
    st.profiles.retain(|x| x.id != id);
    if st.profiles.len() == before {
        return Err(AppError::Other("找不到这把密钥".into()));
    }
    change_key_and_save(&SystemKeys, id, None, || save(&st))
}

/// 按网关令牌 id 查找，用于判断某把 Key 是否已经导入过
pub fn find_by_token_id(token_id: i64, account: &str) -> Result<Option<Profile>> {
    Ok(load()?.profiles.into_iter().find(|p| {
        p.gateway_token_id == Some(token_id) && p.gateway_account.as_deref() == Some(account)
    }))
}

/// 找出当前生效的是哪一把 —— 拿实际配置里的 key 和各条的 key 逐一比对。
/// 比对在 Rust 侧做，密钥不出后端。
pub fn match_profile(live_key: Option<&str>) -> Result<Option<String>> {
    let Some(live) = live_key.map(str::trim).filter(|k| !k.is_empty()) else {
        return Ok(None);
    };
    for p in load()?.profiles {
        if load_profile_key(&p.id)?.as_deref() == Some(live) {
            return Ok(Some(p.id));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zai_keys_use_the_gateway_and_custom_keys_their_own_urls() {
        let gateway = "https://gateway.dkby.com/v1";
        let ai = Profile::default();
        assert!(!ai.is_custom());
        assert_eq!(ai.openai_base(gateway).as_deref(), Some(gateway));
        assert_eq!(ai.anthropic_base(gateway).as_deref(), Some("https://gateway.dkby.com"));

        let only_claude = Profile { claude_base_url: "https://relay.example/".into(), ..Default::default() };
        assert!(only_claude.is_custom());
        assert_eq!(only_claude.anthropic_base(gateway).as_deref(), Some("https://relay.example"));
        // 没填 Codex 地址就是不给 Codex 用，绝不退回 AI 网关（那会把别家的 Key 发给我们）
        assert_eq!(only_claude.openai_base(gateway), None);

        let only_codex = Profile { codex_base_url: "https://api.openai.com/v1".into(), ..Default::default() };
        assert_eq!(only_codex.openai_base(gateway).as_deref(), Some("https://api.openai.com/v1"));
        assert_eq!(only_codex.anthropic_base(gateway), None);
    }
    use std::cell::RefCell;

    struct FakeKeys {
        value: RefCell<Option<String>>,
        fail_new: bool,
        discard_new: bool,
    }
    impl KeyStore for FakeKeys {
        fn get(&self, _: &str) -> Result<Option<String>> {
            Ok(self.value.borrow().clone())
        }
        fn set(&self, _: &str, key: Option<&str>) -> Result<()> {
            if key == Some("new") && self.fail_new {
                return Err(AppError::Keyring("locked".into()));
            }
            if key != Some("new") || !self.discard_new {
                *self.value.borrow_mut() = key.map(str::to_owned);
            }
            Ok(())
        }
    }
    fn keys(old: Option<&str>) -> FakeKeys {
        FakeKeys {
            value: RefCell::new(old.map(str::to_owned)),
            fail_new: false,
            discard_new: false,
        }
    }
    #[test]
    fn failed_keyring_write_never_persists_metadata() {
        let mut store = keys(Some("old"));
        store.fail_new = true;
        assert!(change_key_and_save(&store, "id", Some("new"), || panic!(
            "must not save metadata"
        ))
        .is_err());
        assert_eq!(store.get("id").unwrap().as_deref(), Some("old"));
    }
    #[test]
    fn keyring_readback_failure_never_reports_success() {
        let mut store = keys(Some("old"));
        store.discard_new = true;
        assert!(
            change_key_and_save(&store, "id", Some("new"), || panic!("must verify first")).is_err()
        );
    }
    #[test]
    fn failed_metadata_save_restores_previous_key_or_absence() {
        for old in [None, Some("old")] {
            let store = keys(old);
            assert!(
                change_key_and_save(&store, "id", Some("new"), || Err(AppError::Io(
                    "disk full".into()
                )))
                .is_err()
            );
            assert_eq!(store.get("id").unwrap().as_deref(), old);
        }
        let store = keys(Some("old"));
        assert!(
            change_key_and_save(&store, "id", None, || Err(AppError::Io("disk full".into())))
                .is_err()
        );
        assert_eq!(store.get("id").unwrap().as_deref(), Some("old"));
    }
    #[test]
    fn broken_metadata_is_not_treated_as_empty() {
        let dir = crate::atomic::tests::TestDir::new();
        let path = dir.0.join("state.json");
        fs::write(&path, "{broken").unwrap();
        assert!(read_state(&path).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "{broken");
    }
    #[test]
    fn rapid_profile_ids_do_not_collide() {
        let ids: std::collections::HashSet<_> = (0..1000).map(|_| new_id()).collect();
        assert_eq!(ids.len(), 1000);
    }

    fn p(name: &str) -> Profile {
        Profile {
            id: name.into(),
            name: name.into(),
            codex_model: String::new(),
            codex_effort: String::new(),
            claude_model: String::new(),
            gateway_token_id: None,
            gateway_account: None,
            ..Default::default()
        }
    }

    #[test]
    fn default_name_skips_taken_numbers() {
        let prefix = crate::ui_lang::pick("密钥", "密鑰", "Key");
        let list = vec![p(&format!("{prefix} 1")), p(&format!("{prefix} 3"))];
        assert_eq!(next_default_name(&list), format!("{prefix} 2"));
    }

    /// 回归：曾经用「个数+1」取名，删掉中间一个后新建会重名
    #[test]
    fn default_name_avoids_collision_after_delete() {
        // 原本有默认名 1 / 2，删掉 1 后只剩一个
        let prefix = crate::ui_lang::pick("密钥", "密鑰", "Key");
        let list = vec![p(&format!("{prefix} 2"))];
        assert_eq!(next_default_name(&list), format!("{prefix} 1"));
    }

    #[test]
    fn default_name_on_empty_list() {
        let prefix = crate::ui_lang::pick("密钥", "密鑰", "Key");
        assert_eq!(next_default_name(&[]), format!("{prefix} 1"));
    }

    #[test]
    fn custom_names_do_not_block_numbering() {
        let list = vec![p("主号"), p("测试号")];
        let prefix = crate::ui_lang::pick("密钥", "密鑰", "Key");
        assert_eq!(next_default_name(&list), format!("{prefix} 1"));
    }
}

/// 界面上只显示掩码，永远不回显完整 key
pub fn mask(key: &str) -> String {
    let n = key.chars().count();
    if n <= 8 {
        return "•".repeat(n.max(4));
    }
    let head: String = key.chars().take(3).collect();
    let tail: String = key.chars().skip(n - 4).collect();
    format!("{}{}{}", head, "•".repeat(8), tail)
}
