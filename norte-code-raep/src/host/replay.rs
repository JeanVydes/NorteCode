//! The session replay log: the frames a client missed while detached.
//!
//! One append-only JSONL file per session, capped by bytes. A session that
//! outgrows its cap keeps its newest half, and an attach that asks for a
//! sequence older than what is still on disk is told the window was
//! truncated. There is no second database and no in-memory copy: the file is
//! the log, so the same process and a restarted one read the same frames.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use raep_types::Envelope;
use serde_json::Value;

use super::HostError;

/// What an attach gets back for a `resume_from_seq`.
pub(crate) struct ReplayWindow {
    /// Frames strictly after the requested sequence, oldest first.
    pub(crate) frames: Vec<Envelope<Value>>,
    /// Whether older frames existed and are no longer retained.
    pub(crate) truncated: bool,
}

/// The per-session replay files.
pub struct ReplayLog {
    root: PathBuf,
    max_bytes: u64,
    /// Serializes appends, trims and reads. The files are small and the
    /// critical section does not await.
    lock: Mutex<()>,
}

impl ReplayLog {
    /// Opens the log at `root`, keeping at most `max_bytes` per session.
    pub fn new(root: impl Into<PathBuf>, max_bytes: u64) -> Self {
        Self {
            root: root.into(),
            max_bytes: max_bytes.max(1),
            lock: Mutex::new(()),
        }
    }

    /// The file a session's frames live in.
    ///
    /// Session ids come from the protocol, not the filesystem: anything
    /// outside the safe set becomes `_`, so an id cannot escape the replay
    /// directory.
    fn path(&self, session: &str) -> PathBuf {
        let safe: String = session
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.root.join(format!("{safe}.jsonl"))
    }

    /// Appends one frame to its session's log, trimming the file if it grew
    /// past the cap.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::Replay`] when the log cannot be written.
    pub(crate) fn append(&self, frame: &Envelope<Value>) -> Result<(), HostError> {
        let session = frame.session_id.clone();
        if session.is_empty() {
            return Ok(());
        }
        let mut line = serde_json::to_vec(frame)
            .map_err(|error| HostError::Replay(format!("unserializable frame: {error}")))?;
        line.push(b'\n');
        let _guard = self.lock.lock().expect("replay lock is not poisoned");
        let path = self.path(&session);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| HostError::Replay(format!("replay directory: {error}")))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| HostError::Replay(format!("replay file: {error}")))?;
        file.write_all(&line)
            .map_err(|error| HostError::Replay(format!("replay append: {error}")))?;
        let size = file
            .metadata()
            .map_or(0, |meta| meta.len())
            .saturating_add(line.len() as u64);
        drop(file);
        if size > self.max_bytes {
            self.trim(&path)?;
        }
        Ok(())
    }

    /// Keeps the newest half of the log, at a line boundary.
    fn trim(&self, path: &Path) -> Result<(), HostError> {
        let content =
            fs::read(path).map_err(|error| HostError::Replay(format!("replay read: {error}")))?;
        let keep_from = content.len().saturating_sub((self.max_bytes / 2) as usize);
        let boundary = content[keep_from..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(content.len(), |offset| keep_from + offset + 1);
        let trimmed = &content[boundary.min(content.len())..];
        let temporary = path.with_extension("jsonl.tmp");
        fs::write(&temporary, trimmed)
            .map_err(|error| HostError::Replay(format!("replay trim: {error}")))?;
        fs::rename(&temporary, path)
            .map_err(|error| HostError::Replay(format!("replay rotate: {error}")))?;
        Ok(())
    }

    /// Reads the frames a session keeps after `after`.
    ///
    /// A log that cannot be read is an empty window, not an error: the
    /// client still gets a well-formed attach and reloads history.
    pub(crate) fn after(&self, session: &str, after: u64) -> ReplayWindow {
        let _guard = self.lock.lock().expect("replay lock is not poisoned");
        let path = self.path(session);
        let Ok(file) = File::open(&path) else {
            return ReplayWindow {
                frames: Vec::new(),
                truncated: after > 0,
            };
        };
        let mut frames = VecDeque::new();
        let mut first_seq = None;
        for line in BufReader::new(file).lines() {
            let Ok(line) = line else { continue };
            let Ok(frame) = serde_json::from_str::<Envelope<Value>>(&line) else {
                continue;
            };
            if let Some(seq) = frame.seq {
                first_seq.get_or_insert(seq);
            }
            frames.push_back(frame);
        }
        let truncated = first_seq.is_some_and(|first| after.saturating_add(1) < first);
        let frames = frames
            .into_iter()
            .filter(|frame| frame.seq.is_some_and(|seq| seq > after))
            .collect();
        ReplayWindow { frames, truncated }
    }

    /// The last sequence the log holds for a session, if it has one.
    pub(crate) fn last_seq(&self, session: &str) -> Option<u64> {
        let _guard = self.lock.lock().expect("replay lock is not poisoned");
        let file = File::open(self.path(session)).ok()?;
        let mut last = None;
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            if let Ok(frame) = serde_json::from_str::<Envelope<Value>>(&line)
                && let Some(seq) = frame.seq
            {
                last = Some(seq);
            }
        }
        last
    }

    /// The frames and bytes the log currently holds, across sessions.
    pub(crate) fn total(&self) -> (u64, u64) {
        let _guard = self.lock.lock().expect("replay lock is not poisoned");
        let Ok(entries) = fs::read_dir(&self.root) else {
            return (0, 0);
        };
        let mut frames = 0;
        let mut bytes = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            bytes += entry.metadata().map(|meta| meta.len()).unwrap_or_default();
            if let Ok(file) = File::open(&path) {
                frames += BufReader::new(file).lines().map_while(Result::ok).count() as u64;
            }
        }
        (frames, bytes)
    }

    /// Deletes a session's log, when the session is gone.
    pub(crate) fn remove(&self, session: &str) {
        let _guard = self.lock.lock().expect("replay lock is not poisoned");
        let _ = fs::remove_file(self.path(session));
    }
}
