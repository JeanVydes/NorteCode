//! The catalogue cache and refresh policy.
//!
//! The cache is one JSON file plus a small metadata sidecar (`ETag`,
//! `Last-Modified`, fetch time). A conditional request refreshes it; a 304
//! only touches the metadata. Writes are atomic, and a download that fails to
//! parse never replaces a good cache.

use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::schema::{Catalog, CatalogError, MAX_CATALOG_BYTES};

/// The largest download this build accepts.
pub const MAX_DOWNLOAD_BYTES: usize = 32 * 1024 * 1024;
/// Largest metadata sidecar accepted.
const MAX_META_BYTES: usize = 64 * 1024;

/// Reads a regular file with a hard size cap.
fn read_limited(path: &Path, limit: usize) -> std::io::Result<Vec<u8>> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let file = fs::File::open(path)?;
    let capacity = usize::try_from(metadata.len().min(limit as u64)).unwrap_or(limit);
    let mut bytes = Vec::with_capacity(capacity);
    file.take(limit.saturating_add(1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("file is larger than {limit} bytes"),
        ));
    }
    Ok(bytes)
}

/// How long a cached catalogue stays fresh by default.
pub const DEFAULT_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Where a loaded catalogue came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A fresh cache file.
    Cache,
    /// A conditional request that returned 304; the cache is unchanged.
    NotModified,
    /// A fresh download.
    Network,
    /// A stale cache used after the network failed.
    StaleCache,
    /// The snapshot compiled into the binary.
    Bundled,
}

/// The result of a load: the catalogue, where it came from, and what was
/// ignored or fell back.
#[derive(Clone, Debug)]
pub struct Loaded {
    /// The catalogue.
    pub catalog: Catalog,
    /// Where it came from.
    pub source: Source,
    /// Human-readable notes to show the user.
    pub warnings: Vec<String>,
}

/// What the caller wants from the cache and the network.
#[derive(Clone, Copy, Debug)]
pub struct LoadOptions<'a> {
    /// The catalogue URL.
    pub url: &'a str,
    /// How long a cache stays fresh.
    pub ttl: Duration,
    /// Never touch the network.
    pub offline: bool,
    /// The directory holding `catalog.json` and `catalog.meta.json`.
    pub cache_dir: &'a Path,
}

/// The cache's metadata sidecar.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Meta {
    /// The `ETag` the server sent.
    pub etag: Option<String>,
    /// The `Last-Modified` the server sent.
    pub last_modified: Option<String>,
    /// When the body was last fetched.
    pub fetched_at: Option<DateTime<Utc>>,
}

impl Meta {
    /// Whether the body is younger than `ttl`.
    #[must_use]
    pub fn is_fresh(&self, ttl: Duration) -> bool {
        let Some(fetched) = self.fetched_at else {
            return false;
        };
        let age = Utc::now().signed_duration_since(fetched);
        u64::try_from(age.num_seconds()).is_ok_and(|seconds| seconds < ttl.as_secs())
    }
}

/// The on-disk cache.
#[derive(Clone, Debug)]
pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    /// A cache rooted at `dir`.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The catalogue file path.
    #[must_use]
    pub fn catalog_path(&self) -> PathBuf {
        self.dir.join("catalog.json")
    }

    /// The metadata file path.
    #[must_use]
    pub fn meta_path(&self) -> PathBuf {
        self.dir.join("catalog.meta.json")
    }

    /// Reads the cache, if it exists and parses.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError`] when the file exists but cannot be read or is
    /// not a valid catalogue. A missing file is `Ok(None)`.
    pub fn read(&self) -> Result<Option<(Catalog, Meta)>, CatalogError> {
        let path = self.catalog_path();
        let bytes = match read_limited(&path, MAX_CATALOG_BYTES) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(CatalogError::Read { path, source }),
        };
        let catalog = Catalog::parse(&bytes)?;
        let meta = read_limited(&self.meta_path(), MAX_META_BYTES)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Ok(Some((catalog, meta)))
    }

    /// Writes the catalogue and its metadata atomically.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError`] when the directory or either file cannot be
    /// written.
    pub fn write(&self, bytes: &[u8], meta: &Meta) -> Result<(), CatalogError> {
        if bytes.len() > MAX_CATALOG_BYTES {
            return Err(CatalogError::TooLarge {
                size: bytes.len(),
                max: MAX_CATALOG_BYTES,
            });
        }
        fs::create_dir_all(&self.dir).map_err(|source| CatalogError::Write {
            path: self.dir.clone(),
            source,
        })?;
        atomic_write(&self.catalog_path(), bytes)?;
        self.write_meta(meta)
    }

    /// Writes only the metadata, after a 304.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError`] when the file cannot be written.
    pub fn write_meta(&self, meta: &Meta) -> Result<(), CatalogError> {
        let bytes = serde_json::to_vec(meta)?;
        fs::create_dir_all(&self.dir).map_err(|source| CatalogError::Write {
            path: self.dir.clone(),
            source,
        })?;
        atomic_write(&self.meta_path(), &bytes)
    }
}

/// The user's cache directory: `$XDG_CACHE_HOME/norte-code`, or the platform
/// equivalent.
#[must_use]
pub fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))?;
    Some(base.join("norte-code"))
}

/// Loads the catalogue under the cache policy.
///
/// The order is: a fresh cache, a conditional request, the stale cache, the
/// bundled snapshot. A network failure never loses the last good catalogue.
///
/// # Errors
///
/// Returns [`CatalogError`] only if the bundled snapshot itself cannot be
/// parsed, which a test rules out.
pub async fn load_catalog(options: &LoadOptions<'_>) -> Result<Loaded, CatalogError> {
    let cache = Cache::new(options.cache_dir);
    let mut warnings = Vec::new();
    let cached = match cache.read() {
        Ok(Some((mut catalog, meta))) => {
            warnings.extend(catalog.validate());
            Some((catalog, meta))
        }
        Ok(None) => None,
        Err(error) => {
            warnings.push(format!("the catalogue cache could not be read: {error}"));
            None
        }
    };

    if let Some((catalog, meta)) = &cached {
        if options.offline || meta.is_fresh(options.ttl) {
            return Ok(Loaded {
                catalog: catalog.clone(),
                source: Source::Cache,
                warnings,
            });
        }
    }

    if !options.offline {
        match fetch(options, cached.as_ref().map(|(_, meta)| meta)).await {
            Ok(Fetched::NotModified) => {
                if let Some((catalog, meta)) = cached {
                    let touched = Meta {
                        fetched_at: Some(Utc::now()),
                        ..meta
                    };
                    if let Err(error) = cache.write_meta(&touched) {
                        warnings.push(format!(
                            "the catalogue metadata could not be written: {error}"
                        ));
                    }
                    return Ok(Loaded {
                        catalog,
                        source: Source::NotModified,
                        warnings,
                    });
                }
            }
            Ok(Fetched::Body { bytes, meta }) => match Catalog::parse(&bytes) {
                Ok(mut catalog) => {
                    warnings.extend(catalog.validate());
                    // Cache the normalized catalogue, so a value that was
                    // dropped here does not warn again on every read.
                    let cached = serde_json::to_vec(&catalog).unwrap_or(bytes);
                    if let Err(error) = cache.write(&cached, &meta) {
                        warnings.push(format!("the catalogue cache could not be written: {error}"));
                    }
                    return Ok(Loaded {
                        catalog,
                        source: Source::Network,
                        warnings,
                    });
                }
                Err(error) => warnings.push(format!(
                    "the downloaded catalogue was rejected, keeping the cache: {error}"
                )),
            },
            Err(error) => {
                warnings.push(format!("the catalogue could not be refreshed: {error}"));
            }
        }
    }

    if let Some((catalog, _)) = cached {
        warnings.push("using the stale catalogue cache".to_owned());
        return Ok(Loaded {
            catalog,
            source: Source::StaleCache,
            warnings,
        });
    }

    warnings.push("using the bundled catalogue snapshot".to_owned());
    Ok(Loaded {
        catalog: Catalog::bundled(),
        source: Source::Bundled,
        warnings,
    })
}

enum Fetched {
    NotModified,
    Body { bytes: Vec<u8>, meta: Meta },
}

async fn fetch(options: &LoadOptions<'_>, cached: Option<&Meta>) -> Result<Fetched, CatalogError> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("norte-code/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| CatalogError::Http(error.to_string()))?;
    let mut request = client.get(options.url);
    if let Some(meta) = cached {
        if let Some(etag) = &meta.etag {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }
        if let Some(last_modified) = &meta.last_modified {
            request = request.header(reqwest::header::IF_MODIFIED_SINCE, last_modified);
        }
    }
    let response = request
        .send()
        .await
        .map_err(|error| CatalogError::Http(error.to_string()))?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Fetched::NotModified);
    }
    let response = response
        .error_for_status()
        .map_err(|error| CatalogError::Http(error.to_string()))?;
    let etag = header(&response, reqwest::header::ETAG);
    let last_modified = header(&response, reqwest::header::LAST_MODIFIED);
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| CatalogError::Http(error.to_string()))?
    {
        let size = bytes.len().saturating_add(chunk.len());
        if size > MAX_DOWNLOAD_BYTES {
            return Err(CatalogError::TooLarge {
                size,
                max: MAX_DOWNLOAD_BYTES,
            });
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Fetched::Body {
        bytes,
        meta: Meta {
            etag,
            last_modified,
            fetched_at: Some(Utc::now()),
        },
    })
}

fn header(response: &reqwest::Response, name: reqwest::header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), CatalogError> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    for _ in 0..16 {
        let tmp = path.with_extension(format!(
            "tmp.{}.{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(CatalogError::Write {
                    path: tmp.clone(),
                    source,
                });
            }
        };
        let write = (|| -> std::io::Result<()> {
            let mut file = file;
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&tmp, path)
        })();
        if let Err(source) = write {
            let _ = fs::remove_file(&tmp);
            return Err(CatalogError::Write {
                path: path.to_path_buf(),
                source,
            });
        }
        return Ok(());
    }
    Err(CatalogError::Write {
        path: path.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not create a unique temporary file",
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("norte-code-catalog-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn write_cache(dir: &Path, fetched_at: DateTime<Utc>) -> Catalog {
        let catalog = Catalog::bundled();
        let bytes = serde_json::to_vec(&catalog).expect("serialize");
        let meta = Meta {
            fetched_at: Some(fetched_at),
            ..Meta::default()
        };
        Cache::new(dir).write(&bytes, &meta).expect("write");
        catalog
    }

    #[test]
    fn a_cache_file_over_the_limit_is_refused() {
        let dir = temp_dir();
        let path = dir.join("big.json");
        fs::write(&path, b"0123456789").expect("write");
        let error = read_limited(&path, 4).expect_err("file must be bounded");
        assert!(error.to_string().contains("larger than 4 bytes"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_cache_round_trips() {
        let dir = temp_dir();
        let catalog = write_cache(&dir, Utc::now());
        let (read, meta) = Cache::new(&dir).read().expect("read").expect("present");
        assert_eq!(read.model_count(), catalog.model_count());
        assert!(meta.is_fresh(DEFAULT_TTL));
    }

    #[tokio::test]
    async fn offline_without_a_cache_uses_the_bundle() {
        let dir = temp_dir();
        let loaded = load_catalog(&LoadOptions {
            url: "http://127.0.0.1:1/catalog.json",
            ttl: DEFAULT_TTL,
            offline: true,
            cache_dir: &dir,
        })
        .await
        .expect("load");
        assert_eq!(loaded.source, Source::Bundled);
    }

    #[tokio::test]
    async fn offline_with_a_cache_uses_it() {
        let dir = temp_dir();
        write_cache(&dir, Utc::now());
        let loaded = load_catalog(&LoadOptions {
            url: "http://127.0.0.1:1/catalog.json",
            ttl: DEFAULT_TTL,
            offline: true,
            cache_dir: &dir,
        })
        .await
        .expect("load");
        assert_eq!(loaded.source, Source::Cache);
    }

    #[tokio::test]
    async fn a_stale_cache_survives_a_failed_refresh() {
        let dir = temp_dir();
        write_cache(&dir, Utc::now() - chrono::Duration::hours(48));
        let loaded = load_catalog(&LoadOptions {
            url: "http://127.0.0.1:1/catalog.json",
            ttl: DEFAULT_TTL,
            offline: false,
            cache_dir: &dir,
        })
        .await
        .expect("load");
        assert_eq!(loaded.source, Source::StaleCache);
    }

    #[tokio::test]
    async fn a_corrupt_cache_falls_back_to_the_bundle() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).expect("dir");
        fs::write(dir.join("catalog.json"), b"not json").expect("write");
        let loaded = load_catalog(&LoadOptions {
            url: "http://127.0.0.1:1/catalog.json",
            ttl: DEFAULT_TTL,
            offline: true,
            cache_dir: &dir,
        })
        .await
        .expect("load");
        assert_eq!(loaded.source, Source::Bundled);
    }
}
