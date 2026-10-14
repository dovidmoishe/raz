use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use crate::models::Paths;
use crate::util::report;

pub fn unit_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("home directory")?;
    Ok(home.join(".config/systemd/user/pocket-code.service"))
}

pub fn write_user_unit(paths: &Paths) -> Result<PathBuf> {
    if !cfg!(unix) {
        bail!("systemd units are only written on Linux/Unix");
    }
    let exe = std::env::current_exe().context("current executable")?;
    let path = unit_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = format!(
        "[Unit]\n\
         Description=pocket-code telegram supervisor\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exe} serve\n\
         Restart=on-failure\n\
         RestartSec=3\n\
         WorkingDirectory={home}\n\
         Environment=POCKET_CODE_HOME={home}\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        exe = exe.display(),
        home = paths.home.display(),
    );
    std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    report::info("systemd", format!("wrote {}", path.display()));
    Ok(path)
}
