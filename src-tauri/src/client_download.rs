//! All four installation paths share bounded downloads.
//! Route rule: if the official host is reachable, download directly; if not, use the relay. Decided once per host, no switching mid-download.
use crate::{
    error::{AppError, Result},
    launcher::Target,
};
use base64::Engine;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use tauri::ipc::Channel;

const RELAY: Option<&str> = option_env!("STARNEXUS_DOWNLOAD_RELAY");
const LIMIT: u64 = 2 * 1024 * 1024 * 1024;
static INSTALL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub phase: &'static str,
    pub downloaded: u64,
    pub total: Option<u64>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub installed: bool,
    pub message: &'static str,
}
fn report(channel: &Channel<Progress>, phase: &'static str, downloaded: u64, total: Option<u64>) {
    let _ = channel.send(Progress {
        phase,
        downloaded,
        total,
    });
}
fn err(message: impl Into<String>) -> AppError {
    AppError::Other(message.into())
}

pub(crate) fn allowed(url: &reqwest::Url) -> bool {
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(443)
    {
        return false;
    }
    let path = url.path();
    match url.host_str().unwrap_or_default() {
        "downloads.claude.ai" => {
            path.starts_with("/claude-code-releases/") || path.starts_with("/releases/")
        }
        "claude.ai" => path.starts_with("/api/desktop/"),
        "api.github.com" => path.starts_with("/repos/openai/codex/releases/"),
        "github.com" => path.starts_with("/openai/codex/releases/download/"),
        "release-assets.githubusercontent.com" => {
            path.starts_with("/github-production-release-asset/")
        }
        "persistent.oaistatic.com" => path.starts_with("/codex-app-prod/"),
        "formulae.brew.sh" => path == "/api/cask/claude.json",
        _ => false,
    }
}

fn relay_url(relay: &str, url: &str) -> String {
    format!(
        "{relay}?url={}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(url)
    )
}
/// How long a reachability probe waits: no successful response in this window counts as unreachable
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
/// How long the per-host route decision is kept; one install / version check reuses it instead of probing again
const ROUTE_TTL: Duration = Duration::from_secs(10 * 60);
static ROUTES: OnceLock<Mutex<HashMap<String, (bool, Instant)>>> = OnceLock::new();

/// Can the official address be reached: a successful response within a few seconds is enough; the body is not read
async fn reachable(url: &str) -> bool {
    let Ok(client) = client() else { return false };
    matches!(
        tokio::time::timeout(PROBE_TIMEOUT, client.get(url).header("Range", "bytes=0-0").send()).await,
        Ok(Ok(response)) if response.status().is_success()
    )
}
/// Pick a route for one address: official direct if reachable, otherwise the given relay address. Returns (address, via relay)
async fn pick(url: &str, relay: String) -> (String, bool) {
    if reachable(url).await { (url.to_string(), false) } else { (relay, true) }
}
/// Same as `pick`, but the result is cached per official host
async fn route(url: &str) -> (String, bool) {
    let Some(endpoint) = RELAY.filter(|value| reqwest::Url::parse(value).is_ok_and(|url| url.scheme() == "https" && url.host_str().is_some() && url.username().is_empty() && url.password().is_none() && url.query().is_none() && url.fragment().is_none())) else { return (url.to_string(), false) };
    let host = reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default();
    let routes = ROUTES.get_or_init(|| Mutex::new(HashMap::new()));
    let cached = routes.lock().ok().and_then(|map| map.get(&host).filter(|(_, at)| at.elapsed() < ROUTE_TTL).map(|(relay, _)| *relay));
    if let Some(relay) = cached {
        return if relay { (relay_url(endpoint, url), true) } else { (url.to_string(), false) };
    }
    let (endpoint, relay) = pick(url, relay_url(endpoint, url)).await;
    if let Ok(mut map) = routes.lock() { map.insert(host, (relay, Instant::now())); }
    (endpoint, relay)
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(6))
        .user_agent("starnexus-keytool/0.2 installer")
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 6 || !allowed(attempt.url()) {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|e| err(e.to_string()))
}

async fn transfer(
    url: &str,
    destination: &Path,
    max: u64,
    relay: bool,
    channel: &Channel<Progress>,
) -> Result<()> {
    let started = Instant::now();
    let phase = if relay { "relay" } else { "download" };
    report(channel, phase, 0, None);
    let mut response = tokio::time::timeout(
        Duration::from_secs(35),
        client()?.get(url).send(),
    )
    .await
    .map_err(|_| err("下载连接超时"))?
    .map_err(|e| err(e.to_string()))?;
    if !response.status().is_success() {
        return Err(err(format!("下载服务返回 {}", response.status().as_u16())));
    }
    let total = response.content_length();
    if total.is_some_and(|n| n > max) {
        return Err(err("安装文件超过大小限制"));
    }
    let mut file = File::create(destination)?;
    let mut downloaded = 0;
    let mut last_report = Instant::now();
    loop {
        if started.elapsed() > Duration::from_secs(30 * 60) {
            return Err(err("下载时间过长，请稍后重试。"));
        }
        let chunk = tokio::time::timeout(
            Duration::from_secs(25),
            response.chunk(),
        )
        .await
        .map_err(|_| err("下载连接中断或长时间无响应"))?
        .map_err(|e| err(e.to_string()))?;
        let Some(chunk) = chunk else {
            break;
        };
        downloaded += chunk.len() as u64;
        if downloaded > max {
            return Err(err("安装文件超过大小限制"));
        }
        file.write_all(&chunk)?;
        if last_report.elapsed() >= Duration::from_millis(250) {
            report(channel, phase, downloaded, total);
            last_report = Instant::now();
        }
    }
    if downloaded == 0 || total.is_some_and(|n| n != downloaded) {
        return Err(err("安装文件下载不完整"));
    }
    file.sync_all()?;
    report(channel, phase, downloaded, total);
    Ok(())
}

async fn download(
    url: &str,
    destination: &Path,
    max: u64,
    channel: &Channel<Progress>,
) -> Result<()> {
    if !allowed(&reqwest::Url::parse(url).map_err(|_| err("安装资源地址无效"))?) {
        return Err(err("安装资源不属于受支持的官方来源"));
    }
    let (endpoint, relay) = route(url).await;
    download_from(&endpoint, relay, destination, max, channel).await
}
async fn download_from(
    endpoint: &str,
    relay: bool,
    destination: &Path,
    max: u64,
    channel: &Channel<Progress>,
) -> Result<()> {
    let result = transfer(endpoint, destination, max, relay, channel).await;
    if result.is_err() {
        let _ = fs::remove_file(destination);
    }
    result.map_err(|e| err(format!("{}未完成：{}", if relay { "中继下载" } else { "官方下载" }, e)))
}
async fn metadata(url: &str, dir: &Path, channel: &Channel<Progress>) -> Result<Vec<u8>> {
    let path = dir.join("metadata.json");
    download(url, &path, 4 * 1024 * 1024, channel).await?;
    let data = fs::read(&path)?;
    fs::remove_file(path)?;
    Ok(data)
}

struct Asset {
    version: String,
    url: String,
    filename: &'static str,
    sha256: Option<String>,
    kind: &'static str,
}

// Version checks read only the ZIP directory and manifest, never the full MSIX.
fn content_range(value: &str) -> Option<(usize, usize, usize)> {
    let (bounds, size) = value.strip_prefix("bytes ")?.split_once('/')?;
    let (start, end) = bounds.split_once('-')?;
    let (start, end, size) = (start.parse().ok()?, end.parse().ok()?, size.parse().ok()?);
    (start <= end && end < size).then_some((start, end, size))
}
async fn range_bytes(url: &str, range: &str) -> Result<Vec<u8>> {
    if !allowed(&reqwest::Url::parse(url).map_err(|_| err("安装资源地址无效"))?) {
        return Err(err("安装资源地址无效"));
    }
    let (endpoint, _) = route(url).await;
    let attempt = tokio::time::timeout(Duration::from_secs(20), async {
            let mut response = client()?
                .get(endpoint)
                .header("Range", range)
                .send()
                .await
                .map_err(|e| err(e.to_string()))?;
            if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
                return Err(err("服务器不支持安装包版本查询"));
            }
            let (start, end, _) = response
                .headers()
                .get("content-range")
                .and_then(|h| h.to_str().ok())
                .and_then(content_range)
                .ok_or_else(|| err("安装包分段响应无效"))?;
            if range != format!("bytes={start}-{end}") {
                return Err(err("安装包分段响应不匹配"));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|e| err(e.to_string()))? {
                if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
                    return Err(err("版本响应过大"));
                }
                bytes.extend_from_slice(&chunk);
            }
            if bytes.len() != end - start + 1 {
                return Err(err("安装包分段响应不完整"));
            }
            Ok(bytes)
        })
        .await;
    match attempt {
        Ok(Ok(bytes)) => Ok(bytes),
        _ => Err(err("无法查询安装包版本，请稍后重试。")),
    }
}
fn u16_at(bytes: &[u8], index: usize) -> Result<usize> {
    Ok(u16::from_le_bytes(
        bytes
            .get(index..index + 2)
            .ok_or_else(|| err("安装包目录无效"))?
            .try_into()
            .unwrap(),
    ) as usize)
}
fn u32_at(bytes: &[u8], index: usize) -> Result<usize> {
    Ok(u32::from_le_bytes(
        bytes
            .get(index..index + 4)
            .ok_or_else(|| err("安装包目录无效"))?
            .try_into()
            .unwrap(),
    ) as usize)
}
fn manifest_entry(tail: &[u8]) -> Result<(usize, usize, usize)> {
    for index in 0..tail.len().saturating_sub(46) {
        if tail[index..].starts_with(b"PK\x01\x02") {
            let len = u16_at(tail, index + 28)?;
            if tail.get(index + 46..index + 46 + len) == Some(b"AppxManifest.xml".as_slice()) {
                let mut size = u32_at(tail, index + 20)?;
                let mut offset = u32_at(tail, index + 42)?;
                // Claude uses ZIP64 fields even for packages smaller than 4 GiB.
                if size == u32::MAX as usize || offset == u32::MAX as usize {
                    let extra_start = index + 46 + len;
                    let extra = tail
                        .get(extra_start..extra_start + u16_at(tail, index + 30)?)
                        .ok_or_else(|| err("安装包 ZIP64 目录无效"))?;
                    let mut pos = 0;
                    while pos + 4 <= extra.len() {
                        let tag = u16_at(extra, pos)?;
                        let length = u16_at(extra, pos + 2)?;
                        let field = extra
                            .get(pos + 4..pos + 4 + length)
                            .ok_or_else(|| err("安装包 ZIP64 目录无效"))?;
                        if tag == 1 {
                            let mut cursor = if u32_at(tail, index + 24)? == u32::MAX as usize {
                                8
                            } else {
                                0
                            };
                            let read = |cursor: usize| -> Result<usize> {
                                let bytes = field
                                    .get(cursor..cursor + 8)
                                    .ok_or_else(|| err("安装包 ZIP64 目录无效"))?;
                                usize::try_from(u64::from_le_bytes(bytes.try_into().unwrap()))
                                    .map_err(|_| err("安装包目录过大"))
                            };
                            if size == u32::MAX as usize {
                                size = read(cursor)?;
                                cursor += 8;
                            }
                            if offset == u32::MAX as usize {
                                offset = read(cursor)?;
                            }
                            break;
                        }
                        pos += 4 + length;
                    }
                }
                if size > 1024 * 1024 || offset > LIMIT as usize {
                    return Err(err("安装包版本清单过大"));
                }
                return Ok((offset, size, u16_at(tail, index + 10)?));
            }
        }
    }
    Err(err("无法读取安装包版本清单"))
}
async fn msix_version(url: &str) -> Result<String> {
    // End ranges use a fixed offset after discovering total size from Content-Range.
    let (endpoint, _) = route(url).await;
    let mut size = None;
    if let Ok(Ok(response)) = tokio::time::timeout(
        Duration::from_secs(15),
        client()?.get(endpoint).header("Range", "bytes=0-0").send(),
    )
    .await
    {
        if response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
            size = response
                .headers()
                .get("content-range")
                .and_then(|h| h.to_str().ok())
                .and_then(content_range)
                .filter(|(start, end, _)| *start == 0 && *end == 0)
                .map(|(_, _, size)| size);
        }
    }
    let size = size
        .filter(|n| *n > 0 && *n <= LIMIT as usize)
        .ok_or_else(|| err("无法查询安装包大小"))?;
    let tail = range_bytes(
        url,
        &format!("bytes={}-{}", size.saturating_sub(256 * 1024), size - 1),
    )
    .await?;
    let (offset, len, method) = manifest_entry(&tail)?;
    let header = range_bytes(url, &format!("bytes={offset}-{}", offset + 29)).await?;
    if !header.starts_with(b"PK\x03\x04") {
        return Err(err("安装包目录无效"));
    }
    let start = offset + 30 + u16_at(&header, 26)? + u16_at(&header, 28)?;
    if len == 0 || start + len > size {
        return Err(err("安装包目录无效"));
    }
    let data = range_bytes(url, &format!("bytes={start}-{}", start + len - 1)).await?;
    let xml = if method == 0 {
        data
    } else if method == 8 {
        let mut output = Vec::new();
        flate2::read::DeflateDecoder::new(data.as_slice())
            .take(1024 * 1024 + 1)
            .read_to_end(&mut output)?;
        output
    } else {
        return Err(err("安装包压缩格式不支持版本查询"));
    };
    if xml.len() > 1024 * 1024 {
        return Err(err("版本响应过大"));
    }
    let mut reader = quick_xml::Reader::from_reader(xml.as_slice());
    loop {
        match reader.read_event().map_err(|e| err(e.to_string()))? {
            quick_xml::events::Event::Start(e) | quick_xml::events::Event::Empty(e)
                if e.local_name().as_ref() == "Identity" =>
            {
                for a in e.attributes() {
                    let a = a.map_err(|e| err(e.to_string()))?;
                    if a.key.as_ref() == "Version" {
                        return Ok(a
                            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                            .map_err(|e| err(e.to_string()))?
                            .into_owned());
                    }
                }
            }
            quick_xml::events::Event::Eof => break,
            _ => (),
        }
    }
    Err(err("安装包未提供版本"))
}
fn mac_codex_release(xml: &[u8]) -> Result<(String, String)> {
    let mut reader = quick_xml::Reader::from_reader(xml);
    let mut version = None;
    loop {
        match reader.read_event().map_err(|e| err(e.to_string()))? {
            quick_xml::events::Event::Start(e)
                if e.local_name().as_ref() == "shortVersionString" =>
            {
                version = Some(
                    reader
                        .read_text(e.name())
                        .map_err(|e| err(e.to_string()))?
                        .xml10_content()
                        .into_owned(),
                );
            }
            quick_xml::events::Event::Empty(e) if e.local_name().as_ref() == "enclosure" => {
                for a in e.attributes() {
                    let a = a.map_err(|e| err(e.to_string()))?;
                    if a.key.as_ref() == "url" {
                        let url = a
                            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                            .map_err(|e| err(e.to_string()))?
                            .into_owned();
                        if url.ends_with(".zip") {
                            if let Some(version) = version.take() {
                                return Ok((version, url));
                            }
                        }
                    }
                }
            }
            quick_xml::events::Event::Eof => break,
            _ => (),
        }
    }
    Err(err("最新稳定版本信息不可用"))
}

pub(crate) async fn latest_available_version(target: Target) -> Result<String> {
    let dir = crate::paths::app_dir()?.join("version-checks").join(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| err(e.to_string()))?
            .as_nanos()
            .to_string(),
    );
    fs::create_dir_all(&dir)?;
    let result = resolve(target, &dir, &Channel::new(|_| Ok(())))
        .await
        .map(|asset| asset.version);
    let _ = fs::remove_dir_all(dir);
    result
}
fn normalize_arch(value: &str) -> Result<&'static str> {
    match value.to_ascii_lowercase().as_str() {
        "arm64" | "aarch64" => Ok("arm64"),
        "amd64" | "x86_64" | "x64" => Ok("x64"),
        _ => Err(err("当前处理器架构没有可用安装包，请查看官方系统要求。")),
    }
}
fn arch() -> Result<&'static str> {
    #[cfg(windows)]
    if let Some(native) = std::env::var("PROCESSOR_ARCHITEW6432").ok().filter(|s| !s.is_empty())
        .or_else(|| std::env::var("PROCESSOR_ARCHITECTURE").ok().filter(|s| !s.is_empty())) {
        return normalize_arch(&native);
    }
    #[cfg(target_os = "macos")]
    if crate::process::output(std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", "hw.optional.arm64"]), Duration::from_secs(5))
        .is_ok_and(|r| r.status.success() && String::from_utf8_lossy(&r.stdout).trim() == "1")
    {
        return Ok("arm64");
    }
    normalize_arch(std::env::consts::ARCH)
}
async fn resolve(target: Target, dir: &Path, channel: &Channel<Progress>) -> Result<Asset> {
    let windows = cfg!(windows);
    let arch = arch()?;
    if !cfg!(any(windows, target_os = "macos")) {
        return Err(err("此系统请使用官方安装说明。"));
    }
    match target {
        Target::ClaudeCli => {
            let base = "https://downloads.claude.ai/claude-code-releases";
            let version =
                String::from_utf8(metadata(&format!("{base}/latest"), dir, channel).await?)
                    .map_err(|_| err("版本信息无效"))?;
            let version = version.trim();
            semver::Version::parse(version).map_err(|_| err("版本信息无效"))?;
            let manifest: serde_json::Value = serde_json::from_slice(
                &metadata(&format!("{base}/{version}/manifest.json"), dir, channel).await?,
            )?;
            let platform = format!("{}-{arch}", if windows { "win32" } else { "darwin" });
            let checksum = manifest["platforms"][&platform]["checksum"]
                .as_str()
                .ok_or_else(|| err("当前系统没有可用安装包"))?;
            let filename = if windows { "claude.exe" } else { "claude" };
            Ok(Asset {
                version: version.into(),
                url: format!("{base}/{version}/{platform}/{filename}"),
                filename,
                sha256: Some(checksum.into()),
                kind: "binary",
            })
        }
        Target::CodexCli => {
            let release: serde_json::Value = serde_json::from_slice(
                &metadata(
                    "https://api.github.com/repos/openai/codex/releases/latest",
                    dir,
                    channel,
                )
                .await?,
            )?;
            if release["draft"] != false || release["prerelease"] != false {
                return Err(err("稳定版信息无效"));
            }
            let name = format!(
                "codex-{}-{}",
                if arch == "arm64" { "aarch64" } else { "x86_64" },
                if windows {
                    "pc-windows-msvc.exe.zip"
                } else {
                    "apple-darwin.tar.gz"
                }
            );
            let asset = release["assets"]
                .as_array()
                .and_then(|a| a.iter().find(|a| a["name"].as_str() == Some(&name)))
                .ok_or_else(|| err("当前系统没有可用安装包"))?;
            let digest = asset["digest"]
                .as_str()
                .and_then(|v| v.strip_prefix("sha256:"))
                .ok_or_else(|| err("官方安装包缺少校验值"))?;
            Ok(Asset {
                version: crate::client_setup::release_version(&serde_json::to_vec(&release)?)?,
                url: asset["browser_download_url"]
                    .as_str()
                    .ok_or_else(|| err("安装资源地址无效"))?
                    .into(),
                filename: if windows { "codex.zip" } else { "codex.tar.gz" },
                sha256: Some(digest.into()),
                kind: if windows { "zip" } else { "tgz" },
            })
        }
        Target::ClaudeDesktop | Target::CodexDesktop if windows => {
            let url = if target == Target::ClaudeDesktop {
                format!("https://claude.ai/api/desktop/win32/{arch}/msix/latest/redirect")
            } else {
                format!("https://persistent.oaistatic.com/codex-app-prod/ChatGPT-{arch}.msix")
            };
            let version = msix_version(&url).await?;
            Ok(Asset {
                version,
                url,
                filename: if target == Target::ClaudeDesktop {
                    "Claude.msix"
                } else {
                    "Codex.msix"
                },
                sha256: None,
                kind: "msix",
            })
        }
        Target::ClaudeDesktop => {
            // Homebrew's maintained cask identifies the official universal archive and its hash.
            let cask: serde_json::Value = serde_json::from_slice(
                &metadata(
                    "https://formulae.brew.sh/api/cask/claude.json",
                    dir,
                    channel,
                )
                .await?,
            )?;
            Ok(Asset {
                version: cask["version"]
                    .as_str()
                    .ok_or_else(|| err("版本信息无效"))?
                    .split(',')
                    .next()
                    .unwrap_or_default()
                    .into(),
                url: cask["url"]
                    .as_str()
                    .ok_or_else(|| err("安装资源地址无效"))?
                    .into(),
                filename: "Claude.zip",
                sha256: Some(
                    cask["sha256"]
                        .as_str()
                        .ok_or_else(|| err("官方安装包缺少校验值"))?
                        .into(),
                ),
                kind: "app-zip",
            })
        }
        Target::CodexDesktop if arch == "arm64" => {
            let feed = metadata(
                "https://persistent.oaistatic.com/codex-app-prod/appcast.xml",
                dir,
                channel,
            )
            .await?;
            let (version, url) = mac_codex_release(&feed)?;
            Ok(Asset {
                version,
                url,
                filename: "Codex.zip",
                sha256: None,
                kind: "app-zip",
            })
        }
        Target::CodexDesktop => Err(err(
            "Codex 桌面版当前仅提供 Apple Silicon 安装包，请使用 CLI。",
        )),
    }
}
fn verify_hash(path: &Path, expected: &str) -> Result<()> {
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(err("安装包校验值无效"));
    }
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = [0u8; 128 * 1024];
    loop {
        let len = file.read(&mut buf)?;
        if len == 0 {
            break;
        }
        hash.update(&buf[..len]);
    }
    if format!("{:x}", hash.finalize()) != expected.to_lowercase() {
        return Err(err("安装包完整性校验失败，未运行安装。"));
    }
    Ok(())
}

fn unpack(path: &Path, dir: &Path, kind: &str) -> Result<()> {
    fs::create_dir(dir)?;
    if kind == "zip" {
        let mut archive =
            zip::ZipArchive::new(File::open(path)?).map_err(|e| err(e.to_string()))?;
        let mut expanded = 0u64;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|e| err(e.to_string()))?;
            let name = entry
                .enclosed_name()
                .ok_or_else(|| err("安装包包含无效路径"))?;
            expanded += entry.size();
            if expanded > LIMIT {
                return Err(err("解压文件超过大小限制"));
            }
            if entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
                return Err(err("CLI 安装包包含意外链接"));
            }
            let dest = dir.join(name);
            if entry.is_dir() {
                fs::create_dir_all(dest)?;
            } else {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                std::io::copy(&mut entry, &mut File::create(dest)?)?;
            }
        }
    } else {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(File::open(path)?));
        let mut expanded = 0u64;
        for entry in archive.entries()? {
            let mut entry = entry?;
            expanded += entry.size();
            if expanded > LIMIT
                || !(entry.header().entry_type().is_file() || entry.header().entry_type().is_dir())
            {
                return Err(err("CLI 安装包内容无效"));
            }
            if !entry.unpack_in(dir)? {
                return Err(err("安装包包含无效路径"));
            }
        }
    }
    Ok(())
}
fn find_binary(dir: &Path, prefix: &str) -> Result<PathBuf> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            if let Ok(found) = find_binary(&path, prefix) {
                return Ok(found);
            }
        } else if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
            n == prefix
                || n == format!("{prefix}.exe")
                || n.starts_with(&format!("{prefix}-x86_64-"))
                || n.starts_with(&format!("{prefix}-aarch64-"))
        }) {
            return Ok(path);
        }
    }
    Err(err("安装包中未找到客户端程序"))
}

#[cfg(any(windows, test))]
fn ps_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
#[cfg(any(windows, test))]
fn encoded_ps(script: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(
        script
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    )
}
#[cfg(any(windows, test))]
fn desktop_script(target: Target, path: &Path, dir: &Path, verify_only: bool) -> Result<String> {
    let product = match target {
        Target::ClaudeDesktop => "claude",
        Target::CodexDesktop => "codex",
        _ => return Err(err("此安装步骤仅适用于桌面版")),
    };
    Ok(format!(
        "& {{\n{}\n}} -Product {product} -PackagePath {} -LicensePath {} -ResultPath {} {}",
        include_str!("install_desktop.ps1"),
        ps_literal(&path.to_string_lossy()),
        ps_literal(&dir.join("ChatGPT-License.xml").to_string_lossy()),
        ps_literal(&dir.join("install-result.json").to_string_lossy()),
        if verify_only { "-VerifyOnly" } else { "" }
    ))
}

#[cfg(windows)]
fn install_windows_desktop(
    target: Target,
    path: &Path,
    dir: &Path,
    expected_version: &str,
) -> Result<Outcome> {
    use std::process::Command;
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let powershell = PathBuf::from(root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let verify = crate::process::output(
        Command::new(&powershell).args([
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
            &encoded_ps(&desktop_script(target, path, dir, true)?),
        ]),
        Duration::from_secs(120),
    )?;
    if !verify.status.success() {
        return Err(err(format!(
            "安装包检查未通过：{}",
            String::from_utf8_lossy(&verify.stderr).trim()
        )));
    }
    let verified: serde_json::Value = serde_json::from_slice(&verify.stdout)?;
    if verified["version"].as_str() != Some(expected_version) {
        return Err(err("安装包版本与检查结果不一致，请重新检查更新。"));
    }
    ensure_newer(target, expected_version)?;
    let payload = encoded_ps(&desktop_script(target, path, dir, false)?);
    // The fixed script rechecks the locked package after elevation. No policy bypass,
    // forced app shutdown, uninstall, or automatic Windows restart is performed.
    let bootstrap = format!("$ErrorActionPreference='Stop'; try {{ $p=Start-Process -FilePath {} -Verb RunAs -WindowStyle Hidden -ArgumentList @('-NoProfile','-NonInteractive','-EncodedCommand','{payload}') -Wait -PassThru; exit $p.ExitCode }} catch {{ if ($_.Exception.NativeErrorCode -eq 1223 -or $_.Exception.InnerException.NativeErrorCode -eq 1223) {{ exit 1223 }}; [Console]::Error.WriteLine($_.Exception.Message); exit 1 }}", ps_literal(&powershell.to_string_lossy()));
    let result = crate::process::output(
        Command::new(&powershell).args(["-NoProfile", "-NonInteractive", "-Command", &bootstrap]),
        Duration::from_secs(15 * 60),
    )?;
    if result.status.code() == Some(1223) {
        return Err(err("安装未获管理员批准，请允许系统授权后重试。"));
    }
    let data = fs::read_to_string(dir.join("install-result.json")).unwrap_or_default();
    let status: serde_json::Value =
        serde_json::from_str(data.trim_start_matches('\u{feff}')).unwrap_or_default();
    if !result.status.success() || status["success"] != true {
        let dispatch_error = String::from_utf8_lossy(&result.stderr);
        let detail = status["error"]
            .as_str()
            .unwrap_or(if dispatch_error.trim().is_empty() { "系统安装未完成，请查看安装目录中的结果记录。" } else { dispatch_error.trim() });
        return Err(err(format!("系统安装失败：{detail}")));
    }
    Ok(Outcome {
        installed: false,
        message: "部署完成。请保存工作，重新登录 Windows 后检测，无需重装。",
    })
}

fn install(target: Target, asset: &Asset, path: &Path, dir: &Path) -> Result<Outcome> {
    if let Some(hash) = &asset.sha256 {
        verify_hash(path, hash)?;
    }
    if matches!(target, Target::ClaudeCli | Target::CodexCli) {
        let binary = if asset.kind == "binary" {
            path.into()
        } else {
            let out = dir.join("bin");
            unpack(path, &out, asset.kind)?;
            find_binary(&out, "codex")?
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))?;
        }
        let result = crate::process::output(
            std::process::Command::new(&binary).arg("--version"),
            Duration::from_secs(15),
        )?;
        if !result.status.success()
            || crate::client_setup::target_version(target, &String::from_utf8_lossy(&result.stdout))
                .is_none()
        {
            return Err(err("客户端版本验证失败，未替换当前程序。"));
        }
        if crate::client_setup::target_version(target, &String::from_utf8_lossy(&result.stdout))
            .as_deref()
            != Some(asset.version.as_str())
        {
            return Err(err("安装包版本与检查结果不一致，请重新检查更新。"));
        }
        ensure_newer(target, &asset.version)?;
        crate::launcher::use_installed_client(target, &binary)?;
        if asset.kind != "binary" {
            let _ = fs::remove_file(path);
        }
        return Ok(Outcome {
            installed: true,
            message: "安装完成，客户端已可使用。",
        });
    }
    #[cfg(windows)]
    {
        return install_windows_desktop(target, path, dir, &asset.version);
    }
    #[cfg(target_os = "macos")]
    {
        let open_path = if asset.kind == "app-zip" {
            let out = dir.join("application");
            fs::create_dir(&out)?;
            let result = crate::process::output(
                std::process::Command::new("/usr/bin/ditto")
                    .args(["-x", "-k"])
                    .arg(path)
                    .arg(&out),
                Duration::from_secs(180),
            )?;
            if !result.status.success() {
                return Err(err("安装包解压失败"));
            }
            let names: &[&str] = if target == Target::ClaudeDesktop {
                &["Claude.app"]
            } else {
                &["ChatGPT.app", "Codex.app"]
            };
            let app = names
                .iter()
                .map(|name| out.join(name))
                .find(|p| p.is_dir())
                .ok_or_else(|| err("安装包中未找到桌面应用"))?;
            let expected_id = if target == Target::ClaudeDesktop {
                "com.anthropic.claudefordesktop"
            } else {
                "com.openai.codex"
            };
            let identity = crate::process::output(
                std::process::Command::new("/usr/libexec/PlistBuddy")
                    .args(["-c", "Print :CFBundleIdentifier"])
                    .arg(app.join("Contents/Info.plist")),
                Duration::from_secs(10),
            )?;
            if !identity.status.success()
                || String::from_utf8_lossy(&identity.stdout).trim() != expected_id
            {
                return Err(err("安装包应用身份验证失败"));
            }
            let version = crate::process::output(
                std::process::Command::new("/usr/libexec/PlistBuddy")
                    .args(["-c", "Print :CFBundleShortVersionString"])
                    .arg(app.join("Contents/Info.plist")),
                Duration::from_secs(10),
            )?;
            if !version.status.success()
                || String::from_utf8_lossy(&version.stdout).trim() != asset.version
            {
                return Err(err("安装包版本与检查结果不一致，请重新检查更新。"));
            }
            ensure_newer(target, &asset.version)?;
            let check = crate::process::output(
                std::process::Command::new("/usr/sbin/spctl")
                    .args(["--assess", "--type", "execute"])
                    .arg(&app),
                Duration::from_secs(60),
            )?;
            if !check.status.success() {
                return Err(err("安装包签名验证失败"));
            }
            out
        } else {
            let check = crate::process::output(
                std::process::Command::new("/usr/sbin/spctl")
                    .args([
                        "--assess",
                        "--type",
                        "open",
                        "--context",
                        "context:primary-signature",
                    ])
                    .arg(path),
                Duration::from_secs(60),
            )?;
            if !check.status.success() {
                return Err(err("安装包签名验证失败"));
            }
            path.into()
        };
        let opened = crate::process::output(std::process::Command::new("/usr/bin/open")
            .arg(open_path), Duration::from_secs(15))?;
        if !opened.status.success() { return Err(err("无法打开安装文件夹，请检查系统权限后重试。")); }
        if asset.kind == "app-zip" {
            return Ok(Outcome {
                installed: false,
                message: "将应用拖入「应用程序」，再返回检测。",
            });
        }
    }
    #[cfg(not(windows))]
    Ok(Outcome {
        installed: false,
        message: "按系统提示完成安装。",
    })
}

#[tauri::command]
pub async fn install_client(target: Target, on_progress: Channel<Progress>) -> Result<Outcome> {
    crate::updates::ensure_supported()?;
    let _lock = INSTALL
        .try_lock()
        .map_err(|_| err("另一个客户端正在安装，请稍候。"))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| err("系统时间无效"))?
        .as_nanos();
    let dir = crate::paths::app_dir()?
        .join("clients")
        .join(stamp.to_string());
    fs::create_dir_all(&dir)?;
    report(&on_progress, "resolve", 0, None);
    let install_dir = dir.clone();
    let mut installation_started = false;
    let result = async {
        let asset = resolve(target, &dir, &on_progress).await?;
        // Re-read actual installed state. UI state is never sufficient authorization
        // for reinstalling an equal version, downgrading, or replacing an unreadable client.
        crate::commands::blocking({
            let version = asset.version.clone();
            move || ensure_newer(target, &version)
        })
        .await?;
        let path = dir.join(asset.filename);
        download(&asset.url, &path, LIMIT, &on_progress).await?;
        if cfg!(windows) && target == Target::CodexDesktop {
            download(
                "https://persistent.oaistatic.com/codex-app-prod/ChatGPT-License.xml",
                &dir.join("ChatGPT-License.xml"),
                1024 * 1024,
                &on_progress,
            )
            .await?;
        }
        report(&on_progress, "verify", 0, None);
        installation_started = true;
        let progress = on_progress.clone();
        crate::commands::blocking(move || {
            ensure_newer(target, &asset.version)?;
            if !matches!(target, Target::ClaudeCli | Target::CodexCli) {
                report(&progress, "install", 0, None);
            }
            install(target, &asset, &path, &install_dir)
        })
        .await
    }
    .await;
    // Only this attempt's generated directory is removed. Existing clients remain intact.
    if result.is_err()
        && (!installation_started || matches!(target, Target::ClaudeCli | Target::CodexCli))
    {
        let _ = fs::remove_dir_all(&dir);
    }
    let outcome = result?;
    report(
        &on_progress,
        if outcome.installed {
            "complete"
        } else {
            "installer"
        },
        0,
        None,
    );
    Ok(outcome)
}

fn ensure_newer(target: Target, candidate: &str) -> Result<()> {
    if let Some((current, _)) = crate::launcher::installed_version(target)? {
        if !crate::client_setup::newer_version(candidate, &current)? {
            return Err(err("当前已是最新版或更高版本，无需安装。"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn architecture_names_select_only_supported_packages() {
        for value in ["ARM64", "aarch64"] { assert_eq!(normalize_arch(value).unwrap(), "arm64"); }
        for value in ["AMD64", "x86_64", "x64"] { assert_eq!(normalize_arch(value).unwrap(), "x64"); }
        for value in ["x86", "i686", "arm", "", "riscv64"] { assert!(normalize_arch(value).is_err()); }
    }
    #[test]
    fn manifest_directory_supports_zip_and_zip64() {
        let name = b"AppxManifest.xml";
        let mut entry = vec![0u8; 46];
        entry[..4].copy_from_slice(b"PK\x01\x02");
        entry[10..12].copy_from_slice(&8u16.to_le_bytes());
        entry[20..24].copy_from_slice(&2931u32.to_le_bytes());
        entry[24..28].copy_from_slice(&12308u32.to_le_bytes());
        entry[28..30].copy_from_slice(&(name.len() as u16).to_le_bytes());
        entry[42..46].copy_from_slice(&298876483u32.to_le_bytes());
        entry.extend_from_slice(name);
        assert_eq!(manifest_entry(&entry).unwrap(), (298876483, 2931, 8));
        entry[20..28].fill(255);
        entry[42..46].fill(255);
        entry[30..32].copy_from_slice(&28u16.to_le_bytes());
        entry.extend_from_slice(&1u16.to_le_bytes());
        entry.extend_from_slice(&24u16.to_le_bytes());
        for value in [12308u64, 2931, 298876483] {
            entry.extend_from_slice(&value.to_le_bytes());
        }
        assert_eq!(manifest_entry(&entry).unwrap(), (298876483, 2931, 8));
        entry.pop();
        assert!(manifest_entry(&entry).is_err());
        assert!(manifest_entry(b"not a directory").is_err());
    }
    #[test]
    fn validates_content_range_bounds() {
        assert_eq!(
            content_range("bytes 0-0/299362172"),
            Some((0, 0, 299362172))
        );
        for value in ["bytes 2-1/3", "bytes 0-3/3", "bytes */3", "bytes 0-0/*"] {
            assert!(content_range(value).is_none());
        }
    }
    #[test]
    #[ignore = "explicit live read-only version metadata probe"]
    fn live_desktop_metadata() {
        tauri::async_runtime::block_on(async {
            for target in [Target::ClaudeDesktop, Target::CodexDesktop] {
                let version = latest_available_version(target).await;
                println!("{target:?}: {version:?}");
                assert!(version.is_ok());
            }
        });
    }
    #[test]
    fn mac_feed_uses_full_versioned_archive() {
        let (version,url)=mac_codex_release(br#"<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"><item><sparkle:shortVersionString>26.1111.21308</sparkle:shortVersionString><enclosure url="https://persistent.oaistatic.com/codex-app-prod/ChatGPT-darwin-arm64-26.1111.21308.zip" /></item></rss>"#).unwrap();
        assert_eq!(version, "26.1111.21308");
        assert!(url.ends_with("26.1111.21308.zip"));
        assert!(mac_codex_release(b"<rss/>").is_err());
    }
    #[cfg(windows)]
    #[test]
    fn desktop_scripts_parse_and_quote_user_paths_without_execution() {
        let path = Path::new("C:\\用户\\it's & $(exit 91)\\client.msix");
        let dir = path.parent().unwrap();
        let exe = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        for target in [Target::ClaudeDesktop, Target::CodexDesktop] {
            for verify_only in [false, true] {
                let script = desktop_script(target, path, dir, verify_only).unwrap();
                assert!(script.contains("it''s & $(exit 91)"));
                let encoded = encoded_ps(&script);
                let parser = format!("$s=[Text.Encoding]::Unicode.GetString([Convert]::FromBase64String('{encoded}')); $tokens=$null; $errors=$null; $null=[System.Management.Automation.Language.Parser]::ParseInput($s,[ref]$tokens,[ref]$errors); if ($errors.Count) {{ $errors | Out-String | Write-Output; exit 1 }}");
                let result = crate::process::output(
                    std::process::Command::new(&exe).args([
                        "-NoProfile",
                        "-NonInteractive",
                        "-Command",
                        &parser,
                    ]),
                    Duration::from_secs(15),
                )
                .unwrap();
                assert!(
                    result.status.success(),
                    "{}",
                    String::from_utf8_lossy(&result.stdout)
                );
            }
        }
        assert!(desktop_script(Target::CodexCli, path, dir, false).is_err());
    }
    #[test]
    fn rejects_untrusted_install_urls() {
        for url in [
            "http://downloads.claude.ai/releases/x",
            "https://downloads.claude.ai.evil.com/releases/x",
            "https://user@downloads.claude.ai/releases/x",
            "https://github.com/other/repo/releases/download/x",
            "https://127.0.0.1/x",
        ] {
            assert!(!allowed(&reqwest::Url::parse(url).unwrap()));
        }
        assert!(allowed(
            &reqwest::Url::parse("https://downloads.claude.ai/claude-code-releases/latest")
                .unwrap()
        ));
    }
    #[test]
    fn relay_preserves_upstream_url_as_data() {
        let source = "https://github.com/openai/codex/releases/download/v1/a?x=1&y=2";
        let url = relay_url("https://relay.example/fetch", source);
        let encoded = url.split("url=").nth(1).unwrap();
        assert_eq!(
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(encoded)
                .unwrap(),
            source.as_bytes()
        );
    }
    #[test]
    fn integrity_failure_prevents_installation() {
        let dir = std::env::temp_dir().join(format!("keytool-hash-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("payload");
        fs::write(&path, b"abc").unwrap();
        assert!(verify_hash(
            &path,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        )
        .is_ok());
        assert!(verify_hash(&path, &"0".repeat(64)).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn archive_rejects_paths_outside_install_directory() {
        let dir = std::env::temp_dir().join(format!("keytool-archive-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("payload.zip");
        let mut archive = zip::ZipWriter::new(File::create(&path).unwrap());
        archive
            .start_file("../escaped.exe", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"not an executable").unwrap();
        archive.finish().unwrap();
        assert!(unpack(&path, &dir.join("bin"), "zip").is_err());
        assert!(!dir.join("escaped.exe").exists());
        fs::remove_dir_all(dir).unwrap();
    }
    fn serve_once(
        status: &str,
        body: &str,
        declared: usize,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\n{body}"
        );
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 8192];
            let _ = stream.read(&mut request);
            stream.write_all(response.as_bytes()).unwrap();
        });
        (url, handle)
    }
    #[test]
    fn reachable_official_downloads_directly_otherwise_relay() {
        tauri::async_runtime::block_on(async {
            // Official responds: go direct
            let (url, handle) = serve_once("200 OK", "x", 1);
            assert_eq!(pick(&url, "relay".into()).await, (url.clone(), false));
            handle.join().unwrap();
            // Official unreachable or returning an error: use the relay
            assert_eq!(pick("http://127.0.0.1:1", "relay".into()).await, ("relay".into(), true));
            let (url, handle) = serve_once("503 Unavailable", "", 0);
            assert_eq!(pick(&url, "relay".into()).await, ("relay".into(), true));
            handle.join().unwrap();
        });
    }
    #[test]
    fn incomplete_download_fails_and_leaves_no_file() {
        tauri::async_runtime::block_on(async {
            let dir = std::env::temp_dir().join(format!("keytool-route-{}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            let path = dir.join("payload");
            let channel = Channel::new(|_| Ok(()));
            let (url, handle) = serve_once("200 OK", "complete", 8);
            download_from(&url, false, &path, 100, &channel).await.unwrap();
            handle.join().unwrap();
            assert_eq!(fs::read(&path).unwrap(), b"complete");
            let (url, handle) = serve_once("200 OK", "partial", 30);
            let error = download_from(&url, true, &path, 100, &channel).await.unwrap_err();
            handle.join().unwrap();
            assert!(error.to_string().contains("中继下载未完成"));
            assert!(!path.exists());
            fs::remove_dir_all(dir).unwrap();
        });
    }
}
