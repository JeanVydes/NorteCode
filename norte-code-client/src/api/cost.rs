use norte_code_wire::{CanonicalEvent, ClientRequestKind, SessionKey};

use super::shared::decode_payload;
use super::types::{CostNotReportedResponse, CostReport, CostUpdatedResponse};
use super::{ApiError, RestClient};

impl RestClient {
    /// The provider-reported cost totals.
    ///
    /// # Errors
    ///
    /// Returns the transport error or an answer the shape does not describe.
    pub async fn cost_report(&self, session: Option<&str>) -> Result<CostReport, ApiError> {
        let response = self
            .command(
                session,
                ClientRequestKind::CostReport {
                    session: session.map(SessionKey::new),
                },
            )
            .await?;
        if let Some(payload) = response
            .events
            .iter()
            .find(|frame| CanonicalEvent::CostUpdated.matches(&frame.payload))
        {
            let report: CostUpdatedResponse = decode_payload(&payload.payload)?;
            return Ok(CostReport {
                total_usd: Some(report.total_usd),
                requests: report.requests,
            });
        }
        if let Some(payload) = response
            .events
            .iter()
            .find(|frame| CanonicalEvent::CostNotReported.matches(&frame.payload))
        {
            let report: CostNotReportedResponse = decode_payload(&payload.payload)?;
            return Ok(CostReport {
                total_usd: None,
                requests: report.requests,
            });
        }
        Err(ApiError::InvalidResponse(
            "Cost report was not returned.".to_owned(),
        ))
    }
}
