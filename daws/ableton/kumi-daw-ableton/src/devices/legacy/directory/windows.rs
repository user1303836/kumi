use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
};

// Win32's file flags and access/share masks (the same API used by the app's guarded artifact opens).
const FILE_READ_ATTRIBUTES: u32 = 0x80;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_SHARE_READ: u32 = 1;

pub(crate) struct Directory {
    path: PathBuf,
    // No containing directory can be renamed/replaced (DELETE), or turned into a reparse point (WRITE).
    held: Vec<File>,
}

impl Directory {
    pub(crate) fn root(path: &Path) -> io::Result<Self> {
        let canonical = std::fs::canonicalize(path)?;
        let mut path = PathBuf::new();
        let mut held = Vec::new();
        for component in canonical.components() {
            path.push(component.as_os_str());
            match component {
                Component::Prefix(_) => continue,
                Component::RootDir | Component::Normal(_) => held.push(open_directory(&path)?),
                _ => return Err(io::ErrorKind::InvalidInput.into()),
            }
        }
        if held.is_empty() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Ok(Self { path, held })
    }

    pub(crate) fn child(&self, name: &str, create: bool) -> io::Result<Self> {
        filename(name)?;
        let path = self.path.join(name);
        if create {
            match std::fs::create_dir(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        let child = open_directory(&path)?;
        let mut held = self.held.iter().map(File::try_clone).collect::<io::Result<Vec<_>>>()?;
        held.push(child);
        Ok(Self { path, held })
    }

    pub(crate) fn read(&self, name: &str, most: u64) -> io::Result<Vec<u8>> {
        filename(name)?;
        let file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(self.path.join(name))?;
        if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        super::read_regular(file, most)
    }

    pub(crate) fn temporary(&self, bytes: &[u8]) -> io::Result<Temporary<'_>> {
        let mut file = tempfile::NamedTempFile::new_in(&self.path)?;
        file.write_all(bytes)?;
        file.as_file().sync_all()?;
        Ok(Temporary { directory: self, file: Some(file) })
    }

    pub(crate) fn is_at(&self, path: &Path) -> bool {
        // Every canonical ancestor is pinned without delete sharing; this only checks an intentionally linked root
        // still resolves to the same path. All actual copy paths use the pinned canonical path.
        std::fs::canonicalize(path).is_ok_and(|now| now == self.path)
    }

    pub(crate) fn contains(&self, name: &str, directory: &Self) -> bool {
        self.path.join(name) == directory.path
    }
}

fn open_directory(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(file)
}

pub(crate) struct Temporary<'a> {
    directory: &'a Directory,
    file: Option<tempfile::NamedTempFile>,
}

impl Temporary<'_> {
    pub(crate) fn persist(&mut self, name: &str) -> io::Result<()> {
        filename(name)?;
        let temporary = self.file.take().ok_or(io::ErrorKind::InvalidInput)?;
        match temporary.persist_noclobber(self.directory.path.join(name)) {
            Ok(_) => Ok(()),
            Err(error) => {
                self.file = Some(error.file);
                Err(error.error)
            }
        }
    }
}

fn filename(name: &str) -> io::Result<()> {
    if name.is_empty() || matches!(name, "." | "..") || name.contains(['/', '\\', ':']) || kumi_common::path::reserved_device(name) {
        Err(io::ErrorKind::InvalidInput.into())
    } else {
        Ok(())
    }
}
