use std::sync::Arc;

use super::{ClientData, ClientRuntime};
use crate::components::ToolRuleAction;

impl ClientRuntime {
    pub(crate) fn load_host(&self, generation: u64) {
        let api = Arc::clone(&self.api);
        self.spawn_data(None, generation, async move {
            Ok(host_result(
                "server info",
                api.info().await,
                ClientData::ServerInfo,
            ))
        });
        let api = Arc::clone(&self.api);
        self.spawn_data(None, generation, async move {
            Ok(host_result(
                "liveness",
                api.health_live().await,
                ClientData::Liveness,
            ))
        });
        let api = Arc::clone(&self.api);
        self.spawn_data(None, generation, async move {
            Ok(host_result(
                "readiness",
                api.health_ready().await,
                ClientData::Readiness,
            ))
        });
        let api = Arc::clone(&self.api);
        self.spawn_data(None, generation, async move {
            Ok(host_result("quota", api.quota().await, ClientData::Quota))
        });
        let api = Arc::clone(&self.api);
        self.spawn_data(None, generation, async move {
            Ok(host_result(
                "agents",
                api.agents().await,
                ClientData::Agents,
            ))
        });
        self.load_skills(generation, false);
        self.load_mcp(generation, false);
        self.load_tool_rules(generation);
    }

    pub(crate) fn load_skills(&self, generation: u64, refresh: bool) {
        let api = Arc::clone(&self.api);
        self.spawn_data(None, generation, async move {
            let result = if refresh {
                api.refresh_skills().await
            } else {
                api.skills().await
            };
            Ok(host_result("skills", result, ClientData::Skills))
        });
    }

    pub(crate) fn load_mcp(&self, generation: u64, refresh: bool) {
        let api = Arc::clone(&self.api);
        self.spawn_data(None, generation, async move {
            let result = if refresh {
                api.refresh_mcp().await
            } else {
                api.mcp_catalog().await
            };
            Ok(host_result("MCP", result, ClientData::Mcp))
        });
    }

    pub(crate) fn set_mcp_active(&self, name: String, active: bool, generation: u64) {
        let api = Arc::clone(&self.api);
        self.spawn_data(None, generation, async move {
            api.set_mcp_active(&name, active).await?;
            Ok(host_result("MCP", api.mcp_catalog().await, ClientData::Mcp))
        });
    }

    pub(crate) fn load_tool_rules(&self, generation: u64) {
        let api = Arc::clone(&self.api);
        self.spawn_data(None, generation, async move {
            Ok(host_result(
                "tool rules",
                api.tool_rules().await,
                ClientData::ToolRules,
            ))
        });
    }

    pub(crate) fn set_tool_rule(&self, name: String, action: ToolRuleAction, generation: u64) {
        let api = Arc::clone(&self.api);
        self.spawn_data(None, generation, async move {
            match action {
                ToolRuleAction::Allow => api.set_tool_allowed(&name, true).await?,
                ToolRuleAction::Deny => api.set_tool_allowed(&name, false).await?,
                ToolRuleAction::Forget => api.forget_tool_rule(&name).await?,
            }
            Ok(host_result(
                "tool rules",
                api.tool_rules().await,
                ClientData::ToolRules,
            ))
        });
    }
}

fn host_result<T>(
    probe: &'static str,
    result: Result<T, norte_code_client::ApiError>,
    map: impl FnOnce(T) -> ClientData,
) -> ClientData {
    match result {
        Ok(data) => map(data),
        Err(error) => ClientData::HostFailure {
            probe,
            message: error.to_string(),
        },
    }
}
