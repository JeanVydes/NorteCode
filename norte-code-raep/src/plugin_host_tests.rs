use super::*;

fn service() -> Arc<Service> {
    let spec = norte_code_agent::ModelSpec {
        provider: "test".to_owned(),
        model: "scripted".to_owned(),
        endpoint: "https://example.invalid/v1".to_owned(),
        api_key: norte_code_protocol::config::ApiKey::new("test"),
        schema: norte_code_wire::catalog::WireSchema::OpenAI,
        auth: None,
        capabilities: norte::Capabilities::builder().max_context(4096).build(),
        headers: std::collections::BTreeMap::new(),
        http: norte_code_protocol::config::ProviderHttpConfig::default(),
    };
    Arc::new(
        Service::new(
            norte_code_protocol::config::Config::default(),
            Arc::new(norte_code_protocol::catalog::Catalog::default()),
            Some(spec),
            Arc::new(norte_code_agent::VendorModels),
            Arc::new(norte_code_protocol::InMemoryStore::new()),
            Arc::new(norte::InMemoryBlobStore::new()),
            "system",
        )
        .expect("service"),
    )
}

#[tokio::test]
async fn the_adapter_answers_from_the_default_spec_before_a_session_is_attached() {
    let adapter = PluginHostAdapter::new();
    adapter.install_services(
        HashMap::from([("main".to_owned(), service())]),
        "main".to_owned(),
    );

    assert_eq!(adapter.session(), None);
    assert_eq!(adapter.session_model(), Some("scripted".to_owned()));
    assert_eq!(adapter.session_provider(), Some("test".to_owned()));
    assert_eq!(adapter.session_title(), None);
    assert_eq!(
        adapter.session_context(),
        ContextUsage {
            used: 0,
            window: Some(4096),
        }
    );
}

#[test]
fn the_adapter_reports_the_last_attached_session() {
    let adapter = PluginHostAdapter::new();
    adapter.install_services(HashMap::new(), "main".to_owned());

    assert_eq!(adapter.session(), None);

    adapter.set_session("session-1", "main");

    assert_eq!(adapter.session(), Some(SessionKey::new("session-1")));
    assert_eq!(adapter.session_model(), None);
}
