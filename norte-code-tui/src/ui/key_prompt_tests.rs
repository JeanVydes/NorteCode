//! Cache keys for provider secret lookups.

use std::path::Path;

use super::key_prompt::{cache_key, provider_has_key};

#[test]
fn cache_keys_separate_providers_and_secret_files() {
    let first = cache_key(Path::new("/tmp/one/secrets.json"), "anthropic");
    let other_provider = cache_key(Path::new("/tmp/one/secrets.json"), "openai");
    let other_file = cache_key(Path::new("/tmp/two/secrets.json"), "anthropic");
    assert_ne!(first, other_provider);
    assert_ne!(first, other_file);
}

#[test]
fn a_provider_without_environment_keys_needs_no_lookup() {
    assert!(provider_has_key("anything", &[]));
}
