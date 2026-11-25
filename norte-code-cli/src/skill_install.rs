//! Install the CLI's bundled operations skill.

use std::fs;
use std::io;
use std::path::Path;

use norte_code_protocol::config::{CliConfig, ensure_user_cli_config, user_cli_path};

const SKILL: &str = include_str!("../assets/norte-code/SKILL.md");
const CONFIG_REFERENCE: &str = include_str!("../../CONFIG.md");
const TUI_REFERENCE: &str = include_str!("../assets/norte-code/tui.md");
const DIRECTORY: &str = "norte-code";
const MARKER: &str = ".norte-code-managed";

pub(crate) fn sync() -> Result<(), Box<dyn std::error::Error>> {
    let Some(path) = ensure_user_cli_config()?.or_else(user_cli_path) else {
        return Err(
            io::Error::new(io::ErrorKind::NotFound, "no user configuration directory").into(),
        );
    };
    let config = CliConfig::load(Some(&path), None)?.config;
    let root = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "configuration has no parent"))?
        .join("skills");
    sync_at(&root, config.norte_code_skill)?;
    Ok(())
}

fn sync_at(root: &Path, enabled: bool) -> io::Result<()> {
    let directory = root.join(DIRECTORY);
    let marker = directory.join(MARKER);
    if enabled {
        match fs::create_dir_all(root).and_then(|()| fs::create_dir(&directory)) {
            Ok(()) => {
                fs::write(&marker, b"managed by norte-code-cli\n")?;
                fs::write(directory.join("SKILL.md"), SKILL)?;
                install_references(&directory)?;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && directory.is_dir() => {
                if marker.is_file() && !directory.join("SKILL.md").exists() {
                    fs::write(directory.join("SKILL.md"), SKILL)?;
                }
                if marker.is_file() {
                    install_references(&directory)?;
                }
            }
            Err(error) => return Err(error),
        }
    } else if marker.is_file() {
        match fs::remove_file(directory.join("SKILL.md")) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        for file in ["configuration.md", "tui.md"] {
            match fs::remove_file(directory.join("references").join(file)) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        match fs::remove_dir(directory.join("references")) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(error) => return Err(error),
        }
        fs::remove_file(marker)?;
        match fs::remove_dir(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn install_references(directory: &Path) -> io::Result<()> {
    let references = directory.join("references");
    fs::create_dir_all(&references)?;
    for (name, content) in [
        ("configuration.md", CONFIG_REFERENCE),
        ("tui.md", TUI_REFERENCE),
    ] {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(references.join(name))
        {
            Ok(mut file) => {
                use std::io::Write;
                file.write_all(content.as_bytes())?;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "skill_install_tests.rs"]
mod tests;
