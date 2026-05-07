use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use nekobot_core::agent::{
    Context,
    middleware::{Middleware, MiddlewareFlow},
    tool::Tool,
    types::ChatRequest,
};
use serde::{Deserialize, Serialize};

use crate::{runtime::Runtime, tools::eval_ts::EvalTsTool};
mod runtime;
mod tools;
mod utils;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptConfig {
    timeout_seconds: u64,
}

pub struct ScriptMiddleware {
    config: ScriptConfig,
    tool_specs: RwLock<Vec<nekobot_core::agent::tool::ToolSpec>>,
    runtime_handle: runtime::RuntimeHandle,
    runtime_join_handle: tokio::task::JoinHandle<()>,
}

impl ScriptMiddleware {
    pub fn from_config(config: ScriptConfig) -> Self {
        let (runtime_handle, task_receiver) = runtime::RuntimeHandle::new();
        let runtime_join_handle = tokio::task::spawn_blocking(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(
                    Runtime::try_new(task_receiver)
                        .expect("Failed to create script runtime")
                        .start(),
                );
        });
        ScriptMiddleware {
            config,
            tool_specs: RwLock::new(Vec::new()),
            runtime_handle,
            runtime_join_handle,
        }
    }
}

#[async_trait]
impl Middleware for ScriptMiddleware {
    async fn init(&self, context: &Context) -> anyhow::Result<()> {
        let mut tool_specs = self
            .tool_specs
            .write()
            .map_err(|_| anyhow::anyhow!("tool specs lock poisoned"))?;
        let eval_ts_tool =
            EvalTsTool::new(self.config.timeout_seconds, self.runtime_handle.clone());
        tool_specs.push(nekobot_core::agent::tool::ToolSpec {
            name: eval_ts_tool.name().to_string(),
            description: eval_ts_tool.description().to_string(),
            parameters_schema: eval_ts_tool.parameters_schema(),
        });
        context.tool_registry.register(Arc::new(eval_ts_tool))?;
        Ok(())
    }

    async fn before_chat(
        &self,
        _context: &Context,
        request: &mut ChatRequest,
    ) -> Result<MiddlewareFlow, anyhow::Error> {
        request.tools.extend(
            self.tool_specs
                .read()
                .map_err(|_| anyhow::anyhow!("tool specs lock poisoned"))?
                .clone(),
        );
        Ok(MiddlewareFlow::Continue)
    }
}

impl Drop for ScriptMiddleware {
    fn drop(&mut self) {
        self.runtime_join_handle.abort();
    }
}
