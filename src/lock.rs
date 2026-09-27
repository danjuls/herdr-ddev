//! One ticker per machine.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::Path;

use anyhow::Result;

/// Take an exclusive lock on `path` without waiting; `None` means another process holds it.
/// The OS drops the lock when its holder exits, so a crash leaves no stale lock.
pub fn try_lock(path: &Path) -> Result<Option<File>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(err)) => Err(err.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_lock_fails_until_the_first_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/ticker.lock");
        let first = try_lock(&path).unwrap();
        assert!(first.is_some());
        assert!(try_lock(&path).unwrap().is_none());
        drop(first);
        assert!(try_lock(&path).unwrap().is_some());
    }
}
