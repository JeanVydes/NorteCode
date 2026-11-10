//! One group of endpoints the client exposes.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

impl ApiClient {
    pub async fn skills(&self) -> Result<SkillCatalog, ApiError> {
        self.read_skills().await
    }

    pub async fn refresh_skills(&self) -> Result<SkillCatalog, ApiError> {
        self.norte_code_command("unused", ClientRequestKind::RefreshSkills)
            .await?;
        self.read_skills().await
    }

    async fn read_skills(&self) -> Result<SkillCatalog, ApiError> {
        let response = self
            .norte_code_command("unused", ClientRequestKind::ListSkills)
            .await?;
        let payload = response
            .events
            .iter()
            .find(|frame| CanonicalEvent::agent_skills().matches(&frame.payload))
            .ok_or_else(|| {
                ApiError::InvalidResponse("Skill catalogue was not returned.".to_owned())
            })?;
        let entries: AgentCatalogSkillsResponse = decode_payload(&payload.payload["entries"])?;
        Ok(SkillCatalog {
            skills: entries.skills,
            warnings: entries.warnings,
        })
    }

    pub async fn tool_rules(&self) -> Result<Vec<ToolRule>, ApiError> {
        let response = self
            .norte_code_command("unused", ClientRequestKind::ListTools { session: None })
            .await?;
        let payload = response
            .events
            .iter()
            .find(|frame| CanonicalEvent::tool_list().matches(&frame.payload))
            .ok_or_else(|| ApiError::InvalidResponse("Tool rules were not returned.".to_owned()))?;
        decode_payload(&payload.payload["data"])
    }

    pub async fn set_tool_allowed(&self, tool: &str, allow: bool) -> Result<(), ApiError> {
        self.norte_code_command(
            "unused",
            ClientRequestKind::SetApprovalRule {
                session: None,
                tool: tool.to_owned(),
                allow,
                global: true,
            },
        )
        .await
        .map(|_| ())
    }

    pub async fn forget_tool_rule(&self, tool: &str) -> Result<(), ApiError> {
        self.norte_code_command(
            "unused",
            ClientRequestKind::ForgetApprovalRule {
                session: None,
                tool: tool.to_owned(),
                global: true,
            },
        )
        .await
        .map(|_| ())
    }
}
