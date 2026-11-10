//! The catalogue loader: the files, their bounds, and the error a bad one names.

use std::collections::BTreeMap;
use std::path::PathBuf;

use norte_code_wire::catalog::{
    MAX_CATALOG_BYTES, MAX_CONTEXT, MAX_OUTPUT, Model, ModelEntry, Models, Provider, Providers,
};

/// A failure while reading the catalogue.
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    /// A file is larger than this build accepts.
    #[error("{path} is {size} bytes, over the {max}-byte cap")]
    TooLarge {
        /// The file.
        path: PathBuf,
        /// The size found.
        size: usize,
        /// The cap.
        max: usize,
    },
    /// Neither `XDG_CONFIG_HOME` nor `HOME` is set, so there is nowhere to look.
    #[error("{message}")]
    NoDirectory {
        /// What is missing, as the path helpers word it.
        message: String,
    },
    /// A file could not be read.
    #[error("could not read {path}: {source}")]
    Read {
        /// The path.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// A file could not be written.
    #[error("could not write {path}: {source}")]
    Write {
        /// The path.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// A file is not valid JSON, or does not match the shape.
    #[error("{path} is not a valid catalogue: {source}")]
    Parse {
        /// The path.
        path: PathBuf,
        /// The underlying error.
        source: serde_json::Error,
    },
    /// No providers are configured.
    #[error(
        "{path} defines no providers; Norte Code needs one with a wire before it can talk to a model"
    )]
    NoProviders {
        /// The path.
        path: PathBuf,
    },
    /// No models are configured.
    #[error(
        "{path} defines no models; Norte Code needs one to open a session on (add an entry, or write the file with a plugin)"
    )]
    NoModels {
        /// The path.
        path: PathBuf,
    },
    /// A model names a provider that is not configured.
    #[error("the model `{model}` names the provider `{provider}`, which {path} does not define")]
    UnknownProvider {
        /// The model.
        model: String,
        /// The provider it names.
        provider: String,
        /// The providers file.
        path: PathBuf,
    },
    /// A model is served by a provider with no wire of the requested schema.
    #[error(
        "the provider `{provider}` serves no `{wanted}` wire; it offers {offered}. Name one with --wire"
    )]
    NoSuchWire {
        /// The provider.
        provider: String,
        /// The schema that was wanted.
        wanted: String,
        /// The schemas it does offer, as a list.
        offered: String,
    },
}

/// The catalogue: the providers and the models, read from their two files.
///
/// It is built from bytes and holds no path and no way to refresh itself. What
/// it is read from, when it is re-read and what to do when it is missing are
/// the caller's questions, and keeping them out is what lets a plugin build the
/// same type in memory from an endpoint of its own.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    providers: BTreeMap<String, Provider>,
    /// Every model, in the order the file listed them.
    models: Vec<Model>,
    /// Model key — `provider/id` — to its place in `models`.
    index: BTreeMap<String, usize>,
}

impl Catalog {
    /// The providers, keyed by id.
    #[must_use]
    pub fn provider_entries(&self) -> &BTreeMap<String, Provider> {
        &self.providers
    }

    /// Every model, in file order.
    #[must_use]
    pub fn model_entries(&self) -> &[Model] {
        &self.models
    }

    /// Reads the two files a person owns.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError`] when either file is unreadable, too large, not
    /// valid JSON, or carries nothing: a catalogue with no providers or no
    /// models cannot open a session, and failing here says which file to write
    /// rather than failing later on the first message.
    pub fn load(
        providers: &std::path::Path,
        models: &std::path::Path,
    ) -> Result<Self, CatalogError> {
        let provider_bytes = read_limited(providers)?;
        let model_bytes = read_limited(models)?;
        let listed: Providers = parse(providers, &provider_bytes)?;
        let entries: Models = parse(models, &model_bytes)?;
        // The file that is empty is named here, because this is the error a
        // person meets on a first run and the path is what tells them where to
        // write. `new` has no file to name: it builds a catalogue in memory.
        if listed.providers.is_empty() {
            return Err(CatalogError::NoProviders {
                path: providers.to_path_buf(),
            });
        }
        if entries.models.is_empty() {
            return Err(CatalogError::NoModels {
                path: models.to_path_buf(),
            });
        }
        Self::new(listed, entries)
    }

    /// Builds a catalogue in memory, for a plugin that read the data itself.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogError`] for the same reasons [`Catalog::load`] does,
    /// except that nothing was read from disk: the two shapes must still name
    /// providers that exist and carry at least one of each.
    pub fn new(providers: Providers, models: Models) -> Result<Self, CatalogError> {
        let source = PathBuf::from("the catalogue");
        if providers.providers.is_empty() {
            return Err(CatalogError::NoProviders { path: source });
        }
        if models.models.is_empty() {
            return Err(CatalogError::NoModels { path: source });
        }
        let mut catalog = Self {
            providers: providers.providers,
            models: models.models,
            index: BTreeMap::new(),
        };
        catalog.normalize()?;
        Ok(catalog)
    }

    /// Adds the providers and models a plugin offered.
    ///
    /// What is added lives for this run: the files a person owns are not
    /// written and do not have to be. A provider the files already carry is
    /// left as it is — the file is what the person chose — and so is a model
    /// whose key is taken.
    ///
    /// What was left out is returned, and it says why: a model that silently
    /// did not arrive is a model somebody will look for in the list and not
    /// find.
    pub fn extend(
        &mut self,
        providers: std::collections::BTreeMap<String, Provider>,
        models: Vec<Model>,
    ) -> Vec<String> {
        let mut skipped = Vec::new();
        for (id, provider) in providers {
            if self.providers.contains_key(&id) {
                skipped.push(format!("provider {id}: the catalogue already carries one"));
                continue;
            }
            self.providers.insert(id, provider);
        }
        for mut model in models {
            if !self.providers.contains_key(&model.provider_id) {
                skipped.push(format!(
                    "model {}: no provider named `{}`",
                    model.id, model.provider_id
                ));
                continue;
            }
            let key = model.key();
            if self.index.contains_key(&key) {
                skipped.push(format!("model {key}: the catalogue already carries one"));
                continue;
            }
            if model.name.trim().is_empty() {
                model.name.clone_from(&model.id);
            }
            if model.canonical_slug.trim().is_empty() {
                model.canonical_slug.clone_from(&key);
            }
            self.index.insert(key, self.models.len());
            self.models.push(model);
        }
        skipped
    }

    /// Fills in the names a file left empty and indexes the models.
    fn normalize(&mut self) -> Result<(), CatalogError> {
        for (id, provider) in &mut self.providers {
            if provider.name.trim().is_empty() {
                provider.name.clone_from(id);
            }
        }
        self.index.clear();
        for (position, model) in self.models.iter_mut().enumerate() {
            if !self.providers.contains_key(&model.provider_id) {
                return Err(CatalogError::UnknownProvider {
                    model: model.id.clone(),
                    provider: model.provider_id.clone(),
                    path: PathBuf::from("providers.json"),
                });
            }
            if model.name.trim().is_empty() {
                model.name.clone_from(&model.id);
            }
            if model.canonical_slug.trim().is_empty() {
                model.canonical_slug = model.key();
            }
            self.index.insert(model.key(), position);
        }
        Ok(())
    }

    /// Checks the numbers and drops implausible ones, returning what changed.
    ///
    /// A missing context window makes compaction refuse for that model instead
    /// of guessing. Zero is the catalogue's way of saying "not applicable" for
    /// image, audio and embedding models, so it is cleared silently; only a
    /// genuinely absurd number is worth a warning.
    pub fn validate(&mut self) -> Vec<String> {
        let mut warnings = Vec::new();
        for model in &mut self.models {
            if let Some(context) = model.limit.context {
                if context == 0 {
                    model.limit.context = None;
                } else if context > MAX_CONTEXT {
                    warnings.push(format!(
                        "{}: ignoring an implausible context window of {context}",
                        model.key()
                    ));
                    model.limit.context = None;
                }
            }
            if let Some(output) = model.limit.output {
                if output == 0 {
                    model.limit.output = None;
                } else if output > MAX_OUTPUT {
                    warnings.push(format!(
                        "{}: ignoring an implausible output limit of {output}",
                        model.key()
                    ));
                    model.limit.output = None;
                }
            }
        }
        warnings
    }

    /// Every model, with the provider that serves it.
    pub fn models(&self) -> impl Iterator<Item = ModelEntry<'_>> {
        self.models.iter().filter_map(|model| {
            self.providers
                .get(&model.provider_id)
                .map(|provider| ModelEntry { provider, model })
        })
    }

    /// One model, by provider id and model id.
    #[must_use]
    pub fn find(&self, provider: &str, model: &str) -> Option<&Model> {
        self.index
            .get(&format!("{provider}/{model}"))
            .map(|position| &self.models[*position])
    }

    /// Every model a provider serves, in file order.
    pub fn models_of(&self, provider: &str) -> impl Iterator<Item = &Model> {
        self.models
            .iter()
            .filter(move |model| model.provider_id == provider)
    }

    /// The models whose id is exactly this, from any provider.
    ///
    /// Two providers may serve the same id, and an id alone does not say which
    /// is meant: the caller that has only a name gets every candidate and is
    /// the one that knows whether a choice is needed.
    #[must_use]
    pub fn find_by_id(&self, model: &str) -> Vec<&Model> {
        self.models
            .iter()
            .filter(|entry| entry.id == model)
            .collect()
    }

    /// One provider, by id.
    #[must_use]
    pub fn provider(&self, provider: &str) -> Option<&Provider> {
        self.providers.get(provider)
    }

    /// Every provider, by id.
    #[must_use]
    pub fn providers(&self) -> &BTreeMap<String, Provider> {
        &self.providers
    }

    /// Every model, in file order.
    #[must_use]
    pub fn all_models(&self) -> &[Model] {
        &self.models
    }

    /// How many models the catalogue carries.
    #[must_use]
    pub fn model_count(&self) -> usize {
        self.models.len()
    }
}

/// Reads the two catalogue files from where they belong.
///
/// It is the one place the paths and the reading meet, so the host and the
/// service cannot disagree about which file is which: both call this. A file
/// that is missing is an error naming it: the host writes the two files when
/// they are not there, and a service that reloads a catalogue somebody deleted
/// keeps the one it has.
///
/// # Errors
///
/// Returns [`CatalogError`] when the configuration directory cannot be located,
/// when either file is missing, unreadable or malformed, or when either
/// carries nothing.
pub fn load_catalog_files() -> Result<Catalog, CatalogError> {
    let providers =
        super::source::providers_path().map_err(|message| CatalogError::NoDirectory { message })?;
    let models =
        super::source::models_path().map_err(|message| CatalogError::NoDirectory { message })?;
    Catalog::load(&providers, &models)
}

/// Reads the two files as they are, for an edit that writes them back.
///
/// Unlike [`Catalog::load`] an empty file is not an error: a person adding the
/// first provider has to start from something.
///
/// # Errors
///
/// Returns [`CatalogError`] when a file is unreadable, too large or malformed.
pub fn load_catalog_raw() -> Result<(Providers, Models), CatalogError> {
    let providers_path =
        super::source::providers_path().map_err(|message| CatalogError::NoDirectory { message })?;
    let models_path =
        super::source::models_path().map_err(|message| CatalogError::NoDirectory { message })?;
    let providers: Providers = match read_limited(&providers_path) {
        Ok(bytes) => parse(&providers_path, &bytes)?,
        Err(CatalogError::Read { .. }) => Providers::default(),
        Err(error) => return Err(error),
    };
    let models: Models = match read_limited(&models_path) {
        Ok(bytes) => parse(&models_path, &bytes)?,
        Err(CatalogError::Read { .. }) => Models::default(),
        Err(error) => return Err(error),
    };
    Ok((providers, models))
}

/// Writes the two catalogue files back, after an edit made through the host.
///
/// The edit itself is applied to the in-memory values first, so a write that
/// fails leaves the files as they were; a directory that does not exist is
/// created, because a first run may not have made one yet.
///
/// # Errors
///
/// Returns [`CatalogError::Write`] when either file cannot be written.
pub fn save_catalog_files(providers: &Providers, models: &Models) -> Result<(), CatalogError> {
    use std::io::Write as _;

    fn write_atomic(path: &std::path::Path, body: &str) -> Result<(), CatalogError> {
        let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        std::fs::create_dir_all(parent).map_err(|source| CatalogError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
        let temp = path.with_extension("json.tmp");
        {
            let mut file = std::fs::File::create(&temp).map_err(|source| CatalogError::Write {
                path: temp.clone(),
                source,
            })?;
            file.write_all(body.as_bytes())
                .map_err(|source| CatalogError::Write {
                    path: temp.clone(),
                    source,
                })?;
            file.sync_all().ok();
        }
        std::fs::rename(&temp, path).map_err(|source| CatalogError::Write {
            path: path.to_path_buf(),
            source,
        })
    }

    let providers_path =
        super::source::providers_path().map_err(|message| CatalogError::NoDirectory { message })?;
    let models_path =
        super::source::models_path().map_err(|message| CatalogError::NoDirectory { message })?;

    let providers_body = serde_json::to_string_pretty(providers).unwrap_or_default();
    let models_body = serde_json::to_string_pretty(models).unwrap_or_default();
    write_atomic(&providers_path, &providers_body)?;
    write_atomic(&models_path, &models_body)
}

/// Reads a file with a hard size cap, so a pathological file cannot be read
/// into memory before it is rejected.
fn read_limited(path: &std::path::Path) -> Result<Vec<u8>, CatalogError> {
    let metadata = std::fs::metadata(path).map_err(|source| CatalogError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.len() > MAX_CATALOG_BYTES as u64 {
        return Err(CatalogError::TooLarge {
            path: path.to_path_buf(),
            size: usize::try_from(metadata.len()).unwrap_or(usize::MAX),
            max: MAX_CATALOG_BYTES,
        });
    }
    let bytes = std::fs::read(path).map_err(|source| CatalogError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    if bytes.len() > MAX_CATALOG_BYTES {
        return Err(CatalogError::TooLarge {
            path: path.to_path_buf(),
            size: bytes.len(),
            max: MAX_CATALOG_BYTES,
        });
    }
    Ok(bytes)
}

/// Parses one of the two files, naming it in the error.
fn parse<T: serde::de::DeserializeOwned>(
    path: &std::path::Path,
    bytes: &[u8],
) -> Result<T, CatalogError> {
    serde_json::from_slice(bytes).map_err(|source| CatalogError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use norte_code_wire::catalog::{
        Limit, Modalities, Modality, ModelStatus, SupportedEffort, SupportedParams, Wire,
        WireSchema,
    };

    /// A provider that serves one `OpenAI` wire, for a test that needs a shape
    /// and not a file.
    fn provider(name: &str) -> Provider {
        Provider {
            name: name.to_owned(),
            doc: None,
            environment_keys: vec![format!("{}_API_KEY", name.to_uppercase())],
            wires: vec![Wire {
                api: "https://example.invalid/v1".to_owned(),
                schema: WireSchema::OpenAI,
                auth: None,
            }],
        }
    }

    fn model(id: &str, provider_id: &str) -> Model {
        Model {
            id: id.to_owned(),
            provider_id: provider_id.to_owned(),
            canonical_slug: format!("{provider_id}/{id}"),
            name: id.to_owned(),
            limit: Limit {
                context: Some(128_000),
                output: Some(8_192),
            },
            modalities: Modalities {
                input: vec![Modality::Text],
                output: vec![Modality::Text],
            },
            ..Model::default()
        }
    }

    fn catalog() -> Catalog {
        let mut providers = BTreeMap::new();
        providers.insert("openai".to_owned(), provider("openai"));
        providers.insert("anthropic".to_owned(), provider("anthropic"));
        Catalog::new(
            Providers { providers },
            Models {
                models: vec![model("gpt-4o", "openai"), model("claude-4-5", "anthropic")],
            },
        )
        .expect("catalogue")
    }

    #[test]
    fn a_flat_list_keeps_two_providers_apart() {
        // The reason the list is flat: the same name from two providers is two
        // models, and neither shadows the other.
        let catalog = catalog();
        assert_eq!(
            catalog.find("openai", "gpt-4o").expect("model").id,
            "gpt-4o"
        );
        assert_eq!(catalog.model_count(), 2);
        assert_eq!(catalog.models_of("openai").count(), 1);
        let same = catalog.find_by_id("gpt-4o");
        assert_eq!(same.len(), 1);
        assert_eq!(same[0].provider_id, "openai");
    }

    #[test]
    fn a_model_naming_an_unknown_provider_is_rejected() {
        let mut providers = BTreeMap::new();
        providers.insert("openai".to_owned(), provider("openai"));
        let error = Catalog::new(
            Providers { providers },
            Models {
                models: vec![model("m", "nowhere")],
            },
        )
        .expect_err("unknown provider");
        assert!(
            matches!(error, CatalogError::UnknownProvider { .. }),
            "{error}"
        );
    }

    #[test]
    fn an_empty_catalogue_is_rejected() {
        let error = Catalog::new(Providers::default(), Models::default()).expect_err("empty");
        assert!(matches!(error, CatalogError::NoProviders { .. }), "{error}");
    }

    #[test]
    fn implausible_limits_are_dropped() {
        let mut cat = catalog();
        cat.models[0].limit.context = Some(MAX_CONTEXT + 1);
        let warnings = cat.validate();
        assert_eq!(warnings.len(), 1);
        assert!(
            cat.find("openai", "gpt-4o")
                .expect("model")
                .limit
                .context
                .is_none()
        );
    }

    #[test]
    fn the_preferred_wire_is_the_openai_one() {
        // The order a person gets when they named nothing.
        let mut entry = provider("many");
        entry.wires = vec![
            Wire {
                api: "https://example.invalid/google".to_owned(),
                schema: WireSchema::Google,
                auth: None,
            },
            Wire {
                api: "https://example.invalid/anthropic".to_owned(),
                schema: WireSchema::Anthropic,
                auth: None,
            },
            Wire {
                api: "https://example.invalid/openai".to_owned(),
                schema: WireSchema::OpenAI,
                auth: None,
            },
        ];
        assert_eq!(
            entry.preferred_wire().expect("wire").schema,
            WireSchema::OpenAI
        );
        assert_eq!(
            entry.wire(WireSchema::Google).expect("google").api,
            "https://example.invalid/google"
        );
        assert!(entry.wire(WireSchema::Anthropic).is_some());
        assert_eq!(entry.offered(), "google, anthropic, openai");
    }

    #[test]
    fn a_slug_and_a_name_default_to_the_id() {
        let catalog = Catalog::new(
            Providers {
                providers: BTreeMap::from([("p".to_owned(), provider("p"))]),
            },
            Models {
                models: vec![Model {
                    id: "m".to_owned(),
                    provider_id: "p".to_owned(),
                    ..Model::default()
                }],
            },
        )
        .expect("catalogue");
        let model = catalog.find("p", "m").expect("model");
        assert_eq!(model.name, "m");
        assert_eq!(model.canonical_slug, "p/m");
        assert_eq!(model.key(), "p/m");
    }

    #[test]
    fn the_enums_read_the_names_a_file_spells() {
        // The names are the contract with whoever writes the file, so they are
        // read as they are written and not as Rust spells them.
        let models: Models = serde_json::from_str(
            r#"{"models":[{"id":"m","provider_id":"p","modalities":{"input":["text","pdf"]},
                "supported_params":["tools","temperature","structured_output"],
                "reasoning":{"supported_efforts":["low","xhigh"]},
                "status":"deprecated"}]}"#,
        )
        .expect("models");
        let model = &models.models[0];
        assert_eq!(model.modalities.input, vec![Modality::Text, Modality::File]);
        assert!(model.supported_params.contains(&SupportedParams::Tools));
        assert!(
            model
                .supported_params
                .contains(&SupportedParams::Temperature)
        );
        assert_eq!(
            model.reasoning.supported_efforts,
            vec![SupportedEffort::Low, SupportedEffort::Xhigh]
        );
        assert_eq!(model.status, Some(ModelStatus::Deprecated));
    }

    #[test]
    fn an_unknown_name_in_a_list_is_refused() {
        // The reason these are enums: a typo has to be caught where the file is
        // read, not silently become "the model does not support that".
        let error = serde_json::from_str::<Models>(
            r#"{"models":[{"id":"m","provider_id":"p","supported_params":["temperatur"]}]}"#,
        )
        .expect_err("typo");
        assert!(error.to_string().contains("temperatur"), "{error}");
    }

    #[test]
    fn what_a_plugin_offers_joins_the_catalogue() {
        // The files a person owns stay what they are, and what a plugin serves
        // is in the catalogue for this run.
        let mut catalog = catalog();
        let skipped = catalog.extend(
            BTreeMap::from([("demo".to_owned(), provider("demo"))]),
            vec![model("demo-1", "demo")],
        );
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(catalog.model_count(), 3);
        assert!(
            catalog.find("demo", "demo-1").is_some(),
            "the plugin's model is in the catalogue"
        );
    }

    #[test]
    fn what_the_files_already_carry_is_not_replaced() {
        // The person chose what the files say, and a plugin that offers the
        // same thing does not overwrite it: what was left out is reported.
        let mut catalog = catalog();
        let skipped = catalog.extend(
            BTreeMap::from([("openai".to_owned(), provider("other"))]),
            vec![model("gpt-4o", "openai")],
        );
        assert_eq!(skipped.len(), 2, "{skipped:?}");
        assert_eq!(catalog.model_count(), 2);
    }

    #[test]
    fn a_model_naming_a_provider_nobody_offers_is_left_out() {
        // A model with no provider is a model nobody can call, and it is
        // reported rather than carried.
        let mut catalog = catalog();
        let skipped = catalog.extend(BTreeMap::new(), vec![model("orphan", "nowhere")]);
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert!(skipped[0].contains("nowhere"), "{skipped:?}");
        assert_eq!(catalog.model_count(), 2);
    }
}
