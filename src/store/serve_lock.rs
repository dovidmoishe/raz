use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

use anyhow::{Context, Result, bail};

/// Keep this handle alive for the entire server lifetime. Never unlink the
/// file: replacing it would let another process lock a different file inode.
pub fn acquire_serve_lock(home: &Path) -> Result<File> {
    let path = home.join("serve.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("open server lock {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => bail!(
            "raz serve is already running for {}. Use the existing instance, or stop it before starting another. Do not delete serve.lock; the OS releases the lock when the process exits",
            home.display()
        ),
        Err(TryLockError::Error(err)) => Err(err).with_context(|| {
            format!(
                "lock {}; check directory permissions and filesystem locking support",
                path.display()
            )
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    #[test]
    fn rejects_duplicate_and_reuses_existing_file_after_release() {
        let dir = tempfile::tempdir().unwrap();
        let lock = acquire_serve_lock(dir.path()).unwrap();
        let error = acquire_serve_lock(dir.path()).unwrap_err();
        assert!(error.to_string().contains("already running"));
        assert!(error.to_string().contains("Do not delete"));
        drop(lock);
        assert!(dir.path().join("serve.lock").is_file());
        let _lock = acquire_serve_lock(dir.path()).unwrap();
    }

    #[test]
    fn different_homes_can_run_independently() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let _first = acquire_serve_lock(first.path()).unwrap();
        let _second = acquire_serve_lock(second.path()).unwrap();
    }

    #[test]
    fn lock_is_released_after_process_is_killed() {
        let dir = tempfile::tempdir().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "store::serve_lock::tests::lock_child",
                "--ignored",
                "--nocapture",
            ])
            .env("RAZ_TEST_LOCK_HOME", dir.path())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(
                output.read_line(&mut line).unwrap(),
                0,
                "child exited before locking"
            );
            if line.contains("LOCK_READY") {
                break;
            }
        }
        let blocked = acquire_serve_lock(dir.path()).is_err();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(blocked);
        let _lock = acquire_serve_lock(dir.path()).unwrap();
    }

    #[test]
    #[ignore = "subprocess helper invoked by lock_is_released_after_process_is_killed"]
    fn lock_child() {
        use std::io::Write;
        let Some(home) = std::env::var_os("RAZ_TEST_LOCK_HOME") else {
            return;
        };
        let _lock = acquire_serve_lock(Path::new(&home)).unwrap();
        println!("LOCK_READY");
        std::io::stdout().flush().unwrap();
        std::thread::sleep(std::time::Duration::from_secs(15));
    }
}
