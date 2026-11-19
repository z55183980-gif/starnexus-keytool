//! Trusted update policy, persistent minimum-version gate and signed native updates.
use crate::{
    api, atomic, brand,
    commands::blocking,
    error::{AppError, Result},
    paths,
};
use semver::Version;
use serde::Serialize;
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};
use tauri::{ipc::Channel, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

static POLICY: Mutex<Option<ClientPolicy>> = Mutex::new(None);
static REFRESH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static INSTALLING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientPolicy {
    pub config: api::RemoteConfig,
    pub current_version: String,
    pub required: bool,
    pub source: String,
    pub warning: Option<String>,
}

pub fn version(raw: &str) -> Result<Version> {
    Version::parse(raw.trim().trim_start_matches('v'))
        .map_err(|_| AppError::Other("版本号格式无效".into()))
}

pub fn requires_update(current: &str, minimum: &str) -> Result<bool> {
    Ok(!minimum.trim().is_empty() && version(current)?.cmp_precedence(&version(minimum)?).is_lt())
}

fn validate(config: &api::RemoteConfig) -> Result<()> {
    requires_update(env!("CARGO_PKG_VERSION"), &config.min_client_version)?;
    let url = reqwest::Url::parse(&config.base_url)
        .map_err(|_| AppError::Other("远程服务地址无效".into()))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !["responses", "chat"].contains(&config.wire_api.as_str())
    {
        return Err(AppError::Other("远程配置格式无效".into()));
    }
    Ok(())
}

fn cached(path: &Path) -> Result<Option<api::RemoteConfig>> {
    let Some(text) = atomic::read_text_optional(path)? else {
        return Ok(None);
    };
    if text.len() > 64 * 1024 {
        return Err(AppError::Other("版本策略缓存过大".into()));
    }
    let config = api::parse_remote_config(text.as_bytes())
        .map_err(|_| AppError::Other("版本策略缓存损坏".into()))?;
    validate(&config)?;
    Ok(Some(config))
}

fn snapshot(
    config: api::RemoteConfig,
    source: &str,
    warning: Option<String>,
) -> Result<ClientPolicy> {
    Ok(ClientPolicy {
        required: requires_update(env!("CARGO_PKG_VERSION"), &config.min_client_version)?,
        config,
        current_version: env!("CARGO_PKG_VERSION").into(),
        source: source.into(),
        warning,
    })
}

fn current_policy() -> Result<ClientPolicy> {
    let mut guard = POLICY
        .lock()
        .map_err(|_| AppError::Other("版本策略锁不可用".into()))?;
    if let Some(policy) = &*guard {
        return Ok(policy.clone());
    }
    let policy = match cached(&paths::app_dir()?.join("update-policy.json")) {
        Ok(Some(config)) => snapshot(config, "cache", None)?,
        _ => snapshot(api::builtin_defaults(), "builtin", None)?,
    };
    *guard = Some(policy.clone());
    Ok(policy)
}

pub fn configuration() -> Result<api::RemoteConfig> {
    Ok(current_policy()?.config)
}

/// Also checked at IPC boundaries: hiding the page alone is not an enforcement mechanism.
pub fn ensure_supported() -> Result<()> {
    if INSTALLING.load(Ordering::Acquire) {
        return Err(AppError::Other("正在安装更新，请稍候".into()));
    }
    enforce(&current_policy()?)
}

fn enforce(policy: &ClientPolicy) -> Result<()> {
    if policy.required {
        return Err(AppError::Other(format!(
            "当前版本过低，请先更新到 {} 或更高版本",
            policy.config.min_client_version
        )));
    }
    Ok(())
}

#[tauri::command]
pub async fn get_client_policy(ignore_proxy: bool) -> Result<ClientPolicy> {
    let _refresh = REFRESH.lock().await;
    let previous = blocking(current_policy).await?;
    let remote = api::fetch_remote_config(brand::CLIENT_CONFIG, ignore_proxy)
        .await
        .and_then(|cfg| {
            validate(&cfg)?;
            Ok(cfg)
        });
    let next = match remote {
        Ok(config) => {
            let copy = config.clone();
            let saved = blocking(move || {
                atomic::write_atomic(
                    &paths::app_dir()?.join("update-policy.json"),
                    &serde_json::to_string(&copy)?,
                )
            })
            .await;
            snapshot(
                config,
                "remote",
                saved
                    .err()
                    .map(|_| "版本策略未能保存，离线时可能无法使用最新策略".into()),
            )?
        }
        Err(_) => ClientPolicy {
            warning: Some("无法获取版本策略，正在使用最近一次有效配置".into()),
            ..previous
        },
    };
    *POLICY
        .lock()
        .map_err(|_| AppError::Other("版本策略锁不可用".into()))? = Some(next.clone());
    Ok(next)
}

#[derive(Default)]
pub struct UpdateState(pub tokio::sync::Mutex<Option<Update>>);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableUpdate {
    pub version: String,
    pub notes: String,
}

fn acceptable_update(current: &str, offered: &str, minimum: &str) -> Result<bool> {
    Ok(version(offered)?.cmp_precedence(&version(current)?).is_gt()
        && !requires_update(offered, minimum)?)
}

#[tauri::command]
pub async fn check_for_updates(
    app: tauri::AppHandle,
    ignore_proxy: bool,
) -> Result<Option<AvailableUpdate>> {
    let state = app.state::<UpdateState>();
    let mut pending = state.0.lock().await;
    if INSTALLING.load(Ordering::Acquire) {
        return Err(AppError::Other("正在安装更新，请稍候".into()));
    }
    *pending = None;
    let Some(pubkey) = option_env!("STARNEXUS_UPDATER_PUBLIC_KEY").filter(|s| !s.trim().is_empty()) else {
        return Err(AppError::Other("更新服务尚未配置，请前往下载页面".into()));
    };
    let endpoint = reqwest::Url::parse("https://github.com/z55183980-gif/starnexus-keytool/releases/latest/download/latest.json").map_err(|e| AppError::Other(e.to_string()))?;
    let mut builder = app.updater_builder().pubkey(pubkey).endpoints(vec![endpoint]).map_err(|e| AppError::Other(e.to_string()))?.timeout(Duration::from_secs(8));
    if ignore_proxy {
        builder = builder.no_proxy();
    }
    let updater = builder
        .build()
        .map_err(|_| AppError::Other("更新服务尚未配置，请前往下载页面".into()))?;
    let update = updater
        .check()
        .await
        .map_err(|_| AppError::Other("检查更新失败，请重试或前往下载页面".into()))?;
    let Some(mut update) = update else {
        return Ok(None);
    };
    let policy = blocking(current_policy).await?;
    if !acceptable_update(
        &policy.current_version,
        &update.version,
        &policy.config.min_client_version,
    )? {
        return Err(AppError::Other(
            "当前更新包未满足最低版本要求，请前往下载页面或稍后重试".into(),
        ));
    }
    if update.download_url.scheme() != "https" {
        return Err(AppError::Other("更新下载地址必须使用 HTTPS".into()));
    }
    // Metadata should fail quickly; installer downloads need a longer bounded timeout.
    update.timeout = Some(Duration::from_secs(180));
    let view = AvailableUpdate {
        version: update.version.clone(),
        notes: update.body.clone().unwrap_or_default(),
    };
    *pending = Some(update);
    Ok(Some(view))
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgress {
    pub phase: &'static str,
    pub downloaded: u64,
    pub total: Option<u64>,
}

struct InstallGuard;
impl Drop for InstallGuard {
    fn drop(&mut self) {
        INSTALLING.store(false, Ordering::Release);
    }
}

#[tauri::command]
pub async fn install_update(
    app: tauri::AppHandle,
    on_progress: Channel<UpdateProgress>,
) -> Result<()> {
    let state = app.state::<UpdateState>();
    let pending = state.0.lock().await;
    let update = pending
        .clone()
        .ok_or_else(|| AppError::Other("请先检查更新".into()))?;
    if INSTALLING.swap(true, Ordering::AcqRel) {
        return Err(AppError::Other("正在安装更新，请稍候".into()));
    }
    let _guard = InstallGuard;
    drop(pending);
    let policy = blocking(current_policy).await?;
    if !acceptable_update(
        &policy.current_version,
        &update.version,
        &policy.config.min_client_version,
    )? {
        return Err(AppError::Other("请重新检查可用更新".into()));
    }
    let mut downloaded = 0u64;
    let mut last = Instant::now() - Duration::from_secs(1);
    let progress = on_progress.clone();
    let bytes = update
        .download(
            move |chunk, total| {
                downloaded += chunk as u64;
                if last.elapsed() >= Duration::from_millis(150) || total == Some(downloaded) {
                    let _ = progress.send(UpdateProgress {
                        phase: "downloading",
                        downloaded,
                        total,
                    });
                    last = Instant::now();
                }
            },
            || {},
        )
        .await
        .map_err(|_| AppError::Other("更新下载或签名校验失败，未安装；请重试".into()))?;
    let _ = on_progress.send(UpdateProgress {
        phase: "installing",
        downloaded: bytes.len() as u64,
        total: Some(bytes.len() as u64),
    });
    blocking(move || {
        let _config = crate::backup::lock()?;
        let policy = current_policy()?;
        if !acceptable_update(
            &policy.current_version,
            &update.version,
            &policy.config.min_client_version,
        )? {
            return Err(AppError::Other("请重新检查可用更新".into()));
        }
        update
            .install(bytes)
            .map_err(|_| AppError::Other("更新安装失败，请重试或手动下载安装包".into()))
    })
    .await?;
    app.restart();
}

#[tauri::command]
pub fn open_update_download(app: tauri::AppHandle) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(brand::RELEASES, None::<&str>)
        .map_err(|e| AppError::Other(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn minimum_uses_semver_precedence_not_lexical_order_or_build_metadata() {
        for (current, min, required) in [
            ("0.2.9", "0.2.10", true),
            ("0.2.10", "v0.2.9", false),
            ("1.0.0-beta.1", "1.0.0", true),
            ("1.0.0+one", "1.0.0+two", false),
            ("0.2.6", "", false),
        ] {
            assert_eq!(requires_update(current, min).unwrap(), required);
        }
        assert!(requires_update("0.2.6", "latest").is_err());
        assert!(requires_update("0.2.6", "0.2").is_err());
    }
    #[test]
    fn cached_minimum_remains_enforced_offline_and_can_be_explicitly_lifted() {
        let dir = crate::atomic::tests::TestDir::new();
        let path = dir.0.join("policy.json");
        let mut config = api::builtin_defaults();
        config.min_client_version = "99.0.0".into();
        atomic::write_atomic(&path, &serde_json::to_string(&config).unwrap()).unwrap();
        let policy = snapshot(cached(&path).unwrap().unwrap(), "cache", None).unwrap();
        assert!(enforce(&policy).is_err());
        config.min_client_version.clear();
        assert!(enforce(&snapshot(config, "remote", None).unwrap()).is_ok());
        std::fs::write(&path, "broken").unwrap();
        assert!(cached(&path).is_err());
    }
    #[test]
    fn rejects_invalid_policy_and_updates_below_minimum_or_current() {
        let mut config = api::builtin_defaults();
        config.min_client_version = "nonsense".into();
        assert!(validate(&config).is_err());
        config.min_client_version.clear();
        config.base_url = "http://insecure.example/v1".into();
        assert!(validate(&config).is_err());
        assert!(!acceptable_update("1.0.0", "1.1.0", "2.0.0").unwrap());
        assert!(!acceptable_update("1.0.0", "1.0.0+build", "").unwrap());
        assert!(!acceptable_update("1.0.0", "0.9.0", "").unwrap());
        assert!(acceptable_update("1.0.0", "2.0.0", "2.0.0").unwrap());
    }
}
