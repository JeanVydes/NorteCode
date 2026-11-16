use super::endpoint::TelemetryQuery;
use super::types::{AgentListResponse, Liveness, Readiness, ServerInfo};
use super::{ApiError, Endpoint, RestClient};

impl RestClient {
    /// What the server says about itself.
    ///
    /// # Errors
    ///
    /// Returns the transport error or an answer the shape does not describe.
    pub async fn info(&self) -> Result<ServerInfo, ApiError> {
        self.execute(Endpoint::Info).await
    }

    /// The liveness probe.
    ///
    /// # Errors
    ///
    /// Returns the transport error or an answer the shape does not describe.
    pub async fn health_live(&self) -> Result<Liveness, ApiError> {
        self.execute(Endpoint::HealthLive).await
    }

    /// The readiness probe.
    ///
    /// # Errors
    ///
    /// Returns the transport error or an answer the shape does not describe.
    pub async fn health_ready(&self) -> Result<Readiness, ApiError> {
        self.execute(Endpoint::HealthReady).await
    }

    /// The principal's quota.
    ///
    /// # Errors
    ///
    /// Returns the transport error or an answer the shape does not describe.
    pub async fn quota(&self) -> Result<norte_code_wire::QuotaReport, ApiError> {
        self.execute(Endpoint::Quota).await
    }

    /// The agents the host exposes.
    ///
    /// # Errors
    ///
    /// Returns the transport error or an answer the shape does not describe.
    pub async fn agents(&self) -> Result<Vec<norte_code_wire::AgentSummary>, ApiError> {
        Ok(self
            .execute::<AgentListResponse>(Endpoint::Agents)
            .await?
            .agents)
    }

    /// The request telemetry, optionally for one session.
    ///
    /// # Errors
    ///
    /// Returns the transport error or an answer the shape does not describe.
    pub async fn telemetry(
        &self,
        session: Option<&str>,
    ) -> Result<norte_code_wire::TelemetryReport, ApiError> {
        self.execute(Endpoint::Telemetry(TelemetryQuery { session }))
            .await
    }
}
