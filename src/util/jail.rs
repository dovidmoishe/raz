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
        let root = Path::new("/var/lib/raz/projects");
        assert!(under_root(root, Path::new("../etc")).is_err());
        assert!(under_root(root, Path::new("shop")).is_ok());
        assert!(under_root(root, Path::new("shop/../other")).is_ok());
        assert!(under_root(root, Path::new("shop/../../etc")).is_err());
    }

    #[test]
    fn jail_rejects_absolute_outside_and_keeps_dot() {
        let root = Path::new("/var/lib/raz/projects");
        assert!(under_root(root, Path::new("/etc/passwd")).is_err());
        assert!(under_root(root, Path::new("/var/lib/raz/projects/shop")).is_ok());
        let dot = under_root(root, Path::new(".")).unwrap();
        assert_eq!(dot, super::normalize(root));
        let empty = under_root(root, Path::new("")).unwrap();
        assert_eq!(empty, super::normalize(root));
        let stay = under_root(root, Path::new("shop/src/..")).unwrap();
        assert!(stay.ends_with("shop"));
    }
}
