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

#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_spec(
    config: &Config,
    catalog: &Catalog,
    model: Option<String>,
    endpoint: Option<String>,
    wire: Option<WireKind>,
    auth: Option<AuthKind>,
    api_key_env: Option<String>,
    disable_prompt_cache_key: bool,
) -> Result<ModelSpec, String> {
    let reference = model
        .or_else(|| config.default_model.clone())
        .ok_or_else(|| {
            "no model selected: pass `--model provider/model` or set `default_model` in the config"
                .to_owned()
        })?;
    let config =
        if wire.is_some() || auth.is_some() || api_key_env.is_some() || disable_prompt_cache_key {
            let mut config = config.clone();
            if let Some((provider, _)) = reference.split_once('/') {
                let entry = config.providers.entry(provider.to_owned()).or_default();
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
            }
            Cow::Owned(config)
        } else {
            Cow::Borrowed(config)
        };
    let config = config.as_ref();
    match ModelSpec::from_catalog(config, catalog, &reference, endpoint.clone()) {
        Ok(spec) => Ok(spec),
        Err(ServiceError::UnknownModel { .. }) => {
            eprintln!(
                "note: `{}` is not in the catalogue; using its provider's facts",
                crate::text::terminal(&reference)
            );
            ModelSpec::from_provider(config, catalog, &reference, endpoint)
                .map_err(|error| error.to_string())
        }
        Err(ServiceError::UnknownProvider(_)) => {
            eprintln!(
                "note: `{}` names a provider the catalogue does not list; using the fallback",
                crate::text::terminal(&reference)
            );
            let provider = reference
                .split_once('/')
                .map_or("", |(provider, _)| provider);
            ModelSpec::resolve(config, &reference, endpoint, &fallback_env(provider))
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
            Some("openrouter/anthropic/claude-sonnet-4.5".to_owned()),
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
            Some("openrouter/anthropic/claude-sonnet-4.5".to_owned()),
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
