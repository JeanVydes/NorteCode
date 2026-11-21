//! Disk-backed storage for generated exports and import previews.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use crate::HostError;

fn failure(error: impl std::fmt::Display) -> HostError {
    HostError::Artifact(format!("bundle spool error: {error}"))
}

fn create_file(path: &Path) -> Result<File, HostError> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(failure)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(failure)?;
    }
    Ok(file)
}

pub(super) fn prepare_dir(dir: &Path) -> Result<(), HostError> {
    fs::create_dir_all(dir).map_err(failure)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(failure)?;
    }
    Ok(())
}

pub(super) fn write_bytes(dir: &Path, bytes: &[u8]) -> Result<PathBuf, HostError> {
    prepare_dir(dir)?;
    let path = dir.join(format!("bundle_{}.bin", Uuid::new_v4().simple()));
    let mut file = create_file(&path)?;
    file.write_all(bytes).map_err(failure)?;
    file.flush().map_err(failure)?;
    Ok(path)
}

pub(super) fn write_json<T>(dir: &Path, value: &T) -> Result<PathBuf, HostError>
where
    T: Serialize,
{
    let bytes = serde_json::to_vec(value).map_err(failure)?;
    write_bytes(dir, &bytes)
}

pub(super) fn read_json<T>(path: &Path) -> Result<T, HostError>
where
    T: DeserializeOwned,
{
    let bytes = fs::read(path).map_err(failure)?;
    serde_json::from_slice(&bytes).map_err(failure)
}

pub(super) fn read_bytes(path: &Path) -> Result<Vec<u8>, HostError> {
    fs::read(path).map_err(failure)
}

pub(super) fn remove(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(test)]
#[path = "bundle_store_tests.rs"]
mod tests;
