//! One group of endpoints the client exposes.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

impl ApiClient {
    pub async fn telemetry(&self, session_id: Option<&str>) -> Result<TelemetryReport, ApiError> {
        let mut url = self.resource_url("raep/v1/telemetry")?;
        if let Some(session_id) = session_id {
            url.query_pairs_mut().append_pair("session", session_id);
        }
        self.send_json(self.http.get(url)).await
    }

    pub async fn cost_report(&self, session_id: Option<&str>) -> Result<CostReport, ApiError> {
        let response = self
            .norte_code_command(
                session_id.unwrap_or("unused"),
                ClientRequestKind::CostReport {
                    session: session_id.map(SessionKey::new),
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
