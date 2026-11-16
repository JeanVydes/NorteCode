use norte_code_wire::{CanonicalEvent, ClientRequestKind};

use super::shared::{decode_payload, find_event};
use super::types::{AgentCatalogSkillsResponse, SkillCatalog, ToolRule};
use super::{ApiError, RestClient};

impl RestClient {
    /// The installed skills and the ones that were skipped.
    ///
    /// # Errors
    ///
    /// Returns the transport error or an answer the shape does not describe.
    pub async fn skills(&self) -> Result<SkillCatalog, ApiError> {
        self.read_skills().await
    }

    /// Re-scans the skills roots and returns the catalogue.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn refresh_skills(&self) -> Result<SkillCatalog, ApiError> {
        self.command_ok(None, ClientRequestKind::RefreshSkills)
            .await?;
        self.read_skills().await
    }

    async fn read_skills(&self) -> Result<SkillCatalog, ApiError> {
        let response = self.command(None, ClientRequestKind::ListSkills).await?;
        let payload = find_event(&response, CanonicalEvent::agent_skills())?;
        let entries: AgentCatalogSkillsResponse = decode_payload(&payload["entries"])?;
        Ok(SkillCatalog {
            skills: entries.skills,
            warnings: entries.warnings,
        })
    }

    /// The tool approval rules.
    ///
    /// # Errors
    ///
    /// Returns the transport error or an answer the shape does not describe.
    pub async fn tool_rules(&self) -> Result<Vec<ToolRule>, ApiError> {
        let response = self
            .command(None, ClientRequestKind::ListTools { session: None })
            .await?;
        let payload = find_event(&response, CanonicalEvent::tool_list())?;
        decode_payload(&payload["data"])
    }

    /// Allows or denies one tool globally.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn set_tool_allowed(&self, tool: &str, allow: bool) -> Result<(), ApiError> {
        self.command_ok(
            None,
            ClientRequestKind::SetApprovalRule {
                session: None,
                tool: tool.to_owned(),
                allow,
                global: true,
            },
        )
        .await
    }

    /// Removes the global rule for one tool.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn forget_tool_rule(&self, tool: &str) -> Result<(), ApiError> {
        self.command_ok(
            None,
            ClientRequestKind::ForgetApprovalRule {
                session: None,
                tool: tool.to_owned(),
                global: true,
            },
        )
        .await
    }
}
