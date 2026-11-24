use norte_code_wire::ClientRequestKind;

use super::{ApiError, RestClient};

impl RestClient {
    /// Stores a provider key in the host's key store.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused the write.
    pub async fn save_secret(&self, provider: &str, key: &str) -> Result<(), ApiError> {
        self.command_ok(
            None,
            ClientRequestKind::SaveSecret {
                provider: provider.to_owned(),
                key: key.to_owned(),
            },
        )
        .await
    }

    /// Removes a provider key from the host's key store.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused the removal.
    pub async fn delete_secret(&self, provider: &str) -> Result<(), ApiError> {
        self.command_ok(
            None,
            ClientRequestKind::DeleteSecret {
                provider: provider.to_owned(),
            },
        )
        .await
    }
}
