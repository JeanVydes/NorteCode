use super::*;

/// A provider that serves one wire.
fn provider(name: &str, schemas: &[WireSchema]) -> CatalogProvider {
    CatalogProvider {
        name: name.to_owned(),
        doc: None,
        environment_keys: Vec::new(),
        wires: schemas
            .iter()
            .map(|schema| CatalogWire {
                api: format!("https://{name}.invalid/v1"),
                schema: *schema,
                auth: None,
            })
            .collect(),
    }
}

fn model(id: &str, provider_id: &str) -> CatalogModel {
    CatalogModel {
        id: id.to_owned(),
        provider_id: provider_id.to_owned(),
        canonical_slug: format!("{provider_id}/{id}"),
        name: id.to_owned(),
        modalities: norte_code_wire::catalog::Modalities {
            input: vec![norte_code_wire::catalog::Modality::Text],
            output: vec![norte_code_wire::catalog::Modality::Text],
        },
        ..CatalogModel::default()
    }
}

#[test]
fn openrouter_is_attributed() {
    // The one gateway that asks to be identified, so its requests carry the
    // app's page and its name. Every other provider gets nothing.
    let headers = headers_for("openrouter");
    assert_eq!(
        headers.get("HTTP-Referer").map(String::as_str),
        Some("https://norte.vyde.dev")
    );
    assert_eq!(
        headers.get("X-OpenRouter-Title").map(String::as_str),
        Some("Norte Code")
    );
    assert_eq!(
        headers.get("X-OpenRouter-Categories").map(String::as_str),
        Some("cli-agent,cloud-agent")
    );

    assert!(headers_for("openai").is_empty());
}

#[test]
fn every_wire_builds_from_its_endpoint() {
    // One path for all three: the endpoint is the provider's own, and what
    // the adapter speaks is the schema it declared.
    let capabilities = Capabilities::default();
    for (name, schema) in [
        ("compatible", WireSchema::OpenAI),
        ("anthropic", WireSchema::Anthropic),
        ("google", WireSchema::Google),
    ] {
        let provider = provider(name, &[schema]);
        let model = model("m", name);
        let spec =
            ModelSpec::from_entry(&Config::default(), &provider, &model, None).expect("spec");
        assert_eq!(spec.schema, schema, "{name}");
        assert_eq!(spec.endpoint, format!("https://{name}.invalid/v1"));
        let built = VendorModels.build(&spec, &capabilities);
        assert!(built.is_ok(), "{name} did not build: {:?}", built.err());
    }
}

#[test]
fn a_named_schema_is_honoured_and_an_absent_one_is_refused() {
    // A provider may serve several surfaces; naming one picks it, and
    // naming one it does not serve is an error that says what it has.
    let provider = provider("both", &[WireSchema::Anthropic, WireSchema::Google]);
    let model = model("m", "both");

    let google = ModelSpec::from_entry(
        &Config::default(),
        &provider,
        &model,
        Some(WireSchema::Google),
    )
    .expect("google");
    assert_eq!(google.schema, WireSchema::Google);

    let error = ModelSpec::from_entry(
        &Config::default(),
        &provider,
        &model,
        Some(WireSchema::OpenAI),
    )
    .expect_err("no openai wire");
    let message = error.to_string();
    assert!(message.contains("openai"), "{message}");
    assert!(message.contains("anthropic"), "{message}");
}

#[test]
fn the_preferred_wire_is_used_when_nobody_named_one() {
    // OpenAI first, then Anthropic, then Google: what a person gets when
    // they did not say, and what the picker describes.
    let provider = provider(
        "many",
        &[
            WireSchema::Google,
            WireSchema::Anthropic,
            WireSchema::OpenAI,
        ],
    );
    let model = model("m", "many");
    let spec = ModelSpec::from_entry(&Config::default(), &provider, &model, None).expect("spec");
    assert_eq!(spec.schema, WireSchema::OpenAI);
}

#[test]
fn a_wire_may_name_its_own_credential_header() {
    // A gateway that serves another vendor's shape over its own credential:
    // an Anthropic Messages surface reached with `Authorization: Bearer`.
    // The schema says how the body is shaped and not who is paying, so the
    // wire says which header carries the key, and `None` follows the schema.
    let mut gateway = provider("gateway", &[WireSchema::Anthropic]);
    gateway.wires[0].auth = Some(AuthKind::Bearer);
    let model = model("m", "gateway");
    let spec = ModelSpec::from_entry(&Config::default(), &gateway, &model, None).expect("spec");
    assert_eq!(spec.auth, Some(AuthKind::Bearer));

    // The default still follows the schema when the wire says nothing.
    let plain = provider("anthropic", &[WireSchema::Anthropic]);
    let spec = ModelSpec::from_entry(&Config::default(), &plain, &model, None).expect("spec");
    assert_eq!(spec.auth, None, "the header follows the schema");
    assert_eq!(
        AuthKind::for_schema(WireSchema::Anthropic),
        AuthKind::XApiKey
    );
    assert_eq!(AuthKind::for_schema(WireSchema::OpenAI), AuthKind::Bearer);
    assert_eq!(
        AuthKind::for_schema(WireSchema::Google),
        AuthKind::XGoogApiKey
    );
    // And a wire may opt out for a local endpoint that wants no credential.
    let mut local = provider("local", &[WireSchema::OpenAI]);
    local.wires[0].auth = Some(AuthKind::Disabled);
    let spec = ModelSpec::from_entry(&Config::default(), &local, &model, None).expect("spec");
    assert_eq!(spec.auth, Some(AuthKind::Disabled));
    assert_eq!(AuthKind::Disabled.header(), None);
}

#[test]
fn a_provider_with_no_wire_cannot_be_reached() {
    let provider = provider("empty", &[]);
    let model = model("m", "empty");
    let error =
        ModelSpec::from_entry(&Config::default(), &provider, &model, None).expect_err("no wire");
    assert!(error.to_string().contains("empty"), "{error}");
}

#[test]
fn a_provider_that_names_no_variable_needs_no_key() {
    // A local endpoint is the case: there is nothing to look up, so nothing
    // is missing.
    let provider = provider("local", &[WireSchema::OpenAI]);
    let model = model("m", "local");
    let spec = ModelSpec::from_entry(&Config::default(), &provider, &model, None).expect("spec");
    assert_eq!(spec.api_key.expose(), "", "nothing to send");
}

#[test]
fn the_label_is_the_reference_a_person_writes() {
    let provider = provider("p", &[WireSchema::OpenAI]);
    let model = model("m", "p");
    let spec = ModelSpec::from_entry(&Config::default(), &provider, &model, None).expect("spec");
    assert_eq!(spec.label(), "p/m");
}
