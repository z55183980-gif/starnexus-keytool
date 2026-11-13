use crate::atomic::{self, TwoFileTxn};
use crate::error::{AppError, Result};
use crate::paths;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

const KEEP: usize = 10;
static CONFIG_LOCK: Mutex<()> = Mutex::new(());

pub fn lock() -> Result<MutexGuard<'static, ()>> {
    CONFIG_LOCK
        .lock()
        .map_err(|_| AppError::Other("配置操作锁不可用，请重启本工具".into()))
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupEntry {
    pub id: String,
    pub created_at: String,
    pub has_config: bool,
    pub has_auth: bool,
    pub has_claude: bool,
    pub has_claude_desktop: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    version: u32,
    files: Vec<Snapshot>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Snapshot {
    name: String,
    target: PathBuf,
    existed: bool,
}

fn targets(codex: bool, claude: bool) -> Result<Vec<(String, PathBuf)>> {
    let mut files = Vec::new();
    if codex {
        files.push(("config.toml".into(), paths::config_path()?));
        files.push(("auth.json".into(), paths::auth_path()?));
    }
    if claude {
        files.push((
            "claude-settings.json".into(),
            paths::claude_settings_path()?,
        ));
    }
    Ok(files)
}

const DESKTOP_CONFIG: &str = "claude-desktop.json";
const DESKTOP_META: &str = "claude-desktop-meta.json";

/// Claude 桌面版：配置库索引 + 当前选中的那份配置（配置库还没建时只有索引）
fn desktop_targets() -> Result<Vec<(String, PathBuf)>> {
    let mut files = vec![(DESKTOP_META.into(), crate::claude_desktop::meta_path()?)];
    if let crate::claude_desktop::Applied::Config(path) = crate::claude_desktop::applied()? {
        files.push((DESKTOP_CONFIG.into(), path));
    }
    Ok(files)
}

fn all_targets() -> Result<Vec<(String, PathBuf)>> {
    let mut files = targets(true, true)?;
    files.extend(desktop_targets()?);
    Ok(files)
}

pub fn create() -> Result<String> {
    create_scoped(true, true)
}

pub fn create_scoped(codex: bool, claude: bool) -> Result<String> {
    create_at(&paths::backup_root()?, &targets(codex, claude)?)
}

pub fn create_for_paths(changed: &[PathBuf]) -> Result<String> {
    let mut files = all_targets()?
        .into_iter()
        .filter(|(_, path)| changed.contains(path))
        .collect::<Vec<_>>();
    // 桌面版的配置库是这次才新建的：新配置文件还不在索引里，按所在目录认
    let library = crate::claude_desktop::library_dir()?;
    for path in changed {
        if !files.iter().any(|(_, p)| p == path)
            && path.parent() == Some(library.as_path())
            && !files.iter().any(|(n, _)| n == DESKTOP_CONFIG)
        {
            files.push((DESKTOP_CONFIG.into(), path.clone()));
        }
    }
    create_at(&paths::backup_root()?, &files)
}

fn create_at(root: &Path, files: &[(String, PathBuf)]) -> Result<String> {
    // Read everything before publishing a backup. Missing and unreadable differ.
    let contents = files
        .iter()
        .map(|(name, target)| Ok((name.clone(), target.clone(), atomic::read_optional(target)?)))
        .collect::<Result<Vec<_>>>()?;
    fs::create_dir_all(root)?;
    let id = format!(
        "{}-{}",
        chrono::Local::now().format("%Y%m%d-%H%M%S"),
        atomic::unique_id()
    );
    let staging = root.join(format!(".pending-{}", id));
    fs::create_dir(&staging)?;
    let result = (|| {
        let mut manifest = Manifest {
            version: 1,
            files: Vec::new(),
        };
        for (name, target, bytes) in contents {
            if let Some(bytes) = &bytes {
                atomic::write_atomic_bytes(&staging.join(&name), bytes)?;
            }
            manifest.files.push(Snapshot {
                name,
                target,
                existed: bytes.is_some(),
            });
        }
        atomic::write_atomic(
            &staging.join("manifest.json"),
            &serde_json::to_string_pretty(&manifest)?,
        )?;
        fs::rename(&staging, root.join(&id))?;
        Ok(id)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    // Retention is best effort and never invalidates a successfully published backup.
    if result.is_ok() {
        let _ = prune_at(root);
    }
    result
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|c| c.is_ascii_digit() || c == b'-')
}

pub fn list() -> Result<Vec<BackupEntry>> {
    let root = paths::backup_root()?;
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for e in fs::read_dir(root)? {
        let e = e?;
        let id = e.file_name().to_string_lossy().to_string();
        if !e.file_type()?.is_dir() || !valid_id(&id) {
            continue;
        }
        let p = e.path();
        out.push(BackupEntry {
            created_at: format_id(&id),
            id,
            has_config: p.join("config.toml").exists(),
            has_auth: p.join("auth.json").exists(),
            has_claude: p.join("claude-settings.json").exists(),
            has_claude_desktop: p.join(DESKTOP_CONFIG).exists(),
        });
    }
    out.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(out)
}

pub fn restore(id: &str) -> Result<()> {
    let _guard = lock()?;
    restore_at(&paths::backup_root()?, id, &all_targets()?)
}

fn restore_at(root: &Path, id: &str, allowed: &[(String, PathBuf)]) -> Result<()> {
    if !valid_id(id) {
        return Err(AppError::Other("备份编号不合法".into()));
    }
    let dir = root.join(id);
    if !dir.is_dir() {
        return Err(AppError::Other(format!("找不到备份 {}", id)));
    }
    let mut plan = Vec::new();
    if let Some(text) = atomic::read_text_optional(&dir.join("manifest.json"))? {
        let manifest: Manifest = serde_json::from_str(&text)?;
        if manifest.version != 1 {
            return Err(AppError::Other("不支持这个备份版本".into()));
        }
        for file in manifest.files {
            let target = allowed
                .iter()
                .find(|(name, _)| name == &file.name)
                .map(|(_, path)| path)
                .ok_or_else(|| AppError::Other("备份包含未知文件".into()))?;
            // Do not redirect an old backup into a different CODEX_HOME/CLAUDE_CONFIG_DIR.
            if target != &file.target || plan.iter().any(|(name, _, _)| name == &file.name) {
                return Err(AppError::Other(
                    "备份的配置路径与当前环境不一致，或存在重复文件；未恢复".into(),
                ));
            }
            let bytes = if file.existed {
                Some(
                    atomic::read_optional(&dir.join(&file.name))?.ok_or_else(|| {
                        AppError::Other(format!("备份不完整：缺少 {}", file.name))
                    })?,
                )
            } else {
                None
            };
            plan.push((file.name, target.clone(), bytes));
        }
    } else {
        // Old backups have no scope/absence metadata. Only restore files they contain.
        for (name, target) in allowed {
            if let Some(bytes) = atomic::read_optional(&dir.join(name))? {
                plan.push((name.clone(), target.clone(), Some(bytes)));
            }
        }
    }
    if plan.is_empty() {
        return Err(AppError::Other("备份中没有可恢复的文件".into()));
    }
    let mut txn = TwoFileTxn::new();
    for (_, target, bytes) in &plan {
        txn.set(target, bytes.clone())?;
    }
    // Cache source contents before pruning; abort if the safety backup fails.
    let undo_targets = plan
        .iter()
        .map(|(name, target, _)| (name.clone(), target.clone()))
        .collect::<Vec<_>>();
    create_at(root, &undo_targets)?;
    txn.commit()
}

fn prune_at(root: &Path) -> Result<()> {
    let mut all = fs::read_dir(root)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            valid_id(&e.file_name().to_string_lossy())
                && e.file_type().map(|t| t.is_dir()).unwrap_or(false)
        })
        .map(|e| e.path())
        .collect::<Vec<_>>();
    all.sort();
    for old in all.iter().take(all.len().saturating_sub(KEEP)) {
        fs::remove_dir_all(old)?;
    }
    Ok(())
}

fn format_id(id: &str) -> String {
    if id.len() >= 15 && id.as_bytes().get(8) == Some(&b'-') && id.is_ascii() {
        format!(
            "{}-{}-{} {}:{}:{}",
            &id[..4],
            &id[4..6],
            &id[6..8],
            &id[9..11],
            &id[11..13],
            &id[13..15]
        )
    } else {
        id.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atomic::tests::TestDir;

    fn targets_in(d: &TestDir) -> Vec<(String, PathBuf)> {
        vec![
            ("config.toml".into(), d.0.join("config")),
            ("auth.json".into(), d.0.join("auth")),
            ("claude-settings.json".into(), d.0.join("claude")),
        ]
    }

    #[test]
    fn restore_removes_originally_absent_files_and_preserves_other_tool() {
        let d = TestDir::new();
        let files = targets_in(&d);
        let root = d.0.join("backups");
        fs::write(&files[2].1, "claude-before").unwrap();
        let id = create_at(&root, &files[..2]).unwrap();
        fs::write(&files[0].1, "new config").unwrap();
        fs::write(&files[1].1, "new key").unwrap();
        fs::write(&files[2].1, "claude-edited-later").unwrap();
        restore_at(&root, &id, &files).unwrap();
        assert!(!files[0].1.exists());
        assert!(!files[1].1.exists());
        assert_eq!(
            fs::read_to_string(&files[2].1).unwrap(),
            "claude-edited-later"
        );
    }

    #[test]
    fn unique_backups_and_legacy_restore() {
        let d = TestDir::new();
        let files = targets_in(&d);
        let root = d.0.join("backups");
        fs::write(&files[0].1, "old").unwrap();
        let a = create_at(&root, &files).unwrap();
        let b = create_at(&root, &files).unwrap();
        assert_ne!(a, b);
        let legacy = root.join("20260101-120000");
        fs::create_dir(&legacy).unwrap();
        fs::write(legacy.join("config.toml"), "legacy").unwrap();
        fs::write(&files[1].1, "keep").unwrap();
        restore_at(&root, "20260101-120000", &files).unwrap();
        assert_eq!(fs::read_to_string(&files[0].1).unwrap(), "legacy");
        assert_eq!(fs::read_to_string(&files[1].1).unwrap(), "keep");
    }

    #[test]
    fn incomplete_backup_and_changed_paths_do_not_modify_configuration() {
        let d = TestDir::new();
        let files = targets_in(&d);
        let root = d.0.join("backups");
        fs::write(&files[0].1, "old").unwrap();
        let id = create_at(&root, &files).unwrap();
        fs::write(&files[0].1, "current").unwrap();
        let mut moved = files.clone();
        moved[0].1 = d.0.join("other-home");
        assert!(restore_at(&root, &id, &moved).is_err());
        fs::remove_file(root.join(&id).join("config.toml")).unwrap();
        assert!(restore_at(&root, &id, &files).is_err());
        assert_eq!(fs::read_to_string(&files[0].1).unwrap(), "current");
    }

    #[test]
    fn oldest_backup_can_be_restored_even_when_undo_backup_prunes_it() {
        let d = TestDir::new();
        let files = targets_in(&d);
        let root = d.0.join("backups");
        fs::write(&files[0].1, "oldest").unwrap();
        let id = create_at(&root, &files[..1]).unwrap();
        for _ in 1..KEEP {
            fs::write(&files[0].1, "later").unwrap();
            create_at(&root, &files[..1]).unwrap();
        }
        restore_at(&root, &id, &files).unwrap();
        assert_eq!(fs::read_to_string(&files[0].1).unwrap(), "oldest");
    }
}
