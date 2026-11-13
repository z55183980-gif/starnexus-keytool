//! StarNexus account integration. Cookies stay in backend memory; secrets never enter token views.
use crate::{brand, error::{AppError, Result}};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct Session {
    client: reqwest::Client,
    identity: Mutex<Option<Identity>>,
    requests: tokio::sync::Mutex<()>,
}
#[derive(Clone, Deserialize)]
struct Identity { id: i64, username: String }
impl Session {
    pub fn new(ignore_proxy: bool) -> Result<Self> {
        let mut builder = reqwest::Client::builder().cookie_store(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(25)).connect_timeout(Duration::from_secs(10))
            .user_agent(concat!("StarNexus-Key-Tool/", env!("CARGO_PKG_VERSION")));
        if ignore_proxy { builder = builder.no_proxy(); }
        Ok(Self { client: builder.build().map_err(network)?, identity: Mutex::new(None), requests: tokio::sync::Mutex::new(()) })
    }
    pub fn current_user(&self) -> Option<String> { self.identity.lock().ok()?.as_ref().map(|u| u.username.clone()) }
    fn user_id(&self) -> Result<i64> {
        self.identity.lock().map_err(|_| AppError::Other("内部状态异常".into()))?
            .as_ref().map(|u| u.id).ok_or_else(|| AppError::Other("请先登录 AI 账号".into()))
    }
    pub fn account_scope(&self) -> Result<String> { Ok(format!("{}|{}", brand::SITE, self.user_id()?)) }
}
#[derive(Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
struct Envelope<T> {
    success: bool,
    #[serde(default)] message: String,
    #[serde(default)] data: Option<T>,
}
fn unwrap<T>(env: Envelope<T>) -> Result<T> {
    if !env.success { return Err(AppError::Other(if env.message.is_empty() { "服务器拒绝了请求".into() } else { env.message })); }
    env.data.ok_or_else(|| AppError::Other("服务器返回的内容不完整".into()))
}
async fn decode<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    if !response.status().is_success() {
        return Err(AppError::Other(match response.status().as_u16() {
            401 => "登录已过期，请重新登录".into(),
            429 => "请求太频繁，请稍后再试".into(),
            code => format!("服务器请求失败（{}）", code),
        }));
    }
    let env = response.json::<Envelope<T>>().await.map_err(|_| AppError::Other("服务器响应格式不正确".into()))?;
    unwrap(env)
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginResult { pub username: String, pub require_2fa: bool }
fn adopt(s: &Session, value: serde_json::Value) -> Result<LoginResult> {
    if value.get("require_2fa").and_then(|v| v.as_bool()) == Some(true) {
        return Ok(LoginResult { username: String::new(), require_2fa: true });
    }
    let user: Identity = serde_json::from_value(value).map_err(|_| AppError::Other("登录响应不完整，请重新登录".into()))?;
    if user.id <= 0 || user.username.is_empty() { return Err(AppError::Other("登录响应不完整，请重新登录".into())); }
    let name = user.username.clone();
    *s.identity.lock().map_err(|_| AppError::Other("内部状态异常".into()))? = Some(user);
    Ok(LoginResult { username: name, require_2fa: false })
}
pub async fn login(s: &Session, username: &str, password: &str) -> Result<LoginResult> {
    let _guard = s.requests.lock().await;
    logout(s);
    if username.trim().is_empty() || password.is_empty() { return Err(AppError::Other("请填写账号和密码".into())); }
    let response = s.client.post(format!("{}/api/user/login", brand::SITE))
        .json(&serde_json::json!({"username": username.trim(), "password": password}))
        .send().await.map_err(network)?;
    adopt(s, decode(response).await?)
}
pub async fn login_2fa(s: &Session, code: &str) -> Result<LoginResult> {
    let _guard = s.requests.lock().await;
    if code.trim().is_empty() { return Err(AppError::Other("请输入验证码或备用码".into())); }
    let response = s.client.post(format!("{}/api/user/login/2fa", brand::SITE))
        .json(&serde_json::json!({"code": code.trim()})).send().await.map_err(network)?;
    adopt(s, decode(response).await?)
}
pub fn logout(s: &Session) { if let Ok(mut user) = s.identity.lock() { *user = None; } }
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayToken { pub id: i64, pub name: String, pub usable: bool, pub note: String }
#[derive(Deserialize)]
struct RawToken {
    id: i64,
    #[serde(default)] name: String,
    status: i64,
    #[serde(default)] expired_time: i64,
    #[serde(default)] remain_quota: i64,
    #[serde(default)] unlimited_quota: bool,
}
impl RawToken {
    fn view(self) -> GatewayToken {
        let note = if self.expired_time > 0 && self.expired_time <= chrono::Utc::now().timestamp() { "已过期" }
            else if self.status != 1 { "已停用或状态未知" }
            else if !self.unlimited_quota && self.remain_quota <= 0 { "额度已用完" } else { "" };
        GatewayToken { id: self.id, name: self.name, usable: note.is_empty(), note: note.into() }
    }
}
#[derive(Deserialize)]
struct Page { items: Vec<RawToken>, total: usize }
pub async fn list_tokens(s: &Session) -> Result<Vec<GatewayToken>> {
    let _guard = s.requests.lock().await;
    let uid = s.user_id()?;
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for page in 1..=1000 {
        let response = s.client.get(format!("{}/api/token/", brand::SITE)).header("New-Api-User", uid.to_string())
            .query(&[("p", page), ("page_size", 100)]).send().await.map_err(network)?;
        let data: Page = decode(response).await?;
        if data.items.is_empty() {
            if out.len() >= data.total { return Ok(out); }
            return Err(AppError::Other("密钥列表发生变化，请重新加载".into()));
        }
        for item in data.items {
            if !seen.insert(item.id) { return Err(AppError::Other("密钥分页重复，请重新加载".into())); }
            out.push(item.view());
        }
        if out.len() >= data.total { return Ok(out); }
    }
    Err(AppError::Other("密钥列表过大，请在控制台管理".into()))
}
pub async fn fetch_key(s: &Session, token_id: i64) -> Result<String> {
    let _guard = s.requests.lock().await;
    let uid = s.user_id()?;
    if token_id <= 0 { return Err(AppError::Other("密钥编号无效".into())); }
    #[derive(Deserialize)] struct Key { key: String }
    let response = s.client.post(format!("{}/api/token/{}/key", brand::SITE, token_id))
        .header("New-Api-User", uid.to_string()).send().await.map_err(network)?;
    let key: Key = decode(response).await?;
    if key.key.trim().is_empty() { return Err(AppError::Other("服务器没有返回完整密钥".into())); }
    Ok(if key.key.starts_with("sk-") { key.key } else { format!("sk-{}", key.key) })
}
fn network(e: reqwest::Error) -> AppError {
    AppError::Network(if e.is_timeout() { "连接超时，请检查网络或代理设置".into() } else { "无法连接 StarNexus 服务，请检查网络或代理设置".into() })
}
pub struct SessionState(pub Arc<Mutex<Option<Arc<Session>>>>);
impl Default for SessionState { fn default() -> Self { Self::new() } }
impl SessionState {
    pub fn new() -> Self { Self(Arc::new(Mutex::new(None))) }
    pub fn get_or_init(&self, ignore_proxy: bool) -> Result<Arc<Session>> {
        let mut held = self.0.lock().map_err(|_| AppError::Other("内部状态异常".into()))?;
        if let Some(s) = held.as_ref() { return Ok(s.clone()); }
        let s = Arc::new(Session::new(ignore_proxy)?); *held = Some(s.clone()); Ok(s)
    }
    pub fn reset(&self) { if let Ok(mut held) = self.0.lock() { *held = None; } }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn two_factor_does_not_create_authenticated_identity() {
        let s = Session::new(true).unwrap();
        assert!(adopt(&s, serde_json::json!({"require_2fa":true})).unwrap().require_2fa);
        assert!(s.account_scope().is_err());
        assert!(adopt(&s, serde_json::json!({"id":0,"username":"user"})).is_err());
        adopt(&s, serde_json::json!({"id":12,"username":"user"})).unwrap();
        assert_eq!(s.account_scope().unwrap(), "https://dkby.com|12");
        logout(&s); assert!(s.account_scope().is_err());
    }
    #[test]
    fn masked_listing_never_exposes_secret_and_checks_quota() {
        let token: RawToken = serde_json::from_value(serde_json::json!({"id":1,"name":"main","key":"SECRET","status":1,"expired_time":-1,"unlimited_quota":true})).unwrap();
        let view = token.view(); assert!(view.usable);
        assert!(!serde_json::to_string(&view).unwrap().contains("SECRET"));
        for patch in [serde_json::json!({"status":2}), serde_json::json!({"expired_time":1}), serde_json::json!({"unlimited_quota":false})] {
            let mut value = serde_json::json!({"id":1,"status":1,"unlimited_quota":true});
            for (k,v) in patch.as_object().unwrap() { value[k] = v.clone(); }
            assert!(!serde_json::from_value::<RawToken>(value).unwrap().view().usable);
        }
    }
    #[test]
    fn failure_envelope_cannot_be_treated_as_login_success() {
        let env: Envelope<serde_json::Value> = serde_json::from_str(r#"{"success":false,"message":"wrong password"}"#).unwrap();
        assert!(unwrap(env).is_err());
    }
}

pub async fn finish_browser_login(s: &Session, code: &str, verifier: &str) -> Result<LoginResult> {
    let _guard = s.requests.lock().await;
    logout(s);
    let response = s.client.post(format!("{}/api/desktop/exchange", brand::SITE))
        .json(&serde_json::json!({"code":code,"code_verifier":verifier})).send().await.map_err(network)?;
    adopt(s, decode(response).await?)
}
