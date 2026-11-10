//! One domain of the RAEP host, as `impl NorteCodeRaepHost` blocks.

use norte_code_agent::Service;
use std::sync::Arc;

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::capabilities::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::payloads::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::stores::*;
#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

use super::NorteCodeRaepHost;

impl NorteCodeRaepHost {
    /// Returns the server-owned agent service.
    #[must_use]
    pub fn service(&self) -> &Arc<Service> {
        &self.service
    }
    /// Returns the configured agent identifier.
    #[must_use]
    pub fn agent_id(&self) -> &str {
        &self.config.agent_id
    }
    /// Maximum configured upload size.
    #[must_use]
    pub fn max_artifact_bytes(&self) -> usize {
        self.config.max_artifact_bytes
    }
    pub(crate) fn output_part_bytes_limit(&self) -> usize {
        self.config
            .max_artifact_bytes
            .min(DEFAULT_MAX_OUTPUT_PART_BYTES)
    }
    /// Whether authenticated single-shot uploads are configured.
    #[must_use]
    pub fn uploads_enabled(&self) -> bool {
        self.artifact_store.is_some() && self.upload_index.is_some()
    }
    /// Maximum configured portable bundle size.
    #[must_use]
    pub fn max_export_bytes(&self) -> usize {
        self.config.max_export_bytes
    }
    /// Returns the configured bearer token, when transport authentication is enabled.
    #[must_use]
    pub fn auth_token(&self) -> Option<&str> {
        self.config.auth_token.as_deref()
    }
    /// Resolves a bearer token to its configured principal.
    ///
    /// When authentication is disabled, all requests use the anonymous
    /// principal. A single legacy token maps to `default` unless an explicit
    /// principal mapping exists.
    #[must_use]
    pub fn authenticate_bearer(&self, token: &str) -> Option<String> {
        let configured = self.config.auth_token.as_deref();
        if configured.is_none() && self.config.auth_principals.is_empty() {
            return None;
        }
        if configured != Some(token) && !self.config.auth_principals.contains_key(token) {
            return None;
        }
        Some(
            self.config
                .auth_principals
                .get(token)
                .cloned()
                .unwrap_or_else(|| "default".to_owned()),
        )
    }
    /// Returns the principal used for unauthenticated local operation.
    #[must_use]
    pub fn anonymous_principal(&self) -> &'static str {
        "anonymous"
    }
    /// Returns whether this host requires transport authentication.
    #[must_use]
    pub fn authentication_enabled(&self) -> bool {
        self.config.auth_token.is_some() || !self.config.auth_principals.is_empty()
    }
    /// Returns whether a browser origin may call the HTTP API.
    #[must_use]
    pub fn origin_allowed(&self, origin: &str) -> bool {
        self.config
            .allowed_origins
            .iter()
            .any(|allowed| allowed == origin)
    }
    /// Attempts to reserve one long-lived transport connection.
    pub fn try_connection(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        Arc::clone(&self.connections).try_acquire_owned().ok()
    }
    /// Returns the configured command timeout.
    #[must_use]
    pub fn command_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.config.command_timeout_ms)
    }
    /// Returns the maximum accepted RAEP frame size.
    #[must_use]
    pub fn max_frame_bytes(&self) -> usize {
        usize::try_from(self.config.max_frame_bytes).unwrap_or(usize::MAX)
    }
}
