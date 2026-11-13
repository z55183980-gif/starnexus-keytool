use crate::error::{AppError, Result};
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

pub fn unique_id() -> String {
    format!(
        "{}-{}-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    )
}

/// Only NotFound means absence. Permission and decoding errors must stop writes.
pub fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(AppError::Io(format!("读取 {} 失败：{}", path.display(), e))),
    }
}

pub fn read_text_optional(path: &Path) -> Result<Option<String>> {
    read_optional(path)?
        .map(|bytes| {
            String::from_utf8(bytes).map_err(|_| {
                AppError::Io(format!(
                    "{} 不是有效的 UTF-8 文件，未做修改",
                    path.display()
                ))
            })
        })
        .transpose()
}

pub fn write_atomic(path: &Path, content: &str) -> Result<()> {
    write_atomic_bytes(path, content.as_bytes())
}

/// Same-directory replacement; never remove the destination first on Windows.
/// This protects each file, not an entire transaction against power loss.
pub fn write_atomic_bytes(path: &Path, content: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| AppError::Io("路径没有父目录".into()))?;
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".starnexus-keytool-{}.tmp", unique_id()));
    let result = (|| -> Result<()> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut f = options.open(&tmp)?;
        // Restrict access before any secret is written.
        restrict_permissions(&tmp)?;
        f.write_all(content)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
            .map_err(|e| AppError::Io(format!("替换 {} 失败：{}", path.display(), e)))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn restrict_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let user = std::env::var("USERNAME")
            .map_err(|_| AppError::Io("无法确定当前用户，未写入密钥文件".into()))?;
        let output = std::process::Command::new("icacls")
            .arg(path)
            .args(["/inheritance:r", "/grant:r", &format!("{}:F", user)])
            .creation_flags(0x08000000)
            .output()?;
        if !output.status.success() {
            return Err(AppError::Io("无法限制配置文件访问权限，未写入密钥".into()));
        }
    }
    Ok(())
}

struct Change {
    path: PathBuf,
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
}

/// Stage all changes before committing. Parsing/read failures before commit
/// cannot leave partially written configuration behind.
#[derive(Default)]
pub struct TwoFileTxn {
    changes: Vec<Change>,
}

impl TwoFileTxn {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn paths(&self) -> Vec<PathBuf> {
        self.changes.iter().map(|c| c.path.clone()).collect()
    }
    pub fn write(&mut self, path: &Path, content: &str) -> Result<()> {
        self.set(path, Some(content.as_bytes().to_vec()))
    }
    pub fn set(&mut self, path: &Path, content: Option<Vec<u8>>) -> Result<()> {
        if let Some(change) = self.changes.iter_mut().find(|c| c.path == path) {
            change.after = content;
        } else {
            self.changes.push(Change {
                path: path.into(),
                before: read_optional(path)?,
                after: content,
            });
        }
        Ok(())
    }
    pub fn commit(self) -> Result<()> {
        self.commit_with(replace)
    }
    fn commit_with(self, mut write: impl FnMut(&Path, Option<&[u8]>) -> Result<()>) -> Result<()> {
        for c in &self.changes {
            if read_optional(&c.path)? != c.before {
                return Err(AppError::Io(format!(
                    "{} 已被其他程序修改，请刷新后重试",
                    c.path.display()
                )));
            }
        }
        for (index, c) in self.changes.iter().enumerate() {
            let result = (|| {
                if read_optional(&c.path)? != c.before {
                    return Err(AppError::Io(format!(
                        "{} 已被其他程序修改",
                        c.path.display()
                    )));
                }
                write(&c.path, c.after.as_deref())
            })();
            if let Err(error) = result {
                let mut failures = Vec::new();
                for previous in self.changes[..index].iter().rev() {
                    let restored = (|| {
                        if read_optional(&previous.path)? != previous.after {
                            return Err(AppError::Io("回滚前文件已被其他程序修改".into()));
                        }
                        replace(&previous.path, previous.before.as_deref())
                    })();
                    if restored.is_err() {
                        failures.push(previous.path.display().to_string());
                    }
                }
                return if failures.is_empty() {
                    Err(error)
                } else {
                    Err(AppError::Io(format!(
                        "{}；部分文件回滚失败：{}。请使用备份恢复",
                        error,
                        failures.join("、")
                    )))
                };
            }
        }
        Ok(())
    }
}

fn replace(path: &Path, bytes: Option<&[u8]>) -> Result<()> {
    match bytes {
        Some(bytes) => write_atomic_bytes(path, bytes),
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        },
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub struct TestDir(pub PathBuf);
    impl TestDir {
        pub fn new() -> Self {
            let p = std::env::temp_dir().join(format!("dbkey-test-{}", unique_id()));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn replaces_existing_file() {
        let d = TestDir::new();
        let p = d.0.join("config");
        fs::write(&p, "old").unwrap();
        write_atomic(&p, "new").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "new");
        assert_eq!(fs::read_dir(&d.0).unwrap().count(), 1);
    }
    #[test]
    fn staging_error_and_drop_leave_files_untouched() {
        let d = TestDir::new();
        let p = d.0.join("config");
        fs::write(&p, "old").unwrap();
        let mut txn = TwoFileTxn::new();
        txn.write(&p, "new").unwrap();
        assert!(txn.write(&d.0, "invalid target").is_err());
        drop(txn);
        assert_eq!(fs::read_to_string(p).unwrap(), "old");
    }
    #[test]
    fn later_failure_restores_bytes_and_removes_new_files() {
        let d = TestDir::new();
        let a = d.0.join("a");
        let b = d.0.join("b");
        let c = d.0.join("c");
        fs::write(&a, [0xff, 0x00]).unwrap();
        let mut txn = TwoFileTxn::new();
        txn.write(&a, "new").unwrap();
        txn.write(&b, "created").unwrap();
        txn.write(&c, "fail").unwrap();
        assert!(txn
            .commit_with(|p, bytes| if p == c {
                Err(AppError::Io("injected failure".into()))
            } else {
                replace(p, bytes)
            })
            .is_err());
        assert_eq!(fs::read(a).unwrap(), [0xff, 0x00]);
        assert!(!b.exists());
        assert!(!c.exists());
    }
    #[test]
    fn concurrent_edit_is_not_overwritten() {
        let d = TestDir::new();
        let p = d.0.join("config");
        fs::write(&p, "old").unwrap();
        let mut txn = TwoFileTxn::new();
        txn.write(&p, "new").unwrap();
        fs::write(&p, "external edit").unwrap();
        assert!(txn.commit().is_err());
        assert_eq!(fs::read_to_string(p).unwrap(), "external edit");
    }
    #[cfg(windows)]
    #[test]
    fn locked_destination_survives_failed_replacement() {
        use std::os::windows::fs::OpenOptionsExt;
        let d = TestDir::new();
        let p = d.0.join("config");
        fs::write(&p, "old").unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&p)
            .unwrap();
        assert!(write_atomic(&p, "new").is_err());
        drop(lock);
        assert_eq!(fs::read_to_string(p).unwrap(), "old");
        assert_eq!(fs::read_dir(&d.0).unwrap().count(), 1);
    }
}
