//! Importing a session from a bundle.
//!
//! A bundle is the portable form of a session, built by the host's export
//! endpoint. Importing it is a two-step workflow: the bundle is uploaded and
//! validated, and the preview is committed to create the new session.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::client::status_error;
use super::{ApiError, Endpoint, RestClient};

/// The preview the host answers with before an import is committed.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ImportPreview {
    /// The id of the preview, used to commit it.
    pub import_id: String,
    /// The version of the bundle format.
    #[serde(default)]
    pub bundle_version: String,
    /// How many sessions the bundle carries.
    #[serde(default)]
    pub sessions: usize,
    /// When the preview stops being valid.
    #[serde(default)]
    pub expires_at: i64,
    /// The warnings the host attached to the preview.
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// The answer to a committed import.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ImportedSession {
    /// The new session's id.
    pub session_id: String,
    /// How the old ids map to the new ones.
    #[serde(default)]
    pub id_map: BTreeMap<String, String>,
    /// The warnings the host attached to the result.
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// The body of a commit request.
#[derive(Serialize)]
struct CommitImportRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_map: Option<BTreeMap<String, String>>,
    carry_usage: bool,
}

impl RestClient {
    /// Uploads a bundle and returns the preview.
    ///
    /// The bytes are the bundle as the host wrote it. The host validates the
    /// bundle and keeps the preview for an hour, or until it is committed or
    /// deleted.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused the bundle.
    pub async fn import_bundle(&self, bundle: Vec<u8>) -> Result<ImportPreview, ApiError> {
        let response = self
            .http
            .post(self.url(&Endpoint::Imports))
            .body(bundle)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .send()
            .await
            .map_err(|error| ApiError::Request(error.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(status_error(response).await);
        }
        response
            .json()
            .await
            .map_err(|error| ApiError::InvalidResponse(error.to_string()))
    }

    /// Commits a preview and returns the new session.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused the commit.
    pub async fn commit_import(
        &self,
        import_id: &str,
        agent_map: Option<BTreeMap<String, String>>,
        carry_usage: bool,
    ) -> Result<ImportedSession, ApiError> {
        let response = self
            .http
            .post(self.url(&Endpoint::ImportCommit { id: import_id }))
            .json(&CommitImportRequest {
                agent_map,
                carry_usage,
            })
            .send()
            .await
            .map_err(|error| ApiError::Request(error.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(status_error(response).await);
        }
        response
            .json()
            .await
            .map_err(|error| ApiError::InvalidResponse(error.to_string()))
    }
}
