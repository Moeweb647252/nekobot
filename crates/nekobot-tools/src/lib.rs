//! Built-in utility tools — provides bash and current_time tools.

mod bash;
mod time;

use std::sync::{Arc, RwLock};

use nekobot_core::agent::{
    Context,
    middleware::{Middleware, MiddlewareFlow},
    tool::{Tool, ToolSpec},
    types::ChatRequest,
};
use serde::Deserialize;

const MAX_OUTPUT_BYTES: usize = 100 * 1024;

/// Deserialized from `MiddlewareConfig.data`.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolsConfig {
    /// Timeout for bash commands in seconds. Default 30.
    #[serde(default = "default_timeout")]
    pub bash_timeout_secs: u64,
    /// List of tool names to enable. Default: all.
    #[serde(default = "default_enabled")]
    pub enabled: Vec<String>,
}

fn default_timeout() -> u64 {
    30
}

fn default_enabled() -> Vec<String> {
    vec!["bash".to_owned(), "time".to_owned()]
}

/// Middleware that registers built-in utility tools.
pub struct ToolsMiddleware {
    tool_specs: RwLock<Vec<ToolSpec>>,
    config: ToolsConfig,
}

impl ToolsMiddleware {
    pub fn from_config(config: ToolsConfig) -> Self {
        Self {
            tool_specs: RwLock::new(Vec::new()),
            config,
        }
    }

    fn enabled(&self, name: &str) -> bool {
        self.config.enabled.is_empty() || self.config.enabled.iter().any(|e| e == name)
    }
}

#[async_trait::async_trait]
impl Middleware for ToolsMiddleware {
    fn name(&self) -> &'static str {
        "tools"
    }

    async fn before_chat(
        &self,
        _ctx: &Context,
        request: &mut ChatRequest,
    ) -> Result<MiddlewareFlow, anyhow::Error> {
        let specs = self
            .tool_specs
            .read()
            .map_err(|e| anyhow::anyhow!("tools tool_specs lock poisoned: {e}"))?;
        request.tools.extend(specs.iter().cloned());
        Ok(MiddlewareFlow::Continue)
    }

    async fn init(&self, ctx: &Context) -> Result<(), anyhow::Error> {
        let mut specs = self
            .tool_specs
            .write()
            .map_err(|e| anyhow::anyhow!("tools tool_specs lock poisoned: {e}"))?;

        if self.enabled("bash") {
            let tool = Arc::new(bash::BashTool {
                timeout_secs: self.config.bash_timeout_secs,
            });
            specs.push(ToolSpec {
                name: tool.name().to_owned(),
                description: tool.description().to_owned(),
                parameters_schema: tool.parameters_schema(),
            });
            ctx.tool_registry().register(tool)?;
        }

        if self.enabled("time") {
            let tool = Arc::new(time::CurrentTimeTool);
            specs.push(ToolSpec {
                name: tool.name().to_owned(),
                description: tool.description().to_owned(),
                parameters_schema: tool.parameters_schema(),
            });
            ctx.tool_registry().register(tool)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_config_defaults() {
        let cfg: ToolsConfig = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(cfg.bash_timeout_secs, 30);
        assert!(cfg.enabled.contains(&"bash".to_owned()));
        assert!(cfg.enabled.contains(&"time".to_owned()));
    }

    #[test]
    fn tools_config_custom() {
        let cfg: ToolsConfig = serde_json::from_value(serde_json::json!({
            "bash_timeout_secs": 10,
            "enabled": ["time"]
        }))
        .unwrap();
        assert_eq!(cfg.bash_timeout_secs, 10);
        assert_eq!(cfg.enabled, vec!["time"]);
    }
}
