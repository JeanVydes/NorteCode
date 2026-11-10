//! One group of endpoints the client exposes.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

impl ApiClient {
    pub async fn mcp_catalog(&self) -> Result<McpCatalog, ApiError> {
        self.read_mcp_catalog().await
    }

    pub async fn refresh_mcp(&self) -> Result<McpCatalog, ApiError> {
        self.norte_code_command(None, ClientRequestKind::RefreshMcp)
            .await?;
        self.read_mcp_catalog().await
    }

    async fn read_mcp_catalog(&self) -> Result<McpCatalog, ApiError> {
        let response = self
            .norte_code_command(None, ClientRequestKind::ListMcp)
            .await?;
        let payload = response
            .events
            .iter()
            .find(|frame| CanonicalEvent::mcp_list().matches(&frame.payload))
            .ok_or_else(|| {
                ApiError::InvalidResponse("MCP server list was not returned.".to_owned())
            })?;
        let data: McpListResponse = decode_payload(&payload.payload["data"])?;
        Ok(McpCatalog {
            servers: data.servers,
            failures: data.failures,
        })
    }

    pub async fn set_mcp_active(&self, name: &str, active: bool) -> Result<(), ApiError> {
        self.norte_code_command(
            None,
            ClientRequestKind::SetMcpActive {
                name: name.to_owned(),
                active,
            },
        )
        .await
        .map(|_| ())
    }
}
