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

    if path.exists() {
        fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
    }
    fs::rename(&tmp, path).with_context(|| format!("rename onto {}", path.display()))?;
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
