//! The catalogue's JSON shape.
//!
//! Everything is `#[serde(default)]` and unknown fields are ignored: the
//! catalogue is community data and grows fields without warning. A few numbers
//! are sanity-checked on load so one absurd entry cannot make the UI propose a
//! 40-billion-token context window.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize};

/// Reads a string, mapping `null` to the empty string.
///
/// The catalogue is community data; a missing or null display name must not
/// reject six megabytes of otherwise usable entries.
fn string_or_default<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

/// Reads a list of strings, dropping null elements and tolerating a null list.
fn string_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let values = Option::<Vec<Option<String>>>::deserialize(deserializer)?;
    Ok(values.unwrap_or_default().into_iter().flatten().collect())
}

/// The largest catalogue this build will parse. The live file is about 6 MiB.
pub const MAX_CATALOG_BYTES: usize = 32 * 1024 * 1024;

/// The largest context window a sane model can claim.
pub const MAX_CONTEXT: u64 = 100_000_000;

/// The largest output limit a sane model can claim.
pub const MAX_OUTPUT: u64 = 10_000_000;

/// The whole catalogue. The top-level `models` index is ignored: the model
/// entries under each provider carry the full data.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Catalog {
    /// Provider id to provider.
    pub providers: BTreeMap<String, Provider>,
}

/// One provider and its models.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Provider {
    /// The provider id, as in `openrouter` or `lilac`.
    #[serde(default, deserialize_with = "string_or_default")]
    pub id: String,
    /// The display name.
    #[serde(default, deserialize_with = "string_or_default")]
    pub name: String,
    /// The environment variables that may hold the key, in order.
    #[serde(default, deserialize_with = "string_list")]
    pub env: Vec<String>,
    /// The models.dev SDK package; it selects the wire.
    pub npm: Option<String>,
    /// The provider's API base URL, when the catalogue carries one.
    pub api: Option<String>,
    /// A documentation URL.
    pub doc: Option<String>,
    /// Model id to model.
    pub models: BTreeMap<String, Model>,
}

/// One model entry.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Model {
    /// The model id the provider expects.
    #[serde(default, deserialize_with = "string_or_default")]
    pub id: String,
    /// The display name.
    #[serde(default, deserialize_with = "string_or_default")]
    pub name: String,
    /// A one-line description.
    pub description: Option<String>,
    /// The model family.
    pub family: Option<String>,
    /// Whether the model accepts attachments.
    pub attachment: bool,
    /// Whether the model reasons.
    pub reasoning: bool,
    /// The reasoning controls the provider documents.
    pub reasoning_options: Vec<ReasoningOption>,
    /// Whether the model calls host functions.
    pub tool_call: bool,
    /// Whether the model supports structured output.
    pub structured_output: Option<bool>,
    /// Whether the model accepts a temperature.
    pub temperature: Option<bool>,
    /// The knowledge cutoff.
    pub knowledge: Option<String>,
    /// The release date.
    pub release_date: Option<String>,
    /// The last update date.
    pub last_updated: Option<String>,
    /// Input and output modalities.
    pub modalities: Modalities,
    /// Whether the weights are open.
    pub open_weights: Option<bool>,
    /// Context and output limits.
    pub limit: Limit,
    /// `deprecated`, `beta`, or absent.
    pub status: Option<String>,
}

/// Input and output modalities, as the catalogue spells them.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Modalities {
    /// Accepted inputs (`text`, `image`, `audio`, `video`, `pdf`).
    #[serde(default, deserialize_with = "string_list")]
    pub input: Vec<String>,
    /// Produced outputs.
    #[serde(default, deserialize_with = "string_list")]
    pub output: Vec<String>,
}

/// Context and output token limits.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Limit {
    /// The context window.
    pub context: Option<u64>,
    /// The output limit.
    pub output: Option<u64>,
}

/// One reasoning control the catalogue documents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReasoningOption {
    /// Named effort levels.
    Effort {
        /// The names, such as `low` or `xhigh`.
        #[serde(default, deserialize_with = "string_list")]
        values: Vec<String>,
    },
    /// A token budget.
    BudgetTokens {
        /// The smallest accepted budget. The catalogue uses `-1` for "none".
        #[serde(default)]
        min: Option<i64>,
        /// The largest accepted budget. The catalogue uses `-1` for "none".
        #[serde(default)]
        max: Option<i64>,
    },
    /// An on/off switch.
    Toggle,
    /// A control this build does not understand.
    #[serde(other)]
    Unknown,
}

/// A model together with its provider, for listing.
#[derive(Clone, Copy, Debug)]
pub struct ModelEntry<'a> {
    /// The provider.
    pub provider: &'a Provider,
    /// The model.
    pub model: &'a Model,
}

/// A catalogue failure.
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    /// The file is larger than this build accepts.
    #[error("the catalogue is {size} bytes, over the {max}-byte cap")]
    TooLarge {
        /// The size found.
        size: usize,
        /// The cap.
        max: usize,
    },
    /// The JSON did not match the shape.
    #[error("the catalogue is not valid JSON: {0}")]
    Parse(#[from] serde_json::Error),
    /// The file parsed but carries nothing.
    #[error("the catalogue has no providers")]
    Empty,
    /// A cache file could not be read.
    #[error("could not read {path}: {source}")]
    Read {
        /// The path.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// A cache file could not be written.
    #[error("could not write {path}: {source}")]
    Write {
        /// The path.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The HTTP request failed.
    #[error("the catalogue request failed: {0}")]
    Http(String),
}

impl Catalog {
    /// Parses a catalogue, normalises ids and rejects an empty one.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError`] when the bytes are too large, are not valid
    /// JSON, or carry no providers.
    pub fn parse(bytes: &[u8]) -> Result<Self, CatalogError> {
        if bytes.len() > MAX_CATALOG_BYTES {
            return Err(CatalogError::TooLarge {
                size: bytes.len(),
                max: MAX_CATALOG_BYTES,
            });
        }
        let mut catalog: Self = serde_json::from_slice(bytes)?;
        if catalog.providers.is_empty() {
            return Err(CatalogError::Empty);
        }
        catalog.normalize();
        Ok(catalog)
    }

    /// The snapshot compiled into the binary, used when nothing else works.
    ///
    /// # Panics
    ///
    /// Panics only if the committed snapshot is invalid; a test parses it.
    #[must_use]
    pub fn bundled() -> Self {
        let mut catalog = Self::parse(include_bytes!("../assets/catalog.json"))
            .expect("the bundled catalogue parses");
        catalog.validate();
        catalog
    }

    /// Fills in ids and names the catalogue left empty from their map keys.
    fn normalize(&mut self) {
        for (provider_key, provider) in &mut self.providers {
            if provider.id.is_empty() {
                provider.id.clone_from(provider_key);
            }
            if provider.name.is_empty() {
                provider.name.clone_from(provider_key);
            }
            for (model_key, model) in &mut provider.models {
                if model.id.is_empty() {
                    model.id.clone_from(model_key);
                }
                if model.name.is_empty() {
                    model.name.clone_from(model_key);
                }
            }
        }
    }

    /// Checks the numbers and drops implausible ones, returning what changed.
    ///
    /// A missing context window makes compaction refuse for that model instead
    /// of guessing. Zero is the catalogue's way of saying "not applicable" for
    /// image, audio and embedding models, so it is cleared silently; only a
    /// genuinely absurd number is worth a warning.
    pub fn validate(&mut self) -> Vec<String> {
        let mut warnings = Vec::new();
        for (provider_id, provider) in &mut self.providers {
            for (model_id, model) in &mut provider.models {
                if let Some(context) = model.limit.context {
                    if context == 0 {
                        model.limit.context = None;
                    } else if context > MAX_CONTEXT {
                        warnings.push(format!(
                            "{provider_id}/{model_id}: ignoring an implausible context window of {context}"
                        ));
                        model.limit.context = None;
                    }
                }
                if let Some(output) = model.limit.output {
                    if output == 0 {
                        model.limit.output = None;
                    } else if output > MAX_OUTPUT {
                        warnings.push(format!(
                            "{provider_id}/{model_id}: ignoring an implausible output limit of {output}"
                        ));
                        model.limit.output = None;
                    }
                }
            }
        }
        warnings
    }

    /// Every model in every provider.
    pub fn models(&self) -> impl Iterator<Item = ModelEntry<'_>> {
        self.providers.values().flat_map(|provider| {
            provider
                .models
                .values()
                .map(move |model| ModelEntry { provider, model })
        })
    }

    /// One model, by provider id and model id.
    #[must_use]
    pub fn find(&self, provider: &str, model: &str) -> Option<&Model> {
        self.providers.get(provider)?.models.get(model)
    }

    /// One provider, by id.
    #[must_use]
    pub fn provider(&self, provider: &str) -> Option<&Provider> {
        self.providers.get(provider)
    }

    /// How many models the catalogue carries.
    #[must_use]
    pub fn model_count(&self) -> usize {
        self.providers.values().map(|p| p.models.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_snapshot_parses_and_has_the_four_providers() {
        let catalog = Catalog::bundled();
        for provider in ["openai", "anthropic", "google", "openrouter"] {
            assert!(catalog.provider(provider).is_some(), "{provider}");
        }
        assert!(catalog.model_count() > 0);
    }

    #[test]
    fn an_empty_catalogue_is_rejected() {
        let error = Catalog::parse(br#"{"providers":{}}"#).expect_err("empty");
        assert!(matches!(error, CatalogError::Empty));
    }

    #[test]
    fn implausible_limits_are_dropped() {
        let mut catalog = Catalog::parse(
            br#"{"providers":{"p":{"models":{"m":{"limit":{"context":999999999999}}}}}}"#,
        )
        .expect("catalog");
        let warnings = catalog.validate();
        assert_eq!(warnings.len(), 1);
        assert!(
            catalog
                .find("p", "m")
                .expect("model")
                .limit
                .context
                .is_none()
        );
    }

    #[test]
    fn ids_fall_back_to_their_map_keys() {
        let catalog =
            Catalog::parse(br#"{"providers":{"lilac":{"models":{"m1":{}}}}}"#).expect("catalog");
        let provider = catalog.provider("lilac").expect("provider");
        assert_eq!(provider.id, "lilac");
        assert_eq!(provider.models["m1"].id, "m1");
    }
}
