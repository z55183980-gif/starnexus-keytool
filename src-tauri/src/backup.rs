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
/// 桌面版自己的应用配置（官方登录 / 第三方模式的选择记在这里）
const DESKTOP_APP: &str = "claude-desktop-app.json";

/// Resolve trusted locations without reading another client's configuration.
fn all_targets() -> Result<Vec<(String, PathBuf)>> {
    let mut files = targets(true, true)?;
    files.push(("codex-ownership.json".into(), paths::ownership_path()?));
    files.push((DESKTOP_META.into(), crate::claude_desktop::meta_path()?));
    files.push((DESKTOP_APP.into(), crate::claude_desktop::app_config_path()?));
    Ok(files)
}

pub fn create() -> Result<String> {
    create_scoped(true, true)
}

pub fn create_scoped(codex: bool, claude: bool) -> Result<String> {
    create_at(&paths::backup_root()?, &targets(codex, claude)?)
}

pub fn create_for_paths(changed: &[PathBuf]) -> Result<String> {
    create_for_paths_at(&paths::backup_root()?, changed, &all_targets()?)
}

fn desktop_path_allowed(path: &Path, allowed: &[(String, PathBuf)]) -> bool {
    let Some((_, meta)) = allowed.iter().find(|(n, _)| n == DESKTOP_META) else { return false };
    path.parent() == meta.parent()
        && path.extension().is_some_and(|e| e == "json")
        && path.file_stem().and_then(|n| n.to_str()).is_some_and(crate::claude_desktop::valid_id)
        && !fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

fn create_for_paths_at(root: &Path, changed: &[PathBuf], allowed: &[(String, PathBuf)]) -> Result<String> {
    let mut files = Vec::new();
    for path in changed {
        let name = allowed.iter().find(|(_, p)| p == path).map(|(n, _)| n.as_str())
            .or_else(|| desktop_path_allowed(path, allowed).then_some(DESKTOP_CONFIG))
            .ok_or_else(|| AppError::Other("无法备份未知配置路径，未修改文件".into()))?;
        if files.iter().any(|(n, _)| n == name) {
            return Err(AppError::Other("备份包含重复文件".into()));
        }
        files.push((name.to_string(), path.clone()));
    }
    create_at(root, &files)
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
    restore_at(&paths::backup_root()?, id, &all_targets()?, crate::state::forget_desktop_key)
}

fn restore_at(root: &Path, id: &str, allowed: &[(String, PathBuf)], before_desktop_restore: impl FnOnce() -> Result<()>) -> Result<()> {
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
                .or_else(|| (file.name == DESKTOP_CONFIG && desktop_path_allowed(&file.target, allowed)).then_some(&file.target))
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
    if plan.iter().any(|(name, _, _)| matches!(name.as_str(), "config.toml" | "auth.json"))
        && !plan.iter().any(|(name, _, _)| name == "codex-ownership.json") {
        if let Some((name, path)) = allowed.iter().find(|(name, _)| name == "codex-ownership.json") {
            plan.push((name.clone(), path.clone(), None));
        }
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
    // Invalidate before replacing files. If invalidation fails, leave configuration untouched.
    // A failed commit may lose a cached match, but must never misidentify a restored credential.
    if plan.iter().any(|(name, _, _)| name == DESKTOP_CONFIG || name == DESKTOP_META) {
        before_desktop_restore()?;
    }
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

    fn desktop_in(d: &TestDir) -> (Vec<(String, PathBuf)>, PathBuf, PathBuf) {
        let library = d.0.join("desktop/configLibrary");
        fs::create_dir_all(&library).unwrap();
        let meta = library.join("_meta.json");
        let config = library.join("8eb03636-36f8-4d79-9b9a-3fe9f40135df.json");
        let mut allowed = targets_in(d);
        allowed.push((DESKTOP_META.into(), meta.clone()));
        (allowed, meta, config)
    }

    #[test]
    fn codex_backup_and_restore_ignore_unreadable_desktop_metadata() {
        let d = TestDir::new();
        let (allowed, meta, _) = desktop_in(&d);
        fs::write(&meta, [0xff, 0xfe]).unwrap();
        let cfg = &allowed[0].1;
        fs::write(cfg, "before").unwrap();
        let root = d.0.join("backups");
        let id = create_for_paths_at(&root, std::slice::from_ref(cfg), &allowed).unwrap();
        fs::write(cfg, "after").unwrap();
        restore_at(&root, &id, &allowed, || panic!("unrelated restore must not touch desktop state")).unwrap();
        assert_eq!(fs::read_to_string(cfg).unwrap(), "before");
        assert_eq!(fs::read(meta).unwrap(), [0xff, 0xfe]);
    }

    #[test]
    fn desktop_undo_and_redo_work_with_absent_metadata_and_invalidate_identity() {
        let d = TestDir::new();
        let (allowed, meta, config) = desktop_in(&d);
        let root = d.0.join("backups");
        let id = create_for_paths_at(&root, &[config.clone(), meta.clone()], &allowed).unwrap();
        fs::write(&config, "sealed-key-A").unwrap();
        fs::write(&meta, "selected-A").unwrap();
        let mut record = Some("key-B");
        restore_at(&root, &id, &allowed, || { record = None; Ok(()) }).unwrap();
        assert_eq!(record, None);
        assert!(!config.exists() && !meta.exists());
        let redo = fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).find(|n| n != &id).unwrap();
        restore_at(&root, &redo, &allowed, || Ok(())).unwrap();
        assert_eq!(fs::read_to_string(config).unwrap(), "sealed-key-A");
        assert_eq!(fs::read_to_string(meta).unwrap(), "selected-A");
    }

    #[test]
    fn restoring_old_ciphertext_clears_new_key_record_before_commit() {
        let d = TestDir::new();
        let (allowed, meta, config) = desktop_in(&d);
        let root = d.0.join("backups");
        fs::write(&config, "__sealed__:v1:old-A").unwrap();
        let id = create_for_paths_at(&root, std::slice::from_ref(&config), &allowed).unwrap();
        fs::write(&config, "plaintext-B").unwrap();
        // Current selection may have changed or metadata may be broken; original target is still valid.
        fs::write(&meta, "different-selection").unwrap();
        let mut state = crate::state::AppState {
            claude_desktop_key: Some(crate::state::DesktopKeyRecord { profile_id: "B".into(), key_hash: crate::state::key_hash("plaintext-B"), sealed: None, ..Default::default() }),
            ..Default::default()
        };
        restore_at(&root, &id, &allowed, || {
            assert_eq!(fs::read_to_string(&config).unwrap(), "plaintext-B");
            state.claude_desktop_key = None;
            Ok(())
        }).unwrap();
        assert!(state.claude_desktop_key.is_none());
        assert_eq!(fs::read_to_string(&config).unwrap(), "__sealed__:v1:old-A");
        fs::write(&config, "plaintext-B").unwrap();
        assert!(restore_at(&root, &id, &allowed, || Err(AppError::Io("state locked".into()))).is_err());
        assert_eq!(fs::read_to_string(config).unwrap(), "plaintext-B");
    }

    #[test]
    fn desktop_restore_rejects_paths_outside_library_and_unknown_files() {
        let d = TestDir::new();
        let (allowed, _, config) = desktop_in(&d);
        let root = d.0.join("backups");
        let outside = d.0.join(config.file_name().unwrap());
        fs::write(&outside, "keep").unwrap();
        let id = create_at(&root, &[(DESKTOP_CONFIG.into(), outside.clone())]).unwrap();
        assert!(restore_at(&root, &id, &allowed, || Ok(())).is_err());
        assert_eq!(fs::read_to_string(outside).unwrap(), "keep");
        assert!(create_for_paths_at(&root, &[config.parent().unwrap().join("arbitrary.json")], &allowed).is_err());
    }

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
        restore_at(&root, &id, &files, || Ok(())).unwrap();
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
        restore_at(&root, "20260101-120000", &files, || Ok(())).unwrap();
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
        assert!(restore_at(&root, &id, &moved, || Ok(())).is_err());
        fs::remove_file(root.join(&id).join("config.toml")).unwrap();
        assert!(restore_at(&root, &id, &files, || Ok(())).is_err());
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
        restore_at(&root, &id, &files, || Ok(())).unwrap();
        assert_eq!(fs::read_to_string(&files[0].1).unwrap(), "oldest");
    }
}
