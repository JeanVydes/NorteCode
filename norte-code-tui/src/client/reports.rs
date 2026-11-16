use std::sync::Arc;

use norte_code_wire::SessionKey;

use super::{ClientData, ClientRuntime};

impl ClientRuntime {
    pub(crate) fn load_telemetry(&self, session: Option<SessionKey>, generation: u64) {
        let api = Arc::clone(&self.api);
        let session_id = session.as_ref().map(|key| key.as_str().to_owned());
        self.spawn_data(session, generation, async move {
            api.telemetry(session_id.as_deref())
                .await
                .map(ClientData::Telemetry)
        });
    }

    pub(crate) fn load_costs(&self, session: Option<SessionKey>, generation: u64) {
        let api = Arc::clone(&self.api);
        let session_id = session.as_ref().map(|key| key.as_str().to_owned());
        let report_session = session.clone();
        self.spawn_data(session, generation, async move {
            let report = if let Some(session_id) = session_id.as_deref() {
                api.cost_report(Some(session_id)).await?
            } else {
                api.cost_report(None).await?
            };
            Ok(ClientData::Costs {
                session: report_session,
                report,
            })
        });
    }
}
