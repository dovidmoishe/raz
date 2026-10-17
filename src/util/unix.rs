use std::path::Path;

#[cfg(unix)]
extern "C" {
    fn geteuid() -> u32;
}

pub fn euid() -> Option<u32> {
    #[cfg(unix)]
    {
        Some(unsafe { geteuid() })
    }
    #[cfg(not(unix))]
    {
        None
    }
}

pub fn is_root() -> bool {
    euid() == Some(0)
}

pub fn owns_path(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let Some(uid) = euid() else {
            return false;
        };
        std::fs::metadata(path)
            .map(|meta| meta.uid() == uid)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_check_is_defined() {
        let _ = is_root();
        let _ = euid();
    }
}
