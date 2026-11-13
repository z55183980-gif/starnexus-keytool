use crate::{brand, gateway, error::{AppError, Result}};
use base64::Engine;
use sha2::Digest;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const CALLBACK: &str = "com.dkby.starnexus.keytool://auth/callback";
pub const SCHEME: &str = "com.dkby.starnexus.keytool";
struct Pending { state: String, verifier: String, started: Instant }
#[derive(Default)]
pub struct BrowserLogin(Mutex<Option<Pending>>);
fn random_token(bytes: usize) -> Result<String> {
    let mut raw = vec![0u8; bytes];
    getrandom::getrandom(&mut raw).map_err(|_| AppError::Other("无法生成随机数".into()))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw))
}
fn challenge(verifier: &str) -> String { base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes())) }
impl BrowserLogin {
    fn begin(&self) -> Result<String> {
        let state = random_token(32)?;
        let verifier = random_token(48)?;
        let url = reqwest::Url::parse_with_params(&format!("{}/desktop-login", brand::SITE), &[("state",state.as_str()),("code_challenge",challenge(&verifier).as_str())])
            .map_err(|_| AppError::Other("无法生成登录地址".into()))?;
        *self.0.lock().map_err(|_| AppError::Other("内部状态异常".into()))? = Some(Pending { state, verifier, started: Instant::now() });
        Ok(url.into())
    }
    fn take(&self, state: &str) -> Result<String> {
        let mut held = self.0.lock().map_err(|_| AppError::Other("内部状态异常".into()))?;
        let pending = held.as_ref().ok_or_else(|| AppError::Other("没有进行中的官网登录，请重新发起".into()))?;
        if pending.started.elapsed() > Duration::from_secs(600) { *held = None; return Err(AppError::Other("这次登录已超时，请重新发起".into())); }
        let same = !state.is_empty() && state.len()==pending.state.len() && state.bytes().zip(pending.state.bytes()).fold(true,|same,(a,b)| same & (a==b));
        if !same { return Err(AppError::Other("登录回调与本次请求不匹配，已忽略".into())); }
        Ok(held.take().unwrap().verifier)
    }
    pub fn cancel(&self) { if let Ok(mut held)=self.0.lock(){*held=None;} }
}
#[tauri::command]
pub async fn gateway_browser_login_start(pending: tauri::State<'_, BrowserLogin>, ignore_proxy: bool) -> Result<String> {
    let client = crate::api::client(ignore_proxy)?;
    let response = client.get(format!("{}/api/desktop/config", brand::SITE)).send().await.map_err(|_| AppError::Other("无法连接官网登录服务".into()))?;
    let config: serde_json::Value = response.json().await.map_err(|_| AppError::Other("官网登录服务尚未更新，请先使用账号密码登录".into()))?;
    if config["success"] != true || config["data"]["callback"] != CALLBACK || config["data"]["pkce"] != "S256" { return Err(AppError::Other("官网登录服务尚未更新，请先使用账号密码登录".into())); }
    pending.begin()
}
#[tauri::command]
pub fn gateway_browser_login_cancel(pending: tauri::State<'_, BrowserLogin>) { pending.cancel(); }
#[tauri::command]
pub async fn gateway_browser_login_finish(sessions: tauri::State<'_, gateway::SessionState>, pending: tauri::State<'_, BrowserLogin>, code: String, state: String, ignore_proxy: bool) -> Result<gateway::LoginResult> {
    if code.len()!=43 || !code.bytes().all(|b| b.is_ascii_alphanumeric() || b==b'-' || b==b'_') { return Err(AppError::Other("登录授权码无效".into())); }
    let verifier = pending.take(&state)?;
    sessions.reset();
    let session = sessions.get_or_init(ignore_proxy)?;
    gateway::finish_browser_login(&session, &code, &verifier).await
}
#[cfg(test)]
mod tests {
    use super::*;
    fn state(url: &str)->String { reqwest::Url::parse(url).unwrap().query_pairs().find(|(k,_)|k=="state").unwrap().1.to_string() }
    #[test] fn requests_use_pkce_and_cannot_be_replayed_or_crossed() {
        let pending=BrowserLogin::default(); let url=pending.begin().unwrap(); let first=state(&url);
        assert!(pending.take("wrong-state").is_err());
        let verifier=pending.take(&first).unwrap();
        assert!(!url.contains(&verifier));
        assert!(url.contains(&challenge(&verifier)));
        assert!(pending.take(&first).is_err());
        let old=state(&pending.begin().unwrap()); let new=state(&pending.begin().unwrap());
        assert!(pending.take(&old).is_err()); assert!(pending.take(&new).is_ok());
    }
    #[test] fn cancel_and_expiration_drop_pending_login() {
        let pending=BrowserLogin::default(); let value=state(&pending.begin().unwrap()); pending.cancel(); assert!(pending.take(&value).is_err());
        let value=state(&pending.begin().unwrap()); pending.0.lock().unwrap().as_mut().unwrap().started=Instant::now()-Duration::from_secs(601); assert!(pending.take(&value).is_err());
    }
}
