//! Model selection and the `models` report.

use futures_util::StreamExt;
use norte_code_client::AgentClient;

use norte_code_protocol::config::Config;

use super::{ModelInfo, ModelsArgs, reasoning_label};

/// Writes the model list, filtered the way the flags asked.
pub(crate) async fn list_models(config: &Config, args: &ModelsArgs) -> Result<u8, String> {
    let client = super::remote_client(config)?;
    let mut models = remote_model_list(&client).await?;
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
        eprintln!("catalogue: {} models from the remote server", models.len());
        for model in &models {
            print_model(model);
        }
    }
    Ok(0)
}

/// Fetches the server-owned catalogue.
pub(crate) async fn remote_model_list(client: &dyn AgentClient) -> Result<Vec<ModelInfo>, String> {
    let mut stream = client
        .submit(norte_code_wire::ClientRequest {
            request_id: 0,
            kind: norte_code_wire::ClientRequestKind::ListModels,
        })
        .await
        .map_err(|error| error.to_string())?;
    let mut models = Vec::new();
    while let Some(event) = stream.next().await {
        match event.kind {
            norte_code_wire::ServerEventKind::ModelList { models: listed } => {
                models.extend(listed);
            }
            norte_code_wire::ServerEventKind::Error { message, .. } => return Err(message),
            _ => {}
        }
    }
    Ok(models)
}

/// Resolves an explicitly requested model against the server-owned catalogue.
pub(crate) fn resolve_remote_spec(
    models: &[ModelInfo],
    provider: &str,
    model: &str,
) -> Result<ModelInfo, String> {
    if !models.iter().any(|entry| entry.provider == provider) {
        return Err(format!(
            "unknown provider `{provider}`; ask the server to install it"
        ));
    }
    models
        .iter()
        .find(|entry| entry.provider == provider && entry.model == model)
        .cloned()
        .ok_or_else(|| {
            format!("the provider `{provider}` has no model `{model}` in the server catalogue")
        })
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

#[cfg(test)]
#[path = "models_tests.rs"]
mod tests;
