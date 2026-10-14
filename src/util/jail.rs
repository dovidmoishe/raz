use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Result};

pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

pub fn under_root(root: &Path, candidate: &Path) -> Result<PathBuf> {
    let root = normalize(root);
    let joined = if candidate.is_absolute() {
        normalize(candidate)
    } else {
        normalize(&root.join(candidate))
    };
    if !joined.starts_with(&root) {
        bail!(
            "path {} escapes workspace {}",
            joined.display(),
            root.display()
        );
    }
    Ok(joined)
}

#[cfg(test)]
mod tests {
    use super::under_root;
    use std::path::Path;

    #[test]
    fn jail_rejects_parent() {
        let root = Path::new("/var/lib/pocket-code/projects");
        assert!(under_root(root, Path::new("../etc")).is_err());
        assert!(under_root(root, Path::new("shop")).is_ok());
        assert!(under_root(root, Path::new("shop/../other")).is_ok());
        assert!(under_root(root, Path::new("shop/../../etc")).is_err());
    }
}
