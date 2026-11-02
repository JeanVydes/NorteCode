#![forbid(unsafe_code)]
#![allow(missing_docs)]
#![allow(clippy::empty_line_after_doc_comments)]
#![allow(clippy::missing_errors_doc)]
#![allow(unused_imports)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub fn discovery_roots(workspace: &Path, extra: &[PathBuf], interop: bool) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    // Bundled examples ship next to the executable; they are the lowest
    // precedence root and are skipped when the directory is absent.
    if let Ok(executable) = std::env::current_exe()
        && let Some(directory) = executable.parent()
    {
        roots.push(directory.join("assets").join("skills"));
    }
    let config_base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    if let Some(base) = config_base {
        roots.push(base.join("norte-code").join("skills"));
    }
    if interop && let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join(".agents").join("skills"));
    }
    for path in extra {
        roots.push(if path.is_absolute() {
            path.clone()
        } else {
            workspace.join(path)
        });
    }
    roots.push(workspace.join(".norte-code").join("skills"));
    if interop {
        roots.push(workspace.join(".agents").join("skills"));
    }
    roots
}

/// The frontmatter as the specification defines it.
#[derive(Debug, Deserialize)]
pub(super) struct Frontmatter {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub compatibility: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    #[serde(default, rename = "allowed-tools")]
    pub allowed_tools: Option<String>,
}
