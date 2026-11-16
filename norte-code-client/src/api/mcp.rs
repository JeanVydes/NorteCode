use norte_code_wire::{CanonicalEvent, ClientRequestKind};

use super::shared::{decode_payload, find_event};
use super::types::{McpCatalog, McpListResponse};
use super::{ApiError, RestClient};

impl RestClient {
    /// The MCP servers and the ones that failed to start.
    ///
    /// # Errors
    ///
    /// Returns the transport error or an answer the shape does not describe.
    pub async fn mcp_catalog(&self) -> Result<McpCatalog, ApiError> {
        self.read_mcp_catalog().await
    }

    /// Restarts the MCP servers and returns the catalogue.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn refresh_mcp(&self) -> Result<McpCatalog, ApiError> {
        self.command_ok(None, ClientRequestKind::RefreshMcp).await?;
        self.read_mcp_catalog().await
    }

    async fn read_mcp_catalog(&self) -> Result<McpCatalog, ApiError> {
        let response = self.command(None, ClientRequestKind::ListMcp).await?;
        let payload = find_event(&response, CanonicalEvent::mcp_list())?;
        let data: McpListResponse = decode_payload(&payload["data"])?;
        Ok(McpCatalog {
            servers: data.servers,
            failures: data.failures,
        })
    }

    /// Enables or disables one MCP server.
    ///
    /// # Errors
    ///
    /// Returns the transport error or the reason the host refused.
    pub async fn set_mcp_active(&self, name: &str, active: bool) -> Result<(), ApiError> {
        self.command_ok(
            None,
            ClientRequestKind::SetMcpActive {
                name: name.to_owned(),
                active,
            },
        )
        .await
    }
}
