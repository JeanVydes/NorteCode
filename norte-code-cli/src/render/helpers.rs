//! Config paths, provider environment names and the small number labels
//! the renderer and the reports share.

use std::path::{Path, PathBuf};

/// The per-project overlay config path, searched from `start` upwards.
pub(crate) fn find_project_config(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|dir| dir.join(".norte-code").join("config.json"))
        .find(|path| path.exists())
}

/// The environment variables a provider is likely to use.
///
/// The catalogue's `env` list is authoritative; this is only for a provider the
/// catalogue does not list.
pub(crate) fn fallback_env(provider: &str) -> Vec<String> {
    let upper: String = provider
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    let mut names = vec![format!("{upper}_API_KEY")];
    if matches!(provider, "google" | "gemini") {
        for name in [
            "GOOGLE_API_KEY",
            "GEMINI_API_KEY",
            "GOOGLE_GENERATIVE_AI_API_KEY",
        ] {
            if !names.iter().any(|existing| existing == name) {
                names.push(name.to_owned());
            }
        }
    }
    names
}

/// A path as it is shown in the footer: under the home directory it loses it.
///
/// It takes the home as an argument rather than reading `HOME` when it is
/// called: `set_var` is unsafe in this edition because a test that changes the
/// environment races every other thread that reads it, and this crate forbids
/// unsafe. The caller that wants the real home reads it once, at the top.
pub(crate) fn short_path_in(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// The same, reading the home directory from the environment.
///
/// The full path is what a person pastes into another shell, and the abbreviated
/// one is what they read: `/home/jean/Projects/NorteCode` is nineteen columns of
/// a footer that has a dozen things to say, and every one of them is more
/// informative than the first thirteen characters of a path. Nothing is lost —
/// `~` is the same path to every shell.
pub(crate) fn short_path(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    short_path_in(path, home.as_deref())
}

#[cfg(test)]
mod short_path_tests {
    use super::short_path_in;

    use std::path::Path;

    #[test]
    fn a_path_under_the_home_directory_is_shown_with_a_tilde() {
        // The footer has a dozen things to say, and nineteen columns of a path
        // is not one of them. `~` is the same path to every shell, so nothing
        // is lost by showing it.
        let home = Path::new("/home/someone");
        assert_eq!(short_path_in(home, Some(home)), "~");
        assert_eq!(
            short_path_in(&home.join("src").join("thing"), Some(home)),
            "~/src/thing"
        );
        // A path outside it is shown whole: there is no tilde for `/etc`.
        assert_eq!(
            short_path_in(Path::new("/etc/hosts"), Some(home)),
            "/etc/hosts"
        );
        // Without a home to compare against, nothing can be abbreviated.
        assert_eq!(short_path_in(&home.join("src"), None), "/home/someone/src");
    }
}

pub(crate) fn human_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        let whole = tokens / 1_000_000;
        let decimal = (tokens % 1_000_000) / 100_000;
        if decimal == 0 {
            format!("{whole}M")
        } else {
            format!("{whole}.{decimal}M")
        }
    } else if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

/// The cache share, always as a percentage with two decimals.
///
/// Two decimals because a cache that stops hitting moves by hundredths and the
/// difference is worth seeing, and one rule — the same everywhere the share is
/// printed — reads better than a format that depends on the value.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn cache_percent_label(part: u64, total: u64) -> String {
    if total == 0 {
        return "0.00%".to_owned();
    }
    format!("{:.2}%", part as f64 / total as f64 * 100.0)
}

/// A share of a total, rounded so `0.4%` does not become a bare `0%`.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn percent_label(part: u64, total: u64) -> String {
    if total == 0 {
        return "0%".to_owned();
    }
    let percent = part as f64 / total as f64 * 100.0;
    if percent >= 10.0 {
        format!("{percent:.0}%")
    } else {
        format!("{percent:.1}%")
    }
}

#[cfg(test)]
mod tests {
    use super::{cache_percent_label, percent_label};

    #[test]
    fn the_cache_share_always_carries_two_decimals() {
        // A cache that stops hitting moves by hundredths, so the hundredths are
        // the point; an exact and an empty share read the same way.
        assert_eq!(cache_percent_label(9999, 10_000), "99.99%");
        assert_eq!(cache_percent_label(9, 10), "90.00%");
        assert_eq!(cache_percent_label(10, 10), "100.00%");
        assert_eq!(cache_percent_label(0, 10), "0.00%");
        assert_eq!(cache_percent_label(5, 0), "0.00%");
    }

    #[test]
    fn the_context_share_keeps_its_own_format() {
        // The dial is a different figure: it does not move by hundredths, so it
        // keeps the shorter form.
        assert_eq!(percent_label(1, 100), "1.0%");
        assert_eq!(percent_label(90, 100), "90%");
        assert_eq!(percent_label(1, 0), "0%");
    }
}
