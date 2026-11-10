use crate::api::{ApiClient, CatalogDetail};
use crate::components::{EmptyState, PageHeader};
use crate::state::AppContext;
use dioxus::prelude::*;
use norte_code_wire::catalog::{Model, Provider, Wire, WireSchema};
use norte_code_wire::{CatalogModelInfo, CatalogProviderInfo};

/// A provider as the form writes it into `providers.json`.
/// The catalogue lives in two files on the host; this page edits them.
///
/// A provider a plugin injected is shown, marked and not editable: it is not
/// in `providers.json`, and writing it there would either duplicate it or be
/// overwritten on the next start.
#[component]
pub fn ProvidersPage() -> Element {
    let context = use_context::<AppContext>();
    let detail = use_signal(CatalogDetail::default);
    let loading = use_signal(|| false);
    let mut adding_provider = use_signal(|| false);
    let mut new_provider_id = use_signal(String::new);
    let mut new_provider_name = use_signal(String::new);
    let mut new_provider_api = use_signal(String::new);
    let mut new_provider_env = use_signal(String::new);
    let mut model_form = use_signal(|| None::<String>);
    let mut new_model_id = use_signal(String::new);
    let mut new_model_name = use_signal(String::new);

    use_effect(move || reload(&context, detail, loading));

    let current = detail.read().clone();
    let from_file_models = current
        .models
        .iter()
        .filter(|model| model.from_file)
        .count();

    rsx! {
        PageHeader {
            eyebrow: "Configuration",
            title: "Providers",
            description: "The providers and models the host's catalogue files declare. Changes are written to providers.json and models.json on the host."
        }
        section { class: "panel",
            div { class: "section-heading",
                h2 { "Providers" }
                div { class: "row-actions",
                    button {
                        class: "button-secondary",
                        onclick: move |_| {
                            let next = !adding_provider();
                            adding_provider.set(next);
                        },
                        if adding_provider() { "Cancel" } else { "Add provider" }
                    }
                    button {
                        class: "button-secondary",
                        disabled: *loading.read(),
                        onclick: move |_| reload(&context, detail, loading),
                        if *loading.read() { "Reloading…" } else { "Reload" }
                    }
                }
            }
            if adding_provider() {
                div { class: "inline-form",
                    input {
                        class: "text-input",
                        placeholder: "provider id (e.g. openrouter)",
                        value: "{new_provider_id}",
                        oninput: move |event| new_provider_id.set(event.value()),
                    }
                    input {
                        class: "text-input",
                        placeholder: "display name",
                        value: "{new_provider_name}",
                        oninput: move |event| new_provider_name.set(event.value()),
                    }
                    input {
                        class: "text-input",
                        placeholder: "base URL (https://api.example.com/v1)",
                        value: "{new_provider_api}",
                        oninput: move |event| new_provider_api.set(event.value()),
                    }
                    input {
                        class: "text-input",
                        placeholder: "environment keys, comma separated",
                        value: "{new_provider_env}",
                        oninput: move |event| new_provider_env.set(event.value()),
                    }
                    button {
                        class: "button-primary",
                        onclick: move |_| {
                            let id = new_provider_id.read().trim().to_owned();
                            let name = new_provider_name.read().trim().to_owned();
                            let api = new_provider_api.read().trim().to_owned();
                            let environment_keys = new_provider_env
                                .read()
                                .split(',')
                                .map(str::trim)
                                .filter(|key| !key.is_empty())
                                .map(str::to_owned)
                                .collect::<Vec<_>>();
                            if id.is_empty() || api.is_empty() {
                                context.set_page_error(Some(
                                    "A provider needs an id and a base URL.".to_owned(),
                                ));
                                return;
                            }
                            let entry = Provider {
                                name: if name.is_empty() { id.clone() } else { name },
                                environment_keys,
                                wires: vec![Wire {
                                    api,
                                    schema: WireSchema::OpenAI,
                                    auth: None,
                                }],
                                ..Provider::default()
                            };
                            save_provider(&context, detail, loading, id, entry);
                            adding_provider.set(false);
                            new_provider_id.set(String::new());
                            new_provider_name.set(String::new());
                            new_provider_api.set(String::new());
                            new_provider_env.set(String::new());
                        },
                        "Save"
                    }
                }
            }
            if current.providers.is_empty() {
                EmptyState {
                    title: "No providers",
                    message: "Add one, or install a plugin that injects one."
                }
            }
            div { class: "catalog-provider-grid",
                for provider in current.providers.clone() {
                    ProviderCard {
                        key: "{provider.provider_id}",
                        provider: provider.clone(),
                        models: current
                            .models
                            .iter()
                            .filter(|model| model.provider == provider.provider_id)
                            .cloned()
                            .collect(),
                        detail,
                        loading,
                    }
                }
            }
        }
        section { class: "panel",
            div { class: "section-heading",
                h2 { "Models" }
                span { class: "status status-muted", "{from_file_models} editable" }
            }
            if let Some(provider_id) = model_form.read().clone() {
                div { class: "inline-form",
                    input {
                        class: "text-input",
                        placeholder: "model id (e.g. gpt-4o)",
                        value: "{new_model_id}",
                        oninput: move |event| new_model_id.set(event.value()),
                    }
                    input {
                        class: "text-input",
                        placeholder: "display name",
                        value: "{new_model_name}",
                        oninput: move |event| new_model_name.set(event.value()),
                    }
                    button {
                        class: "button-primary",
                        onclick: move |_| {
                            let id = new_model_id.read().trim().to_owned();
                            let name = new_model_name.read().trim().to_owned();
                            if id.is_empty() {
                                context.set_page_error(Some("A model needs an id.".to_owned()));
                                return;
                            }
                            let entry = Model {
                                provider_id: provider_id.clone(),
                                canonical_slug: id.clone(),
                                name: if name.is_empty() { id.clone() } else { name },
                                id,
                                ..Model::default()
                            };
                            save_model(&context, detail, loading, entry);
                            model_form.set(None);
                            new_model_id.set(String::new());
                            new_model_name.set(String::new());
                        },
                        "Save"
                    }
                    button {
                        class: "link-button",
                        onclick: move |_| model_form.set(None),
                        "Cancel"
                    }
                }
            }
            div { class: "model-table",
                for model in current.models.iter().filter(|model| model.from_file) {
                    div { class: "model-row", key: "{model.provider}/{model.model}",
                        div {
                            strong { "{model.name}" }
                            p { class: "mono", "{model.provider} / {model.model}" }
                        }
                        div { class: "row-actions",
                            button {
                                class: "link-button",
                                title: "Remove from models.json",
                                onclick: {
                                    let provider = model.provider.clone();
                                    let model_id = model.model.clone();
                                    move |_| remove_model(&context, detail, loading, provider.clone(), model_id.clone())
                                },
                                "Remove"
                            }
                        }
                    }
                }
                if from_file_models == 0 {
                    p { class: "helper-text", "No model is editable in models.json." }
                }
            }
            if !current.providers.is_empty() {
                div { class: "row-actions",
                    for provider in current.providers.iter().filter(|provider| provider.from_file) {
                        button {
                            class: "button-secondary",
                            key: "add-{provider.provider_id}",
                            onclick: {
                                let provider_id = provider.provider_id.clone();
                                move |_| model_form.set(Some(provider_id.clone()))
                            },
                            "+ {provider.provider_id}"
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ProviderCard(
    provider: CatalogProviderInfo,
    models: Vec<CatalogModelInfo>,
    detail: Signal<CatalogDetail>,
    loading: Signal<bool>,
) -> Element {
    let context = use_context::<AppContext>();
    let mut adding_model = use_signal(|| false);
    let mut model_id = use_signal(String::new);
    let mut model_name = use_signal(String::new);
    rsx! {
        article { class: "panel catalog-provider",
            div { class: "section-heading",
                h3 { "{provider.name}" }
                if provider.from_file {
                    div { class: "row-actions",
                        button {
                            class: "link-button",
                            title: "Add a model served by this provider",
                            onclick: move |_| adding_model.toggle(),
                            "Add model"
                        }
                        button {
                            class: "link-button",
                            title: "Remove this provider and its models from the files",
                            onclick: {
                                let provider_id = provider.provider_id.clone();
                                move |_| {
                                    let provider_id = provider_id.clone();
                                    spawn(async move {
                                        let Some(client) = client(&context) else { return };
                                        match client.remove_provider(&provider_id).await {
                                            Ok(()) => reload(&context, detail, loading),
                                            Err(error) => context.set_page_error(Some(format!(
                                                "Could not remove `{provider_id}`: {error}"
                                            ))),
                                        }
                                    });
                                }
                            },
                            "Remove"
                        }
                    }
                } else {
                    span { class: "status status-muted", title: "Injected by a plugin; edit the plugin instead",
                        "plugin"
                    }
                }
            }
            if let Some(api) = &provider.api {
                p { class: "mono helper-text", "{api}" }
            }
            if adding_model() {
                div { class: "inline-form",
                    input {
                        class: "text-input",
                        placeholder: "model id",
                        value: "{model_id}",
                        oninput: move |event| model_id.set(event.value()),
                    }
                    input {
                        class: "text-input",
                        placeholder: "display name",
                        value: "{model_name}",
                        oninput: move |event| model_name.set(event.value()),
                    }
                    button {
                        class: "button-primary",
                        onclick: {
                            let provider_id = provider.provider_id.clone();
                            move |_| {
                                let id = model_id.read().trim().to_owned();
                                if id.is_empty() {
                                    context.set_page_error(Some("A model needs an id.".to_owned()));
                                    return;
                                }
                                let name = model_name.read().trim().to_owned();
                                let entry = Model {
                                    provider_id: provider_id.clone(),
                                    canonical_slug: id.clone(),
                                    name: if name.is_empty() { id.clone() } else { name },
                                    id,
                                    ..Model::default()
                                };
                                save_model(&context, detail, loading, entry);
                                adding_model.set(false);
                                model_id.set(String::new());
                                model_name.set(String::new());
                            }
                        },
                        "Save"
                    }
                }
            }
            details { class: "catalog-models",
                summary {
                    span { "Models" }
                    span { class: "plan-count", "{models.len()}" }
                }
                for model in models {
                    div { class: "catalog-model", key: "{model.model}",
                        div {
                            strong { "{model.name}" }
                            p { class: "mono", "{model.model}" }
                        }
                        if !model.from_file {
                            span { class: "status status-muted", "plugin" }
                        }
                    }
                }
            }
        }
    }
}

fn reload(context: &AppContext, mut detail: Signal<CatalogDetail>, mut loading: Signal<bool>) {
    let context = *context;
    loading.set(true);
    spawn(async move {
        let Some(client) = client(&context) else {
            loading.set(false);
            return;
        };
        match client.catalog_detail().await {
            Ok(next) => {
                detail.set(next);
                context.set_page_error(None);
            }
            Err(error) => context.set_page_error(Some(error.to_string())),
        }
        loading.set(false);
    });
}

fn save_provider(
    context: &AppContext,
    detail: Signal<CatalogDetail>,
    loading: Signal<bool>,
    provider_id: String,
    entry: Provider,
) {
    let context = *context;
    spawn(async move {
        let Some(client) = client(&context) else {
            return;
        };
        match client.upsert_provider(&provider_id, entry).await {
            Ok(()) => reload(&context, detail, loading),
            Err(error) => {
                context.set_page_error(Some(format!("Could not save `{provider_id}`: {error}")));
            }
        }
    });
}

fn save_model(
    context: &AppContext,
    detail: Signal<CatalogDetail>,
    loading: Signal<bool>,
    entry: Model,
) {
    let context = *context;
    spawn(async move {
        let Some(client) = client(&context) else {
            return;
        };
        match client.upsert_model(entry).await {
            Ok(()) => reload(&context, detail, loading),
            Err(error) => {
                context.set_page_error(Some(format!("Could not save the model: {error}")));
            }
        }
    });
}

fn remove_model(
    context: &AppContext,
    detail: Signal<CatalogDetail>,
    loading: Signal<bool>,
    provider_id: String,
    model_id: String,
) {
    let context = *context;
    spawn(async move {
        let Some(client) = client(&context) else {
            return;
        };
        match client.remove_model(&provider_id, &model_id).await {
            Ok(()) => reload(&context, detail, loading),
            Err(error) => context.set_page_error(Some(format!(
                "Could not remove `{provider_id}/{model_id}`: {error}"
            ))),
        }
    });
}

fn client(context: &AppContext) -> Option<ApiClient> {
    let profile = context.active_profile()?;
    ApiClient::new(&profile.base_url, &profile.bearer_token).ok()
}
