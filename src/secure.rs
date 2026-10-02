//! Descriptor-relative Unix storage. No operation follows a directory or file
//! symlink, including when a pathname is replaced after an earlier check.
use anyhow::{Context, Result, bail};
use std::ffi::{CStr, CString, OsStr};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path};

pub struct Dir(File);

fn name(value: &OsStr) -> Result<CString> {
    if value.as_bytes().is_empty()
        || value.as_bytes().contains(&b'/')
        || value == "."
        || value == ".."
    {
        bail!("unsafe storage component");
    }
    Ok(CString::new(value.as_bytes())?)
}
fn owned_file(file: File) -> Result<File> {
    let meta = file.metadata()?;
    if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } || meta.nlink() != 1 {
        bail!("private file is not a singly linked owned regular file");
    }
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}
fn check_dir(file: File, private: bool) -> Result<Dir> {
    let meta = file.metadata()?;
    let uid = unsafe { libc::geteuid() };
    if !meta.is_dir()
        || (meta.uid() != uid && meta.uid() != 0)
        || (meta.mode() & 0o022 != 0 && meta.mode() & 0o1000 == 0)
        || (private && meta.uid() != uid)
    {
        bail!("storage parent is symlinked or untrusted");
    }
    if private {
        file.set_permissions(std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(Dir(file))
}
impl Dir {
    pub fn open(path: &Path, create: bool) -> Result<Self> {
        if path
            .as_os_str()
            .as_bytes()
            .split(|b| *b == b'/')
            .any(|part| part == b"." || part == b"..")
        {
            bail!("unsafe storage path component");
        }
        if !path.is_absolute() {
            bail!("storage directory must be absolute");
        }
        let root = CString::new("/").unwrap();
        let fd = unsafe {
            libc::open(
                root.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error().into());
        }
        let mut dir = check_dir(unsafe { File::from_raw_fd(fd) }, false)?;
        let parts: Vec<_> = path
            .components()
            .filter(|c| !matches!(c, Component::RootDir))
            .collect();
        for (index, part) in parts.iter().enumerate() {
            let Component::Normal(part) = part else {
                bail!("unsafe storage path");
            };
            dir = dir.child_inner(part, create, index + 1 == parts.len())?;
        }
        Ok(dir)
    }
    fn child_inner(&self, part: &OsStr, create: bool, private: bool) -> Result<Self> {
        let part = name(part)?;
        if create {
            let result = unsafe { libc::mkdirat(self.0.as_raw_fd(), part.as_ptr(), 0o700) };
            if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists {
                return Err(io::Error::last_os_error().into());
            }
            if result == 0 {
                self.sync()?;
            }
        }
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                part.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error().into());
        }
        check_dir(unsafe { File::from_raw_fd(fd) }, private)
    }
    pub fn child(&self, part: &str, create: bool) -> Result<Self> {
        self.child_inner(OsStr::new(part), create, true)
    }
    pub fn parent(&self, path: &str, create: bool) -> Result<(Self, String)> {
        relative(path)?;
        let mut parts = path.split('/').peekable();
        let mut dir = Self(self.0.try_clone()?);
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                return Ok((dir, part.to_owned()));
            }
            dir = dir.child(part, create)?;
        }
        unreachable!()
    }
    pub fn file(&self, part: &str, create: bool) -> Result<File> {
        self.open_file(
            part,
            if create {
                libc::O_RDWR | libc::O_CREAT
            } else {
                libc::O_RDONLY
            },
        )
    }
    pub fn create_new(&self, part: &str) -> Result<File> {
        self.open_file(part, libc::O_RDWR | libc::O_CREAT | libc::O_EXCL)
    }
    fn open_file(&self, part: &str, flags: i32) -> Result<File> {
        let part = name(OsStr::new(part))?;
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                part.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error().into());
        }
        owned_file(unsafe { File::from_raw_fd(fd) })
    }
    pub fn read(&self, path: &str) -> Result<File> {
        let (dir, leaf) = self.parent(path, false)?;
        dir.file(&leaf, false)
    }
    pub fn optional_file(&self, part: &str) -> Result<Option<File>> {
        match self.file(part, false) {
            Ok(file) => Ok(Some(file)),
            Err(error) if not_found(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }
    pub fn rename(&self, from: &str, dest: &Self, to: &str) -> Result<()> {
        self.rename_unsynced(from, dest, to)?;
        self.sync()?;
        dest.sync()
    }
    pub fn rename_unsynced(&self, from: &str, dest: &Self, to: &str) -> Result<()> {
        let from = name(OsStr::new(from))?;
        let to = name(OsStr::new(to))?;
        if unsafe {
            libc::renameat(
                self.0.as_raw_fd(),
                from.as_ptr(),
                dest.0.as_raw_fd(),
                to.as_ptr(),
            )
        } < 0
        {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }
    pub fn remove(&self, part: &str) -> Result<()> {
        let part = name(OsStr::new(part))?;
        if unsafe { libc::unlinkat(self.0.as_raw_fd(), part.as_ptr(), 0) } < 0 {
            return Err(io::Error::last_os_error().into());
        }
        self.sync()
    }
    pub fn sync(&self) -> Result<()> {
        self.0.sync_all().context("cannot sync private directory")
    }
    pub fn entries(&self) -> Result<Vec<String>> {
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                c".".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error().into());
        }
        let stream = unsafe { libc::fdopendir(fd) };
        if stream.is_null() {
            unsafe {
                libc::close(fd);
            }
            return Err(io::Error::last_os_error().into());
        }
        let mut names = Vec::new();
        loop {
            let item = unsafe { libc::readdir(stream) };
            if item.is_null() {
                break;
            }
            let bytes = unsafe { CStr::from_ptr((*item).d_name.as_ptr()) }.to_bytes();
            if bytes != b"." && bytes != b".." {
                names.push(String::from_utf8(bytes.to_vec())?);
            }
        }
        unsafe {
            libc::closedir(stream);
        }
        names.sort_by_key(|name| (name == "committed", name.clone()));
        Ok(names)
    }
    pub fn remove_tree(&self, part: &str) -> Result<()> {
        let child = self.child(part, false)?;
        for item in child.entries()? {
            // Opening with O_NOFOLLOW refuses hostile symlinks, rather than following them.
            match child.child(&item, false) {
                Ok(_) => child.remove_tree(&item)?,
                Err(_) => {
                    child.file(&item, false)?;
                    child.remove(&item)?;
                }
            }
        }
        let part = name(OsStr::new(part))?;
        if unsafe { libc::unlinkat(self.0.as_raw_fd(), part.as_ptr(), libc::AT_REMOVEDIR) } < 0 {
            return Err(io::Error::last_os_error().into());
        }
        self.sync()
    }
    pub fn atomic_json(&self, leaf: &str, value: &impl serde::Serialize) -> Result<()> {
        self.atomic_json_limited(leaf, value, u64::MAX)
    }
    pub fn atomic_json_limited(
        &self,
        leaf: &str,
        value: &impl serde::Serialize,
        max_bytes: u64,
    ) -> Result<()> {
        // The caller holds the parent lock. An orphan temp is never authoritative.
        if self.optional_file(".write.tmp")?.is_some() {
            self.remove(".write.tmp")?;
        }
        let mut file = self.create_new(".write.tmp")?;
        serde_json::to_writer(
            &mut LimitedWriter {
                file: &mut file,
                remaining: max_bytes,
            },
            value,
        )?;
        file.sync_all()?;
        self.rename(".write.tmp", self, leaf)
    }
}
pub fn not_found(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<io::Error>()
        .is_some_and(|e| e.kind() == io::ErrorKind::NotFound)
}
pub fn relative(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 4096
        || path.contains(['\0', '\n', '\r', '\\'])
        || path
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

struct LimitedWriter<'a> {
    file: &'a mut File,
    remaining: u64,
}
impl std::io::Write for LimitedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            return Err(io::Error::other("transaction metadata limit exceeded"));
        }
        let written = std::io::Write::write(self.file, bytes)?;
        self.remaining -= written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        std::io::Write::flush(self.file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn temp() -> tempfile::TempDir {
        tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap()
    }
    #[test]
    fn swapped_directory_name_cannot_redirect_anchored_writes() {
        let storage = temp();
        let outside = temp();
        let root_path = storage.path().canonicalize().unwrap();
        let outside_path = outside.path().canonicalize().unwrap();
        let root = Dir::open(&root_path, false).unwrap();
        let anchored = root.child("managed", true).unwrap();
        root.rename("managed", &root, "original").unwrap();
        std::os::unix::fs::symlink(&outside_path, root_path.join("managed")).unwrap();
        anchored
            .create_new("artifact")
            .unwrap()
            .write_all(b"private")
            .unwrap();
        anchored.atomic_json("config.json", &"private").unwrap();
        assert!(!outside_path.join("artifact").exists());
        assert!(!outside_path.join("config.json").exists());
        assert!(root.child("managed", false).is_err());
        assert_eq!(
            std::fs::read(root_path.join("original/artifact")).unwrap(),
            b"private"
        );
    }
    #[test]
    fn oversized_journal_is_rejected_before_becoming_authoritative() {
        let storage = temp();
        let root = Dir::open(&storage.path().canonicalize().unwrap(), false).unwrap();
        assert!(
            root.atomic_json_limited("ready.json", &vec!["large metadata"; 10], 32)
                .is_err()
        );
        assert!(root.optional_file("ready.json").unwrap().is_none());
    }
}
