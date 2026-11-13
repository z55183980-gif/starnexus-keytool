use crate::error::{AppError, Result};
use std::path::PathBuf;

/// Codex 配置目录。
/// 必须优先读 CODEX_HOME —— 硬编码 ~/.codex 会在改过环境变量的用户上翻车。
pub fn codex_home() -> Result<PathBuf> {
    if let Ok(v) = std::env::var("CODEX_HOME") {
        let v = v.trim();
        if !v.is_empty() {
            return Ok(std::path::absolute(v)?);
        }
    }
    let home =
        dirs::home_dir().ok_or_else(|| AppError::CodexHomeMissing("无法确定用户主目录".into()))?;
    Ok(home.join(".codex"))
}

pub fn config_path() -> Result<PathBuf> {
    Ok(codex_home()?.join("config.toml"))
}

pub fn auth_path() -> Result<PathBuf> {
    Ok(codex_home()?.join("auth.json"))
}

/// Claude Code 配置目录。
/// 同样优先读 CLAUDE_CONFIG_DIR —— 不少用户会改它。
pub fn claude_home() -> Result<PathBuf> {
    if let Ok(v) = std::env::var("CLAUDE_CONFIG_DIR") {
        let v = v.trim();
        if !v.is_empty() {
            return Ok(std::path::absolute(v)?);
        }
    }
    let home = dirs::home_dir().ok_or_else(|| AppError::Other("无法确定用户主目录".into()))?;
    Ok(home.join(".claude"))
}

pub fn claude_settings_path() -> Result<PathBuf> {
    Ok(claude_home()?.join("settings.json"))
}

/// 备份根目录，放在 Codex 目录下，用户自己也能找到
pub fn backup_root() -> Result<PathBuf> {
    Ok(codex_home()?.join(".starnexus-keytool-backups"))
}

/// 本工具自己的状态目录（记住上次选的模型、备份索引）
pub fn app_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| AppError::Other("无法确定用户主目录".into()))?;
    Ok(home.join(".starnexus-keytool"))
}

pub fn state_path() -> Result<PathBuf> {
    Ok(app_dir()?.join("state.json"))
}
