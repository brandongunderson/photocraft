//! Cooperative store locking across desktop processes. Never replace the lock file:
//! its OS identity must remain the same for every reader, writer and worker lease.
use std::fs::{File, OpenOptions, TryLockError};
use std::io::ErrorKind;
use std::path::Path;

use super::{Result, checked_path, error, is_link};

const LOCK: &str = ".photocraft-store.lock";

#[must_use]
pub(crate) struct StoreGuard {
    _file: File,
}

impl StoreGuard {
    pub(crate) fn read(root: &Path) -> Result<Self> {
        Self::acquire(root, false)
    }

    pub(crate) fn write(root: &Path) -> Result<Self> {
        Self::acquire(root, true)
    }

    fn acquire(root: &Path, exclusive: bool) -> Result<Self> {
        Self::named(root, LOCK, exclusive)
    }

    pub(super) fn named(root: &Path, name: &str, exclusive: bool) -> Result<Self> {
        let path = checked_path(root, name)?;
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.is_file() || is_link(&metadata) => return Err(error("plugin store lock must be a regular file")),
            Ok(_) => {}
            Err(cause) if cause.kind() == ErrorKind::NotFound => {}
            Err(cause) => return Err(error(cause)),
        }
        let file = match OpenOptions::new().read(true).write(exclusive).open(&path) {
            Ok(file) => file,
            Err(cause) if cause.kind() == ErrorKind::NotFound => match OpenOptions::new().read(true).write(true).create_new(true).open(&path) {
                Ok(file) => file,
                Err(cause) if cause.kind() == ErrorKind::AlreadyExists => OpenOptions::new().read(true).write(exclusive).open(&path).map_err(error)?,
                Err(cause) => return Err(error(format!("could not initialize the plugin store lock; the store needs one writable initialization: {cause}"))),
            },
            Err(cause) => return Err(error(cause)),
        };
        let metadata = file.metadata().map_err(error)?;
        if !metadata.is_file() || is_link(&metadata) {
            return Err(error("plugin store lock must be a regular file"));
        }
        let locked = if exclusive { file.try_lock() } else { file.try_lock_shared() };
        match locked {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(error(
                "Plugin files are still in use by an operation or processing engine, possibly in another PhotoCraft instance. Wait for the engine to close, or disable the plugin and review the file again before retrying.",
            )),
            Err(TryLockError::Error(cause)) => Err(error(format!("could not lock the plugin store: {cause}"))),
        }
    }
}
