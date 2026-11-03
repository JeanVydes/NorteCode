//! Model selection and the `models` report.

use norte_code_protocol::catalog::WireSchema;

use super::{Catalog, Config, ModelInfo, ModelSpec, ModelsArgs, model_list, reasoning_label};

/// Writes the model list, filtered the way the flags asked.
pub(crate) fn list_models(
    catalog: &Catalog,
    args: &ModelsArgs,
    providers_path: &std::path::Path,
    models_path: &std::path::Path,
) -> Result<u8, String> {
    let mut models = model_list(catalog);
    models.retain(|model| {
        args.provider
            .as_ref()
            .is_none_or(|provider| &model.provider == provider)
    });
    if args.tools {
        models.retain(|model| model.tools);
    }
    if args.reasoning {
        models.retain(|model| model.reasoning.supported);
    }
    models.sort_by(|a, b| {
        (a.provider.as_str(), a.model.as_str()).cmp(&(b.provider.as_str(), b.model.as_str()))
    });

    if args.json {
        for model in &models {
            let line = serde_json::to_string(model).map_err(|error| error.to_string())?;
            println!("{line}");
        }
    } else {
        // The two files are named, and not a "source": there is one source now,
        // and saying which file the list was read from is what tells a person
        // where to add a model.
        eprintln!(
            "catalogue: {} models from {} and {}",
            models.len(),
            providers_path.display(),
            models_path.display()
        );
        for model in &models {
            print_model(model);
        }
    }
    Ok(0)
}

pub(crate) fn print_model(model: &ModelInfo) {
    let reference_text = format!("{}/{}", model.provider, model.model);
    let reference = crate::text::terminal(&reference_text);
    let context = model
        .max_context
        .map_or_else(|| "-".to_owned(), super::human_tokens);
    let tools = if model.tools { "tools" } else { "-" };
    let reasoning_text = reasoning_label(&model.reasoning);
    let reasoning = crate::text::terminal(&reasoning_text);
    let name_text = if model.deprecated {
        format!("{} (deprecated)", model.name)
    } else {
        model.name.clone()
    };
    let name = crate::text::terminal(&name_text);
    println!("{reference:<54} {context:>8}  {tools:<5}  {reasoning:<18} {name}");
}

/// The provider and the model a session runs on.
///
/// Two fields and never one string: a model id may carry slashes of its own —
/// `nvidia/nemotron-3.5-super-vl-120b-a12b` is a model of the `nvidia`
/// provider — so the pair is never joined and taken apart again. The label is
/// the join, and it is a label: it is what a person reads, not what the code
/// carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelChoice {
    /// The provider that serves the model.
    pub(crate) provider: String,
    /// The model id.
    pub(crate) model: String,
}

impl ModelChoice {
    /// A choice of a provider and a model.
    pub(crate) fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }
}

/// The provider and the model a new session opens on.
///
/// The flags win over the choice the startup flow made, each on its own half:
/// `--model` names a model and `--provider` names a provider, so neither takes
/// the other out of a joined string. A half nobody named falls back to the
/// choice, and a choice that was never made falls back to the configuration.
pub(crate) fn session_choice(
    choice: Option<&ModelChoice>,
    provider: Option<String>,
    model: Option<String>,
) -> (Option<String>, Option<String>) {
    (
        provider.or_else(|| choice.map(|choice| choice.provider.clone())),
        model.or_else(|| choice.map(|choice| choice.model.clone())),
    )
}

/// The provider and the model a run opens on, from the flags and the config.
///
/// Neither name is ever split: the provider is its own key and the model is an
/// opaque string that may carry slashes of its own — `nvidia/nemotron` is a
/// model of the `nvidia` provider. `--provider` wins over `default_provider`,
/// `--model` over `default_model`, and each falls back to its own config key; a
/// model with no provider (from either) is an error rather than a default
/// guessed at.
fn resolve_reference(
    config: &Config,
    provider: Option<String>,
    model: Option<String>,
) -> Result<ModelChoice, String> {
    let provider = provider.or_else(|| config.default_provider.clone());
    let provider = provider.ok_or_else(|| {
        "no provider specified: pass --provider or set default_provider in config".to_owned()
    })?;
    let model = model.or_else(|| config.default_model.clone());
    let model = model.ok_or_else(|| {
        "no model selected: pass --model or set default_model in config".to_owned()
    })?;
    Ok(ModelChoice::new(provider, model))
}

/// The spec a run opens on.
///
/// The catalogue is the only place a model comes from, so a reference it does
/// not carry is not resolved from somewhere else: an unknown provider or model
/// is an error naming what is missing, and the fix is an entry in one of the two
/// files rather than a flag that papers over it.
pub(crate) fn resolve_spec(
    config: &Config,
    catalog: &Catalog,
    provider: Option<String>,
    model: Option<String>,
    endpoint: Option<String>,
    wire: Option<WireSchema>,
) -> Result<ModelSpec, String> {
    let choice = resolve_reference(config, provider, model)?;
    if catalog.provider(&choice.provider).is_none() {
        return Err(format!(
            "unknown provider `{}`; add it to providers.json, or pick one of: {}",
            choice.provider,
            catalog
                .providers()
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if catalog.find(&choice.provider, &choice.model).is_none() {
        return Err(format!(
            "the provider `{}` has no model `{}`; add it to models.json",
            choice.provider, choice.model
        ));
    }
    let mut spec = ModelSpec::from_catalog(config, catalog, &choice.provider, &choice.model, wire)
        .map_err(|error| error.to_string())?;
    // `--endpoint` replaces the wire's base URL for one run: a proxy, a local
    // mirror, a staging host. It is the one override left, and it is about
    // *where* the request goes and never about what it says.
    if let Some(endpoint) = endpoint {
        spec.endpoint = endpoint;
    }
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use norte_code_protocol::catalog::{Models, Providers};

    /// A catalogue with one provider that serves one `OpenAI` wire.
    fn catalog() -> Catalog {
        let providers: Providers = serde_json::from_str(
            r#"{"providers":{"p":{"name":"P","environment_keys":["PATH"],
                "wires":[{"api":"https://p.invalid/v1","schema":"openai"}]}}}"#,
        )
        .expect("providers");
        let models: Models = serde_json::from_str(
            r#"{"models":[{"id":"m","provider_id":"p","name":"M",
                "supported_params":["tools"],
                "modalities":{"input":["text"],"output":["text"]},
                "limit":{"context":1000,"output":100}}]}"#,
        )
        .expect("models");
        Catalog::new(providers, models).expect("catalogue")
    }

    #[test]
    fn the_choice_the_flow_made_is_the_pair_the_session_opens_on() {
        // The startup flow writes the choice into the configuration and hands
        // back the same pair, and the session is opened on that pair: a run that
        // dropped it and read the configuration instead died on "no provider
        // specified" right after the question was answered.
        let choice = ModelChoice::new("nvidia", "nvidia/nemotron-3.5-super-vl-120b-a12b");
        let (provider, model) = session_choice(Some(&choice), None, None);
        assert_eq!(provider.as_deref(), Some("nvidia"));
        assert_eq!(
            model.as_deref(),
            Some("nvidia/nemotron-3.5-super-vl-120b-a12b")
        );

        // A flag wins on its own half and leaves the other where it was: a
        // `--model` names a model, not a pair.
        let (provider, model) = session_choice(
            Some(&choice),
            None,
            Some("deepseek-ai/deepseek-v4.1-flash".to_owned()),
        );
        assert_eq!(provider.as_deref(), Some("nvidia"));
        assert_eq!(model.as_deref(), Some("deepseek-ai/deepseek-v4.1-flash"));
        let (provider, model) = session_choice(Some(&choice), Some("river".to_owned()), None);
        assert_eq!(provider.as_deref(), Some("river"));
        assert_eq!(
            model.as_deref(),
            Some("nvidia/nemotron-3.5-super-vl-120b-a12b")
        );

        // No choice and no flag: the configuration answers.
        let (provider, model) = session_choice(None, None, None);
        assert!(provider.is_none() && model.is_none());
    }

    #[test]
    fn the_flags_beat_the_configuration_and_the_configuration_beats_nothing() {
        let config = Config {
            default_provider: Some("p".to_owned()),
            default_model: Some("m".to_owned()),
            ..Config::default()
        };
        let spec = resolve_spec(&config, &catalog(), None, None, None, None).expect("spec");
        assert_eq!(spec.label(), "p/m");

        // A flag wins over the configured default, and an explicit endpoint
        // replaces the catalogue's for one run.
        let spec = resolve_spec(
            &config,
            &catalog(),
            Some("p".to_owned()),
            Some("m".to_owned()),
            Some("https://proxy.invalid/v1".to_owned()),
            None,
        )
        .expect("spec");
        assert_eq!(spec.endpoint, "https://proxy.invalid/v1");
    }

    #[test]
    fn a_model_id_keeps_the_slashes_it_carries() {
        // The model id is an opaque string and the provider is its own key: the
        // two are never joined and taken apart again, so a model id that looks
        // like a reference survives the trip whole.
        let config = Config {
            default_provider: Some("nvidia".to_owned()),
            default_model: Some("nvidia/nemotron-3.5-super-vl-120b-a12b".to_owned()),
            ..Config::default()
        };
        let choice = resolve_reference(&config, None, None).expect("a choice");
        assert_eq!(choice.provider, "nvidia");
        assert_eq!(choice.model, "nvidia/nemotron-3.5-super-vl-120b-a12b");
        // The label is a label: it is what a person reads, not what is parsed.
        assert_eq!(
            format!("{}/{}", choice.provider, choice.model),
            "nvidia/nvidia/nemotron-3.5-super-vl-120b-a12b"
        );
    }

    #[test]
    fn a_provider_the_catalogue_does_not_have_is_refused_by_name() {
        let config = Config::default();
        let error = resolve_spec(
            &config,
            &catalog(),
            Some("nowhere".to_owned()),
            Some("m".to_owned()),
            None,
            None,
        )
        .expect_err("unknown provider");
        assert!(error.contains("nowhere"), "{error}");
        assert!(error.contains("providers.json"), "{error}");
    }

    #[test]
    fn a_model_the_catalogue_does_not_have_says_which_file_to_edit() {
        let config = Config::default();
        let error = resolve_spec(
            &config,
            &catalog(),
            Some("p".to_owned()),
            Some("nope".to_owned()),
            None,
            None,
        )
        .expect_err("unknown model");
        assert!(error.contains("models.json"), "{error}");
    }

    #[test]
    fn a_named_wire_picks_among_the_ones_the_provider_serves() {
        // The provider serves the OpenAI schema only, so naming another is an
        // error that lists what it has rather than a silent downgrade.
        let config = Config::default();
        let error = resolve_spec(
            &config,
            &catalog(),
            Some("p".to_owned()),
            Some("m".to_owned()),
            None,
            Some(WireSchema::Anthropic),
        )
        .expect_err("no anthropic wire");
        assert!(error.contains("anthropic"), "{error}");
        assert!(error.contains("openai"), "{error}");

        let spec = resolve_spec(
            &config,
            &catalog(),
            Some("p".to_owned()),
            Some("m".to_owned()),
            None,
            Some(WireSchema::OpenAI),
        )
        .expect("spec");
        assert_eq!(spec.schema, WireSchema::OpenAI);
    }

    #[test]
    fn no_reference_at_all_asks_for_the_two_keys() {
        let config = Config::default();
        let error =
            resolve_spec(&config, &catalog(), None, None, None, None).expect_err("nothing named");
        assert!(error.contains("--provider"), "{error}");
    }
}
