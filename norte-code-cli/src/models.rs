//! Model selection and the `models` report.

use std::borrow::Cow;

use norte_code_config::{AuthKind, WireKind};

use super::{
    Catalog, Config, ModelInfo, ModelSpec, ModelsArgs, ServiceError, Source, fallback_env,
    human_tokens, model_list, reasoning_label, source_label,
};

pub(crate) fn list_models(
    config: &Config,
    catalog: &Catalog,
    source: Source,
    args: &ModelsArgs,
) -> Result<u8, String> {
    let mut models = model_list(config, catalog);
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
        eprintln!(
            "catalogue: {} models from {}",
            models.len(),
            source_label(source)
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
        .map_or_else(|| "-".to_owned(), human_tokens);
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
/// `--model` over `default_model`, and each falls back to its own config key;
/// a model with no provider (from either) is an error rather than a default
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_spec(
    config: &Config,
    catalog: &Catalog,
    provider: Option<String>,
    model: Option<String>,
    endpoint: Option<String>,
    wire: Option<WireKind>,
    auth: Option<AuthKind>,
    api_key_env: Option<String>,
    disable_prompt_cache_key: bool,
) -> Result<ModelSpec, String> {
    let (provider, model_name) = resolve_reference(config, provider, model)?;

    let reference = format!("{provider}/{model_name}");

    // When provider is "disabled", it's a non-persistent custom provider:
    // require explicit wire, endpoint, and api_key_env.
    let is_disabled = provider == "disabled";
    if is_disabled {
        if wire.is_none() {
            return Err("--provider disabled requires --wire".to_owned());
        }
        if endpoint.is_none()
            && config
                .providers
                .get("disabled")
                .and_then(|e| e.endpoint.clone())
                .is_none()
        {
            return Err("--provider disabled requires --endpoint".to_owned());
        }
        if api_key_env.is_none()
            && config
                .providers
                .get("disabled")
                .and_then(|e| e.api_key_env.clone())
                .is_none()
        {
            return Err("--provider disabled requires --api-key-env".to_owned());
        }
    } else {
        // Validate that the provider exists in the catalogue or config.
        let in_catalogue = catalog.provider(&provider).is_some();
        let in_config = config.providers.contains_key(&provider);
        if !in_catalogue && !in_config {
            return Err(format!(
                "unknown provider `{provider}`; use a catalogue provider, a configured provider, or --provider disabled"
            ));
        }
    }

    let config =
        if wire.is_some() || auth.is_some() || api_key_env.is_some() || disable_prompt_cache_key {
            let mut config = config.clone();
            let entry = config.providers.entry(provider.clone()).or_default();
            if wire.is_some() {
                entry.wire = wire;
            }
            if auth.is_some() {
                entry.auth = auth;
            }
            if api_key_env.is_some() {
                entry.api_key_env = api_key_env;
            }
            if disable_prompt_cache_key {
                entry.prompt_cache_key = Some(false);
            }
            Cow::Owned(config)
        } else {
            Cow::Borrowed(config)
        };
    let config = config.as_ref();
    match ModelSpec::from_catalog(config, catalog, &reference, endpoint.clone()) {
        Ok(spec) => Ok(spec),
        // The provider is in the catalogue and the model is not: a fresh
        // release, or a model named in the config and nowhere else. The
        // provider still supplies the wire, the endpoint and the key, so the
        // reference resolves with conservative capabilities and whatever the
        // host declared under `providers.<id>.models` rather than failing.
        //
        // This is the same fallback the service makes in `default_spec_for`:
        // `/provider liquid` inside a session resolved a model the command
        // line refused, which is one reference answered two ways.
        Err(ServiceError::UnknownModel {
            provider: unknown, ..
        }) if catalog.provider(&unknown).is_some() => {
            ModelSpec::from_provider(config, catalog, &reference, endpoint.clone())
                .map_err(|error| error.to_string())
        }
        Err(ServiceError::UnknownProvider(_)) => {
            if !is_disabled {
                eprintln!(
                    "note: `{}` names a provider the catalogue does not list; using the fallback",
                    crate::text::terminal(&reference)
                );
            }
            let fallback_provider = if is_disabled { "disabled" } else { &provider };
            ModelSpec::resolve(
                config,
                &reference,
                endpoint,
                &fallback_env(fallback_provider),
            )
            .map_err(|error| error.to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use norte_code_catalog::Wire;
    use norte_code_config::{AuthKind, WireKind};

    use super::*;

    #[test]
    fn the_command_line_overrides_the_provider_wire_and_auth() {
        let catalog = Catalog::parse(
            br#"{
                "providers": {
                    "openrouter": {
                        "id": "openrouter",
                        "env": ["OPENROUTER_API_KEY"],
                        "npm": "@openrouter/ai-sdk-provider",
                        "api": "https://openrouter.ai/api/v1",
                        "models": {
                            "anthropic/claude-sonnet-4.5": {
                                "id": "anthropic/claude-sonnet-4.5",
                                "name": "Claude Sonnet 4.5",
                                "tool_call": true,
                                "modalities": {"input": ["text"], "output": ["text"]},
                                "limit": {"context": 200000, "output": 64000}
                            }
                        }
                    }
                }
            }"#,
        )
        .expect("catalog");
        let key_file =
            std::env::temp_dir().join(format!("norte-code-resolve-{}-key", std::process::id()));
        fs::write(&key_file, "sk-or-test\n").expect("key file");
        let config = Config::from_json(&format!(
            r#"{{"providers":{{"openrouter":{{"api_key_file":"{}"}}}}}}"#,
            key_file.display()
        ))
        .expect("config")
        .config;

        let plain = resolve_spec(
            &config,
            &catalog,
            Some("openrouter".to_owned()),
            Some("anthropic/claude-sonnet-4.5".to_owned()),
            None,
            None,
            None,
            None,
            false,
        )
        .expect("spec");
        assert_eq!(plain.wire, Wire::Chat);
        assert_eq!(plain.auth, None);

        let overridden = resolve_spec(
            &config,
            &catalog,
            Some("openrouter".to_owned()),
            Some("anthropic/claude-sonnet-4.5".to_owned()),
            None,
            Some(WireKind::Messages),
            Some(AuthKind::Bearer),
            None,
            false,
        )
        .expect("spec");
        assert_eq!(overridden.wire, Wire::Messages);
        assert_eq!(overridden.auth, Some(AuthKind::Bearer));
        let _ = fs::remove_file(key_file);
    }
}
