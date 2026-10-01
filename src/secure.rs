//! Unix private filesystem primitives. Parents must be owned by this user or root;
//! writable shared parents require the sticky bit. Symlinks are never accepted.
use anyhow::{Result, bail};
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path};

pub fn directory(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        bail!("storage directory must be absolute");
    }
    let mut current = std::path::PathBuf::from("/");
    for part in path.components() {
        match part {
            Component::RootDir => continue,
            Component::Normal(p) => current.push(p),
            _ => bail!("storage path contains unsafe components"),
        }
        match fs::symlink_metadata(&current) {
            Ok(meta) => {
                let uid = unsafe { libc::geteuid() };
                if !meta.is_dir()
                    || (meta.uid() != uid && meta.uid() != 0)
                    || (meta.mode() & 0o022 != 0 && meta.mode() & 0o1000 == 0)
                {
                    bail!("storage parent is symlinked or untrusted");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                fs::DirBuilder::new().mode(0o700).create(&current)?;
            }
            Err(e) => return Err(e.into()),
        }
    }
    let meta = fs::symlink_metadata(path)?;
    if meta.uid() != unsafe { libc::geteuid() } {
        bail!("private storage must be owned by the current user");
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

pub fn file(path: &Path, create: bool) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(create)
        .create(create)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } || meta.nlink() != 1 {
        bail!("private file is not a singly linked owned regular file");
    }
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

pub fn relative(path: &str) -> Result<()> {
    if path.is_empty() || path.len() > 4096 || path.contains(['\n', '\r', '\\']) {
        bail!("unsafe artifact path");
    }
    if path
        .split('/')
        .any(|p| p.is_empty() || p == "." || p == "..")
        || !Path::new(path)
            .components()
            .all(|p| matches!(p, Component::Normal(_)))
    {
        bail!("unsafe artifact path");
    }
    Ok(())
}
