//! What the client shows and how it reaches the service.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ConfigError;
use super::loading::{merge, read_text, read_value};

/// How the client shows what it does.
///
/// A subset of the configuration that only the interface reads: the engine
/// never consults it, and a host with no interface — a script driving the
/// service — is free to ignore the whole block.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CliConfig {
    /// The service endpoint used by the CLI.
    pub service_url: String,
    /// Whether lifecycle commands may manage the local service process.
    pub service_type: ServiceType,
    /// Optional raw bearer token for the service.
    pub service_auth: Option<String>,
    /// Agent identifier sent when attaching to the service.
    pub agent_id: String,
    /// How much of the conversation to show: `low` (the answer alone), `medium`
    /// (also that tools run and that the model is thinking) or `high` (the
    /// reasoning, what each tool was asked and what it returned).
    ///
    /// `--verbose` overrides it for one run. It is a string rather than an enum
    /// so a value the client does not know is reported beside the ones it does,
    /// instead of failing the whole file.
    pub verbosity: String,
    /// What happens to a message typed while a turn is running: `reject`,
    /// `queue` or `interrupt`. An unknown value is reported beside the ones
    /// the client knows instead of failing the whole file, so the
    /// renderer can name them.
    pub queue_mode: String,
    /// How an `edit` call's diff is drawn.
    pub diff_style: DiffStyle,
    /// Configuration shared by the terminal's halftone charts.
    pub charts: ChartConfig,
}

/// The service type configured for the CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ServiceType {
    /// The service is managed as a local process by lifecycle commands.
    #[default]
    Local,
    /// The service is remote and cannot be started or stopped by this CLI.
    Remote,
}

/// The result of loading the client-only `cli.json` file.
#[derive(Clone, Debug)]
pub struct LoadedCli {
    /// The merged interface settings.
    pub config: CliConfig,
    /// Unknown keys, as dotted paths, to report to the client.
    pub warnings: Vec<String>,
}

impl CliConfig {
    /// Loads client settings from a user file and an optional
    /// project overlay. The project file wins for every key it sets.
    ///
    /// Unlike [`Config::load`], this function never reads `service.json`.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when a file cannot be read or is not valid JSON.
    pub fn load(user: Option<&Path>, project: Option<&Path>) -> Result<LoadedCli, ConfigError> {
        let mut value = Value::Object(serde_json::Map::new());
        if let Some(path) = user
            && path.exists()
        {
            merge(&mut value, read_value(path)?);
        }
        if let Some(path) = project
            && path.exists()
        {
            merge(&mut value, read_value(path)?);
        }
        let mut warnings = Vec::new();
        let config: CliConfig = serde_ignored::deserialize(value, |path| {
            warnings.push(path.to_string());
        })
        .map_err(|source| ConfigError::Parse {
            path: PathBuf::from("<cli.json>"),
            source,
        })?;
        Ok(LoadedCli { config, warnings })
    }

    /// Parses client-only interface settings from JSON.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when the JSON is invalid or does not match the
    /// interface schema.
    pub fn from_json(text: &str) -> Result<LoadedCli, ConfigError> {
        let value = read_text(text)?;
        let mut warnings = Vec::new();
        let config: CliConfig = serde_ignored::deserialize(value, |path| {
            warnings.push(path.to_string());
        })
        .map_err(|source| ConfigError::Parse {
            path: PathBuf::from("<cli.json>"),
            source,
        })?;
        Ok(LoadedCli { config, warnings })
    }
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            service_url: "http://127.0.0.1:8787".to_owned(),
            service_type: ServiceType::Local,
            service_auth: None,
            agent_id: "norte-code".to_owned(),
            verbosity: "medium".to_owned(),
            queue_mode: "queue".to_owned(),
            diff_style: DiffStyle::default(),
            charts: ChartConfig::default(),
        }
    }
}

/// Visual settings shared by chart widgets.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ChartConfig {
    /// Contrast of the halftone ramp, from 0 to 100.
    pub contrast: u8,
    /// Probability that a candidate dot is drawn, from 0 to 100.
    pub density: u8,
    /// Character scale used for halftone dots.
    pub dot_size: ChartDotSize,
}

impl Default for ChartConfig {
    fn default() -> Self {
        Self {
            contrast: 75,
            density: 82,
            dot_size: ChartDotSize::Medium,
        }
    }
}

/// Character scale for halftone chart marks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChartDotSize {
    /// Fine single-cell dots.
    Small,
    /// Balanced dots for normal terminal sizes.
    #[default]
    Medium,
    /// Dense dots with stronger visual weight.
    Large,
}

/// How the diff of an `edit` call is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffStyle {
    /// Removed and added lines interleaved, the way a patch reads: one column,
    /// which is what fits a narrow terminal.
    #[default]
    Unified,
    /// The file before and after, side by side. Wider, and the only shape that
    /// shows a change inside a line rather than the two whole lines.
    Split,
}

impl DiffStyle {
    /// The name the configuration uses.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Unified => "unified",
            Self::Split => "split",
        }
    }
}
