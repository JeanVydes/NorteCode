//! One MCP server the agent may reach.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One MCP server the agent may reach.
///
/// An entry names either a program to run, which speaks the protocol over its
/// standard input and output and is what a server published on `npm` or `PyPI`
/// expects, or a `url` to talk to over Streamable HTTP. Exactly one of the two
/// is required.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct McpServerConfig {
    /// The program to run, for a local server.
    pub command: String,
    /// The endpoint, for a remote server.
    ///
    /// A server that has one is reached over Streamable HTTP instead of being
    /// started, so `command` is ignored.
    pub url: Option<String>,
    /// The headers every request to a remote server carries.
    ///
    /// A token goes here as `Authorization`, by value or read from the
    /// environment; the values are registered for redaction like any other
    /// secret.
    pub headers: BTreeMap<String, String>,
    /// Its arguments, in order.
    pub args: Vec<String>,
    /// Extra environment variables for the child.
    ///
    /// A server that needs a token gets it here, by name, and the value is
    /// registered for redaction like any other secret.
    pub env: BTreeMap<String, String>,
    /// Whether the server is started. Off lets an entry be kept without
    /// paying for its process on every run.
    pub enabled: bool,
    /// The tools of this server that may reach the model.
    ///
    /// Empty means every tool the server publishes, minus the ones in `deny`.
    /// Names are written as the server names them, without the
    /// `mcp__<server>__` prefix the model sees.
    pub allow: Vec<String>,
    /// The tools of this server that never reach the model.
    ///
    /// `deny` wins over `allow`, so naming a tool in both is a way to keep it
    /// out while leaving it in a copied `allow` list.
    pub deny: Vec<String>,
}

impl McpServerConfig {
    /// How the startup line names this server.
    #[must_use]
    pub fn target(&self) -> String {
        self.url.clone().unwrap_or_else(|| self.command.clone())
    }

    /// Whether this entry reaches a remote endpoint.
    #[must_use]
    pub fn is_remote(&self) -> bool {
        self.url.as_ref().is_some_and(|url| !url.trim().is_empty())
    }

    /// The tools of `published` this entry lets through.
    ///
    /// A tool is kept when it is not denied and either nothing is allowed or it
    /// is named in `allow`.
    #[must_use]
    pub fn select<'a, T>(&self, published: impl Iterator<Item = &'a T>) -> Vec<&'a T>
    where
        T: AsRef<str> + 'a,
    {
        published
            .filter(|tool| {
                let name = tool.as_ref();
                !self.deny.iter().any(|denied| denied == name)
                    && (self.allow.is_empty() || self.allow.iter().any(|allowed| allowed == name))
            })
            .collect()
    }
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            url: None,
            headers: BTreeMap::new(),
            args: Vec::new(),
            env: BTreeMap::new(),
            enabled: true,
            allow: Vec::new(),
            deny: Vec::new(),
        }
    }
}
