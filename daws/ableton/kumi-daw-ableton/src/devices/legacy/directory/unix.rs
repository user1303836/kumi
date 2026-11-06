use std::{
    ffi::{CStr, CString},
    fs::{File, OpenOptions},
    io::{self, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Component, Path},
};

pub(crate) struct Directory {
    file: File,
}

impl Directory {
    pub(crate) fn root(path: &Path) -> io::Result<Self> {
        // The configured root may be a link. Resolve it once, then walk from / with no-follow directory opens.
        let path = std::fs::canonicalize(path)?;
        let file = OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW).open("/")?;
        let mut directory = Self { file };
        for component in path.components() {
            match component {
                Component::Normal(name) => directory = directory.child(name.to_str().ok_or(io::ErrorKind::InvalidInput)?, false)?,
                Component::RootDir => {}
                _ => return Err(io::ErrorKind::InvalidInput.into()),
            }
        }
        Ok(directory)
    }

    pub(crate) fn child(&self, name: &str, create: bool) -> io::Result<Self> {
        let name = filename(name)?;
        if create {
            // SAFETY: the directory descriptor and NUL-terminated single-component name remain live.
            let made = unsafe { libc::mkdirat(self.file.as_raw_fd(), name.as_ptr(), 0o755) };
            if made < 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::AlreadyExists {
                    return Err(error);
                }
            }
        }
        let file = self.open(&name, libc::O_RDONLY | libc::O_DIRECTORY)?;
        Ok(Self { file })
    }

    fn open(&self, name: &CStr, flags: i32) -> io::Result<File> {
        // SAFETY: the descriptor and NUL-terminated name remain live; mode is supplied for O_CREAT.
        let fd = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                0o600 as libc::c_uint,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat returned a new owned descriptor.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    pub(crate) fn read(&self, name: &str, most: u64) -> io::Result<Vec<u8>> {
        super::read_regular(self.open(&filename(name)?, libc::O_RDONLY)?, most)
    }

    pub(crate) fn temporary(&self, bytes: &[u8]) -> io::Result<Temporary<'_>> {
        let name = filename(&format!(".kumi-legacy-{}.tmp", uuid::Uuid::new_v4()))?;
        let file = self.open(&name, libc::O_RDWR | libc::O_CREAT | libc::O_EXCL)?;
        let mut temporary = Temporary { directory: self, name, file };
        temporary.file.write_all(bytes)?;
        temporary.file.sync_all()?;
        Ok(temporary)
    }

    pub(crate) fn is_at(&self, path: &Path) -> bool {
        // The root may intentionally be linked; only its identity matters here. No write uses this path.
        let Ok(there) = std::fs::metadata(path) else { return false };
        self.file.metadata().is_ok_and(|held| there.is_dir() && held.dev() == there.dev() && held.ino() == there.ino())
    }

    pub(crate) fn contains(&self, name: &str, directory: &Self) -> bool {
        let Ok(now) = self.child(name, false).and_then(|now| now.file.metadata()) else { return false };
        directory.file.metadata().is_ok_and(|held| held.dev() == now.dev() && held.ino() == now.ino())
    }
}

pub(crate) struct Temporary<'a> {
    directory: &'a Directory,
    name: CString,
    file: File,
}

impl Temporary<'_> {
    pub(crate) fn persist(&mut self, name: &str) -> io::Result<()> {
        let name = filename(name)?;
        // SAFETY: both names and the containing descriptor remain live. linkat never replaces an existing name;
        // both sides are anchored to the same held directory, even if that directory has moved since it was opened.
        let result =
            unsafe { libc::linkat(self.directory.file.as_raw_fd(), self.name.as_ptr(), self.directory.file.as_raw_fd(), name.as_ptr(), 0) };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

impl Drop for Temporary<'_> {
    fn drop(&mut self) {
        // SAFETY: the containing descriptor and temporary name are still live; cleanup never follows a replaced path.
        unsafe { libc::unlinkat(self.directory.file.as_raw_fd(), self.name.as_ptr(), 0) };
    }
}

fn filename(name: &str) -> io::Result<CString> {
    if name.is_empty() || matches!(name, "." | "..") || name.contains('/') {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    CString::new(name).map_err(|_| io::ErrorKind::InvalidInput.into())
}
