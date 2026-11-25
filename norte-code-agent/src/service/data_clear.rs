use super::{Service, ServiceError};

impl Service {
    /// Clears durable session data and the service's in-memory telemetry.
    ///
    /// # Errors
    ///
    /// Returns an error if the session directory cannot be cleared.
    pub async fn clear_data(&self) -> Result<(), ServiceError> {
        self.directory
            .clear_data()
            .await
            .map_err(|source| ServiceError::DataCleanup { source })?;
        self.telemetry.clear();
        Ok(())
    }
}
