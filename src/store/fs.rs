use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use anyhow::{Context, Result};

pub fn restrict_private(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(path, perms)
            .with_context(|| format!("chmod 0600 {}", path.display()))?;
    }
    let _ = path;
    Ok(())
}

pub fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }

    let tmp = path.with_extension("json.tmp");
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)
            .with_context(|| format!("open {}", tmp.display()))?;
        file.write_all(contents)
            .with_context(|| format!("write {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("fsync {}", tmp.display()))?;
    }
    restrict_private(&tmp)?;

    if path.exists() {
        fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
    }
    fs::rename(&tmp, path).with_context(|| format!("rename onto {}", path.display()))?;
    restrict_private(path)?;
    Ok(())
}

pub fn append_jsonl(path: &Path, line: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("append {}", path.display()))?;
        file.write_all(line.as_bytes())
            .with_context(|| format!("write {}", path.display()))?;
        if !line.ends_with('\n') {
            file.write_all(b"\n")?;
        }
        file.sync_all()
            .with_context(|| format!("fsync {}", path.display()))?;
    }
    restrict_private(path)?;
    Ok(())
}

pub fn write_secret(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    {
        let mut file = File::create(path).with_context(|| format!("create {}", path.display()))?;
        file.write_all(contents.trim().as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    }
    restrict_private(path)?;
    Ok(())
}

pub fn is_world_readable(path: &Path) -> io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path)?.permissions().mode();
        return Ok(mode & 0o077 != 0);
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    #[cfg(unix)]
    fn restrict_private_clears_group_and_other() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("secret");
        fs::write(&path, b"x").unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&path, perms).unwrap();
        assert!(is_world_readable(&path).unwrap());
        restrict_private(&path).unwrap();
        assert!(!is_world_readable(&path).unwrap());
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn atomic_write_tmp_is_removed() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.json");
        atomic_write(&path, b"{\"ok\":true}").unwrap();
        assert!(path.is_file());
        assert!(!path.with_extension("json.tmp").exists());
        #[cfg(unix)]
        {
            assert!(!is_world_readable(&path).unwrap());
        }
    }

    #[test]
    fn append_jsonl_writes_lines() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("turns.jsonl");
        append_jsonl(&path, "{\"event\":\"start\"}").unwrap();
        append_jsonl(&path, "{\"event\":\"end\"}\n").unwrap();
        let body = fs::read_to_string(&path).unwrap();
        assert_eq!(body, "{\"event\":\"start\"}\n{\"event\":\"end\"}\n");
    }
}
