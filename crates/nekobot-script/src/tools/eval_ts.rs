use anyhow::Context;
use async_trait::async_trait;
use nekobot_core::agent::tool::{Tool, ToolError, ToolResult};
use serde_json::Value;

use crate::runtime::RuntimeHandle;

pub struct EvalTsTool {
    pub timeout_seconds: u64,
    pub handle: RuntimeHandle,
}

impl EvalTsTool {
    pub fn new(timeout_seconds: u64, handle: RuntimeHandle) -> Self {
        Self {
            timeout_seconds,
            handle,
        }
    }
}

#[async_trait]
impl Tool for EvalTsTool {
    fn name(&self) -> &str {
        "eval_ts"
    }

    fn description(&self) -> &str {
        "Evaluate TypeScript code and return the result. setTimeout, setInterval, fetch are available. async code is supported."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "code": { "type": "string", "description": "The TypeScript code to evaluate" }
            },
            "required": ["code"]
        })
    }

    async fn call(&self, mut args: Value) -> ToolResult<serde_json::Value> {
        let code = match args
            .get_mut("code")
            .ok_or(ToolError::InvalidArguments(
                "missing 'code' parameter".to_string(),
            ))?
            .take()
        {
            Value::String(s) => Some(s),
            _ => None,
        }
        .ok_or(ToolError::InvalidArguments(
            "'code' parameter must be a string".to_string(),
        ))?;
        let js_code = crate::utils::transpile(code)
            .context("failed to transpile TypeScript code")
            .map_err(|e| ToolError::Execution(format!("transpilation error: {e}")))?;
        self.handle
            .eval(js_code)
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))
    }
}
