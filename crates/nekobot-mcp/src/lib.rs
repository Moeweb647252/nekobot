//! MCP (Model Context Protocol) middleware — connects to external MCP servers
//! and registers their tools into the agent's [`ToolRegistry`](nekobot_core::agent::tool::ToolRegistry).
//!
//! Uses the official [`rmcp`] crate for the MCP client protocol.
//!
//! Supports two transports:
//! - `transport: http`  (default) — Streamable HTTP, e.g. `url: http://localhost:8080/mcp`
//! - `transport: stdio` — spawns a child process, e.g. `command: npx` + `args: [...]`

use std::sync::{Arc, RwLock};
use std::time::Duration;

use nekobot_core::agent::{
    Context,
    middleware::{Middleware, MiddlewareFlow},
    tool::{Tool, ToolError, ToolResult, ToolSpec},
    types::ChatRequest,
};
use rmcp::{
    handler::client::ClientHandler,
    model::{CallToolRequestParams, PaginatedRequestParams},
    service::{Peer, RoleClient, serve_client},
    transport::{
        child_process::TokioChildProcess,
        streamable_http_client::StreamableHttpClientTransport,
    },
};
use serde::Deserialize;
use serde_json::Value;
use tracing::debug;

/// Deserialized from `MiddlewareConfig.data` via `#[serde(tag = "transport")]`.
///
/// ```yaml
/// # HTTP
/// - name: mcp
///   server: web-search
///   transport: http          # default
///   url: http://...
///
/// # Stdio
/// - name: mcp
///   server: filesystem
///   transport: stdio
///   command: npx
///   args: ["-y", "@modelcontextprotocol/server-filesystem", "/path"]
/// ```
#[derive(Deserialize)]
#[serde(tag = "transport")]
pub enum McpConfig {
    /// Streamable HTTP transport (the default when `transport` is absent).
    #[serde(rename = "http")]
    Http { server: String, url: String },
    /// Stdio transport — spawns a child process.
    #[serde(rename = "stdio")]
    Stdio {
        server: String,
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

/// Middleware that connects to an MCP server and registers its tools.
pub struct McpMiddleware {
    config: McpConfig,
    tool_specs: RwLock<Vec<ToolSpec>>,
}

impl McpMiddleware {
    /// Create from a parsed [`McpConfig`].
    pub fn from_config(config: McpConfig) -> Self {
        Self {
            config,
            tool_specs: RwLock::new(Vec::new()),
        }
    }
}

/// Empty client handler — in client role we don't need to handle requests from the server.
struct EmptyHandler;

#[async_trait::async_trait]
impl ClientHandler for EmptyHandler {}

#[async_trait::async_trait]
impl Middleware for McpMiddleware {
    fn name(&self) -> &'static str {
        "mcp"
    }

    async fn before_chat(
        &self,
        _ctx: &Context,
        request: &mut ChatRequest,
    ) -> Result<MiddlewareFlow, anyhow::Error> {
        let specs = self.tool_specs.read().map_err(|e| {
            anyhow::anyhow!("MCP tool_specs lock poisoned: {e}")
        })?;
        request.tools.extend(specs.iter().cloned());
        Ok(MiddlewareFlow::Continue)
    }

    async fn init(&self, ctx: &Context) -> Result<(), anyhow::Error> {
        debug!("connecting to MCP server {}", self.name());
        let (server, peer) =
            match &self.config {
                McpConfig::Http { server, url } => {
                    let transport = StreamableHttpClientTransport::from_uri(url.as_str());
                    let running = Box::new(
                        tokio::time::timeout(
                            Duration::from_secs(30),
                            serve_client(EmptyHandler, transport),
                        )
                        .await
                        .map_err(|_| anyhow::anyhow!("timeout connecting to MCP server: {url}"))?
                        .map_err(|e| anyhow::anyhow!("failed to connect to MCP server: {e}"))?,
                    );
                    let peer = running.peer().clone();
                    Box::leak(running);
                    (server.as_str(), peer)
                }
                McpConfig::Stdio {
                    server,
                    command,
                    args,
                } => {
                    let mut cmd = tokio::process::Command::new(command);
                    cmd.args(args);
                    // New process group so the child doesn't receive Ctrl+C
                    #[cfg(unix)]
                    cmd.process_group(0);

                    let builder = TokioChildProcess::builder(cmd)
                        .stderr(std::process::Stdio::piped());
                    let (transport, stderr) = builder
                        .spawn()
                        .map_err(|e| anyhow::anyhow!("failed to spawn MCP child process: {e}"))?;

                    // Drain stderr in the background so the child doesn't block
                    if let Some(stderr) = stderr {
                        use tokio::io::AsyncBufReadExt;
                        tokio::spawn(async move {
                            let reader = tokio::io::BufReader::new(stderr);
                            let mut lines = reader.lines();
                            while let Ok(Some(line)) = lines.next_line().await {
                                tracing::debug!(target: "mcp.stdio", "{line}");
                            }
                        });
                    }

                    let running = Box::new(
                        tokio::time::timeout(
                            Duration::from_secs(30),
                            serve_client(EmptyHandler, transport),
                        )
                        .await
                        .map_err(|_| anyhow::anyhow!("timeout connecting to MCP server"))?
                        .map_err(|e| anyhow::anyhow!("failed to connect to MCP server: {e}"))?,
                    );
                    let peer = running.peer().clone();
                    Box::leak(running);
                    (server.as_str(), peer)
                }
            };

        let tools = peer
            .list_tools(Some(PaginatedRequestParams::default()))
            .await
            .map_err(|e| anyhow::anyhow!("failed to list MCP tools: {e}"))?;

        let mut specs = self.tool_specs.write().map_err(|e| {
            anyhow::anyhow!("MCP tool_specs lock poisoned: {e}")
        })?;

        for tool in tools.tools {
            let key = format!("mcp_{server}_{}", tool.name);
            tracing::info!(target: "mcp", "registering {key}");

            let input_schema: Value = serde_json::to_value(&*tool.input_schema).unwrap_or_default();

            specs.push(ToolSpec {
                name: key.clone(),
                description: tool.description.as_deref().unwrap_or("").to_string(),
                parameters_schema: input_schema.clone(),
            });

            ctx.tool_registry().register(Arc::new(McpTool {
                key,
                peer: peer.clone(),
                tool_name: tool.name.to_string(),
                description: tool.description.as_deref().unwrap_or("").to_string(),
                input_schema,
            }))?;
        }

        Ok(())
    }
}

/// Wraps an MCP server tool as a nekobot [`Tool`].
struct McpTool {
    key: String,
    peer: Peer<RoleClient>,
    tool_name: String,
    description: String,
    input_schema: Value,
}

#[async_trait::async_trait]
impl Tool for McpTool {
    fn name(&self) -> &'static str {
        Box::leak(self.key.clone().into_boxed_str())
    }

    fn description(&self) -> &'static str {
        Box::leak(self.description.clone().into_boxed_str())
    }

    fn parameters_schema(&self) -> Value {
        self.input_schema.clone()
    }

    async fn call(&self, args: Value) -> ToolResult<Value> {
        let args_map = match args {
            Value::Object(map) => map,
            Value::Null => serde_json::Map::new(),
            other => {
                return Err(ToolError::InvalidArguments(format!(
                    "expected JSON object, got {other}"
                )));
            }
        };

        let params = CallToolRequestParams::new(self.tool_name.clone()).with_arguments(args_map);

        let result = self
            .peer
            .call_tool(params)
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        Ok(serde_json::to_value(result.content)
            .unwrap_or_else(|_| Value::String("(unserializable)".into())))
    }
}
