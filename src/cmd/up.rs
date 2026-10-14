use anyhow::Result;

use crate::models::Paths;

pub fn run() -> Result<()> {
    let paths = Paths::resolve()?;
    paths.ensure()?;
    if cfg!(unix) {
        let unit = crate::integrations::systemd::write_user_unit(&paths)?;
        println!(
            "wrote {}\n\nnext:\n  systemctl --user daemon-reload\n  systemctl --user enable --now pocket-code\n",
            unit.display()
        );
    } else {
        println!("systemd is not available here. run:\n  pocket-code serve\n");
    }
    Ok(())
}
