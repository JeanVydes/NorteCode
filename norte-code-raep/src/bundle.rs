//! Portable RAEP bundle encoding and validation.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Write};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

/// A file hash and length recorded in the bundle manifest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BundleFile {
    /// Archive-relative file path.
    pub path: String,
    /// Lowercase SHA-256 digest.
    pub sha256: String,
    /// Uncompressed byte length.
    pub bytes: u64,
}

/// A session entry in a portable bundle.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BundleSession {
    /// Original session identifier.
    pub session_id: String,
    /// Original parent, or `None` for the root.
    pub parent_session_id: Option<String>,
    /// Agent reference as defined by RAEP.
    pub agent: serde_json::Value,
    /// Original state name.
    pub state: Option<String>,
    /// Path to the session's JSONL history.
    pub history_file: String,
    /// Last event sequence included in the bundle.
    pub last_seq: Option<u64>,
    /// Usage snapshot, when available.
    pub usage_total: Option<serde_json::Value>,
}

/// The versioned manifest at the root of a portable RAEP ZIP.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BundleManifest {
    /// Bundle format version, currently `"1"`.
    pub bundle_version: String,
    /// RAEP document version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raep_document_version: Option<String>,
    /// RAEP wire version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wire_version: Option<String>,
    /// Unix timestamp in milliseconds.
    pub exported_at: u64,
    /// Exporting server metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exporter: Option<serde_json::Value>,
    /// Original root session identifier.
    pub root_session_id: String,
    /// Sessions included in the bundle.
    pub sessions: Vec<BundleSession>,
    /// All archive files except the manifest and optional signature.
    pub files: Vec<BundleFile>,
    /// Content deliberately omitted from the bundle.
    pub excluded: Vec<String>,
}

/// Errors raised while creating or validating a portable bundle.
#[derive(Debug, Error)]
pub enum BundleError {
    /// The archive is malformed or violates a configured bound.
    #[error("invalid bundle: {0}")]
    Invalid(String),
    /// ZIP compression or extraction failed.
    #[error("bundle ZIP failed: {0}")]
    Zip(#[from] zip::result::ZipError),
    /// JSON serialization failed.
    #[error("bundle JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    /// Writing an archive failed.
    #[error("bundle I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Builds a ZIP bundle and hashes every included data file.
///
/// # Errors
///
/// Returns an error when the manifest is invalid or ZIP construction fails.
pub fn create_bundle(
    mut manifest: BundleManifest,
    files: BTreeMap<String, Vec<u8>>,
) -> Result<Vec<u8>, BundleError> {
    if manifest.sessions.is_empty() || manifest.bundle_version != "1" {
        return Err(BundleError::Invalid(
            "version 1 requires at least one session".to_owned(),
        ));
    }
    manifest.files = files
        .iter()
        .map(|(path, bytes)| {
            validate_path(path)?;
            Ok(BundleFile {
                path: path.clone(),
                sha256: sha256(bytes),
                bytes: bytes.len() as u64,
            })
        })
        .collect::<Result<_, BundleError>>()?;
    validate_manifest(&manifest)?;
    let manifest_bytes = serde_json::to_vec(&manifest)?;
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default();
    writer.start_file("manifest.json", options)?;
    writer.write_all(&manifest_bytes)?;
    for (path, bytes) in files {
        writer.start_file(path, options)?;
        writer.write_all(&bytes)?;
    }
    Ok(writer.finish()?.into_inner())
}

/// Reads a bundle, enforcing path, decompressed-size, and SHA-256 checks.
///
/// # Errors
///
/// Returns an error for an unsupported version, unsafe path, oversized
/// archive, missing file, or hash/length mismatch.
pub fn read_bundle(
    bytes: &[u8],
    max_uncompressed_bytes: usize,
) -> Result<(BundleManifest, BTreeMap<String, Vec<u8>>), BundleError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))?;
    if archive.len() > 4096 {
        return Err(BundleError::Invalid("too many archive entries".to_owned()));
    }
    let mut files = BTreeMap::new();
    let mut total = 0_usize;
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        let path = entry.name().to_owned();
        if path != "manifest.json" {
            validate_path(&path)?;
        }
        if entry.is_dir() || files.contains_key(&path) {
            return Err(BundleError::Invalid(
                "duplicate or directory entry".to_owned(),
            ));
        }
        let size = usize::try_from(entry.size())
            .map_err(|_| BundleError::Invalid("entry is too large".to_owned()))?;
        total = total
            .checked_add(size)
            .filter(|total| *total <= max_uncompressed_bytes)
            .ok_or_else(|| BundleError::Invalid("bundle exceeds size limit".to_owned()))?;
        let mut data = Vec::with_capacity(size);
        entry
            .take((max_uncompressed_bytes.saturating_sub(total) as u64).saturating_add(size as u64))
            .read_to_end(&mut data)?;
        if data.len() != size {
            return Err(BundleError::Invalid("truncated archive entry".to_owned()));
        }
        files.insert(path, data);
    }
    let manifest_bytes = files
        .remove("manifest.json")
        .ok_or_else(|| BundleError::Invalid("manifest.json is missing".to_owned()))?;
    let manifest: BundleManifest = serde_json::from_slice(&manifest_bytes)?;
    validate_manifest(&manifest)?;
    if manifest.files.len() != files.len() {
        return Err(BundleError::Invalid(
            "manifest file list does not match archive".to_owned(),
        ));
    }
    for entry in &manifest.files {
        validate_path(&entry.path)?;
        let data = files
            .get(&entry.path)
            .ok_or_else(|| BundleError::Invalid(format!("missing {}", entry.path)))?;
        if data.len() as u64 != entry.bytes || sha256(data) != entry.sha256 {
            return Err(BundleError::Invalid(format!(
                "size or SHA-256 mismatch for {}",
                entry.path
            )));
        }
    }
    let listed: BTreeSet<_> = manifest
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    if files.keys().any(|path| !listed.contains(path.as_str())) {
        return Err(BundleError::Invalid(
            "archive contains a file not listed in manifest".to_owned(),
        ));
    }
    Ok((manifest, files))
}

fn validate_manifest(manifest: &BundleManifest) -> Result<(), BundleError> {
    if manifest.bundle_version != "1" || manifest.sessions.is_empty() {
        return Err(BundleError::Invalid(
            "unsupported version or empty session list".to_owned(),
        ));
    }
    let session_ids: BTreeSet<_> = manifest
        .sessions
        .iter()
        .map(|session| session.session_id.as_str())
        .collect();
    if !session_ids.contains(manifest.root_session_id.as_str())
        || session_ids.len() != manifest.sessions.len()
    {
        return Err(BundleError::Invalid(
            "root session is missing or session ids are duplicated".to_owned(),
        ));
    }
    let sessions: BTreeMap<_, _> = manifest
        .sessions
        .iter()
        .map(|session| (session.session_id.as_str(), session))
        .collect();
    if sessions[manifest.root_session_id.as_str()]
        .parent_session_id
        .is_some()
    {
        return Err(BundleError::Invalid(
            "root session cannot have a parent".to_owned(),
        ));
    }
    for session in &manifest.sessions {
        let mut current = session;
        let mut visited = BTreeSet::new();
        while current.session_id != manifest.root_session_id {
            if !visited.insert(current.session_id.as_str()) {
                return Err(BundleError::Invalid(
                    "session parent references contain a cycle".to_owned(),
                ));
            }
            let parent = current
                .parent_session_id
                .as_deref()
                .and_then(|parent| sessions.get(parent).copied())
                .ok_or_else(|| {
                    BundleError::Invalid("session tree is disconnected from the root".to_owned())
                })?;
            current = parent;
        }
    }
    let file_paths: BTreeSet<_> = manifest
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    if file_paths.len() != manifest.files.len()
        || manifest.sessions.iter().any(|session| {
            !file_paths.contains(session.history_file.as_str())
                || !manifest
                    .sessions
                    .iter()
                    .filter(|candidate| candidate.session_id != session.session_id)
                    .all(|candidate| candidate.history_file != session.history_file)
        })
    {
        return Err(BundleError::Invalid(
            "session history file is missing or duplicated".to_owned(),
        ));
    }
    Ok(())
}

fn validate_path(path: &str) -> Result<(), BundleError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || path == "manifest.json"
        || path == "signature.jws"
    {
        return Err(BundleError::Invalid(format!(
            "unsafe archive path `{path}`"
        )));
    }
    Ok(())
}

/// Returns the lowercase SHA-256 digest of `bytes`.
#[must_use]
pub fn sha256(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut digest = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(digest, "{byte:02x}").expect("writing to a String cannot fail");
    }
    digest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> BundleManifest {
        BundleManifest {
            bundle_version: "1".to_owned(),
            raep_document_version: Some("0.1.0".to_owned()),
            wire_version: Some("1.0".to_owned()),
            exported_at: 1,
            exporter: None,
            root_session_id: "source".to_owned(),
            sessions: vec![BundleSession {
                session_id: "source".to_owned(),
                parent_session_id: None,
                agent: serde_json::json!({"id": "test"}),
                state: None,
                history_file: "sessions/source/history.jsonl".to_owned(),
                last_seq: None,
                usage_total: None,
            }],
            files: Vec::new(),
            excluded: Vec::new(),
        }
    }

    #[test]
    fn bundle_round_trip_checks_manifest_hashes() {
        let files = BTreeMap::from([(
            "sessions/source/history.jsonl".to_owned(),
            b"{\"role\":\"user\",\"content\":\"hi\"}\n".to_vec(),
        )]);
        let bytes = create_bundle(manifest(), files.clone()).expect("bundle");
        let (manifest, restored) = read_bundle(&bytes, 1024).expect("read bundle");
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(restored, files);
    }

    #[test]
    fn bundle_rejects_parent_paths() {
        let files = BTreeMap::from([("../outside".to_owned(), b"no".to_vec())]);
        assert!(matches!(
            create_bundle(manifest(), files),
            Err(BundleError::Invalid(_))
        ));
    }

    #[test]
    fn bundle_rejects_disconnected_sessions() {
        let mut manifest = manifest();
        manifest.sessions.push(BundleSession {
            session_id: "child".to_owned(),
            parent_session_id: Some("missing".to_owned()),
            agent: serde_json::json!({"id": "test"}),
            state: None,
            history_file: "sessions/child/history.jsonl".to_owned(),
            last_seq: None,
            usage_total: None,
        });
        let files = BTreeMap::from([
            (
                "sessions/source/history.jsonl".to_owned(),
                b"{\"role\":\"user\",\"content\":\"hi\"}\n".to_vec(),
            ),
            ("sessions/child/history.jsonl".to_owned(), Vec::new()),
        ]);
        assert!(matches!(
            create_bundle(manifest, files),
            Err(BundleError::Invalid(_))
        ));
    }

    #[test]
    fn bundle_rejects_hash_mismatches() {
        let mut manifest = manifest();
        manifest.files.push(BundleFile {
            path: "sessions/source/history.jsonl".to_owned(),
            sha256: "0".repeat(64),
            bytes: 2,
        });
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("manifest.json", SimpleFileOptions::default())
            .expect("manifest entry");
        writer
            .write_all(&serde_json::to_vec(&manifest).expect("manifest JSON"))
            .expect("manifest bytes");
        writer
            .start_file(
                "sessions/source/history.jsonl",
                SimpleFileOptions::default(),
            )
            .expect("history entry");
        writer.write_all(b"{}").expect("history bytes");
        let bytes = writer.finish().expect("finish bundle").into_inner();
        assert!(matches!(
            read_bundle(&bytes, 1024),
            Err(BundleError::Invalid(_))
        ));
    }
}
