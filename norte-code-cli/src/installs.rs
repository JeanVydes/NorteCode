//! Plugin installs that run while the interface stays alive.
//!
//! Cloning a repository and building it takes minutes. The interface has to
//! stay usable while it does: an install that blocked the loop would leave a
//! person staring at a frozen screen with no way to tell whether it was
//! working or the client had died. The build therefore runs on a blocking
//! thread of its own, and what it ended with is left where the loop will find
//! it.
//!
//! The output of the clone and of the build is captured rather than printed:
//! the terminal shows the client, and the error that comes out of a failed
//! build is the one thing worth reading.

use std::sync::Arc;

use crate::input::{InstallReport, Pinned};

/// Starts the installation of a plugin and returns immediately.
///
/// Nothing is waited for here. The clone, the build and the copy happen on the
/// blocking thread, and the outcome reaches the conversation when it is done.
pub(crate) fn start(url: String, pinned: Arc<Pinned>) {
    let name = plugin_name(&url);
    pinned.set_installing(name.clone());
    tokio::spawn(async move {
        let built = url.clone();
        let outcome =
            tokio::task::spawn_blocking(move || crate::plugin_manager::install_plugin(&built))
                .await;
        let report = match outcome {
            Ok(Ok(library)) => InstallReport {
                name,
                result: Ok(library),
            },
            Ok(Err(error)) => InstallReport {
                name,
                result: Err(error),
            },
            Err(error) => InstallReport {
                name,
                result: Err(format!("the build stopped before it finished: {error}")),
            },
        };
        pinned.finish_install(report);
    });
}

/// The name of the plugin a URL names.
///
/// The last segment of the path, without the `.git` a clone URL may carry: it
/// is what the person recognises the plugin by, and what the footer shows
/// while the build runs.
pub(crate) fn plugin_name(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    let last = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let name = last.strip_suffix(".git").unwrap_or(last);
    if name.is_empty() {
        "plugin".to_owned()
    } else {
        name.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_is_the_last_segment_of_the_url() {
        assert_eq!(
            plugin_name("https://github.com/JeanVydes/norte-example-plugin"),
            "norte-example-plugin"
        );
    }

    #[test]
    fn a_clone_suffix_is_not_part_of_the_name() {
        assert_eq!(
            plugin_name("https://github.com/JeanVydes/norte-example-plugin.git"),
            "norte-example-plugin"
        );
    }

    #[test]
    fn a_trailing_slash_does_not_hide_the_name() {
        assert_eq!(
            plugin_name("https://github.com/JeanVydes/norte-example-plugin/"),
            "norte-example-plugin"
        );
    }
}
