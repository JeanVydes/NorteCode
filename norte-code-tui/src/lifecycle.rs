//! Coordination of local terminal clients and service lifecycle.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use rustix::fs::{FlockOperation, flock};

fn state_dir() -> io::Result<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no state directory"))?;
    Ok(base.join("norte-code"))
}

fn open_lock(dir: &Path, name: &str) -> io::Result<File> {
    fs::create_dir_all(dir)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(name))
}

fn exclusive(file: &File) -> io::Result<()> {
    flock(file, FlockOperation::LockExclusive).map_err(Into::into)
}

fn any_clients(file: &File) -> io::Result<bool> {
    match flock(file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => {
            flock(file, FlockOperation::Unlock)?;
            Ok(false)
        }
        Err(rustix::io::Errno::AGAIN) => Ok(true),
        Err(error) => Err(error.into()),
    }
}

/// Serializes startup and shutdown decisions across local TUI processes.
///
/// # Errors
/// Returns an error if coordination, the terminal, or a lifecycle action fails.
pub fn run_with_lifecycle<P, S>(prepare: P, stop: S) -> io::Result<()>
where
    P: FnOnce(bool) -> io::Result<()>,
    S: FnOnce() -> io::Result<()>,
{
    let path = norte_code_protocol::config::user_cli_path();
    let config = norte_code_protocol::config::CliConfig::load(path.as_deref(), None)
        .map_err(io::Error::other)?
        .config;
    if config.service_type == norte_code_protocol::config::ServiceType::Remote {
        prepare(false)?;
        return crate::app::run().map_err(io::Error::other);
    }
    run_at(
        &state_dir()?,
        prepare,
        || crate::app::run().map_err(io::Error::other),
        stop,
    )
}

fn run_at<P, R, S>(dir: &Path, prepare: P, run: R, stop: S) -> io::Result<()>
where
    P: FnOnce(bool) -> io::Result<()>,
    R: FnOnce() -> io::Result<()>,
    S: FnOnce() -> io::Result<()>,
{
    let gate = open_lock(dir, "tui-lifecycle.lock")?;
    exclusive(&gate)?;
    let clients = open_lock(dir, "tui-clients.lock")?;
    let active = any_clients(&clients)?;
    prepare(active)?;
    flock(&clients, FlockOperation::LockShared)?;
    flock(&gate, FlockOperation::Unlock)?;
    let result = run();
    exclusive(&gate)?;
    flock(&clients, FlockOperation::Unlock)?;
    let remaining = any_clients(&clients)?;
    let stopped = if remaining { Ok(()) } else { stop() };
    flock(&gate, FlockOperation::Unlock)?;
    result.and(stopped)
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
