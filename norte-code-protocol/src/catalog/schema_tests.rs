use super::*;
use norte_code_wire::catalog::{
    Limit, Modalities, Modality, ModelStatus, SupportedEffort, SupportedParams, Wire, WireSchema,
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
