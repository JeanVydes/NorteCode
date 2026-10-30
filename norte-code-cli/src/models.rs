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
) -> Result<(String, String), String> {
    let provider = provider.or_else(|| config.default_provider.clone());
    let provider = provider.ok_or_else(|| {
        "no provider specified: pass --provider or set default_provider in config".to_owned()
    })?;
    let model = model.or_else(|| config.default_model.clone());
    let model = model.ok_or_else(|| {
        "no model selected: pass --model or set default_model in config".to_owned()
    })?;
    Ok((provider, model))
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
    let (provider, model_name) = resolve_reference(config, provider, model)?;
    if catalog.provider(&provider).is_none() {
        return Err(format!(
            "unknown provider `{provider}`; add it to providers.json, or pick one of: {}",
            catalog
                .providers()
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if catalog.find(&provider, &model_name).is_none() {
        return Err(format!(
            "the provider `{provider}` has no model `{model_name}`; add it to models.json"
        ));
    }
    let mut spec =
        ModelSpec::from_catalog(config, catalog, &format!("{provider}/{model_name}"), wire)
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
