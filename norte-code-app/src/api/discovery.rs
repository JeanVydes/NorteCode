//! One group of endpoints the client exposes.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

impl ApiClient {
    pub async fn info(&self) -> Result<ServerInfo, ApiError> {
        self.get_json("raep/v1/info").await
    }

    pub async fn health_live(&self) -> Result<Liveness, ApiError> {
        self.get_json("raep/v1/health/live").await
    }

    pub async fn health_ready(&self) -> Result<Readiness, ApiError> {
        self.get_json("raep/v1/health/ready").await
    }

    pub async fn quota(&self) -> Result<QuotaReport, ApiError> {
        self.get_json("raep/v1/quota").await
    }

    pub async fn agents(&self) -> Result<Vec<AgentSummary>, ApiError> {
        let response: AgentListResponse = self.get_json("raep/v1/agents").await?;
        Ok(response.agents)
    }
}
