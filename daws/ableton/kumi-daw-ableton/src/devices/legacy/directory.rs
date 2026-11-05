//! A pinned directory: all reads and writes remain in the directory opened, through concurrent path replacement.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(unix)]
pub(super) use unix::Directory;
#[cfg(windows)]
pub(super) use windows::Directory;

use std::{
    fs::File,
    io::{self, Read},
};

fn read_regular(file: File, most: u64) -> io::Result<Vec<u8>> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > most {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular device file within the size limit"));
    }
    let mut bytes = Vec::new();
    file.take(most + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > most {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "device grew beyond the size limit"));
    }
    Ok(bytes)
}
