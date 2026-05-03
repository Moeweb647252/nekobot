//! Scripting middleware — registers an `eval_ts` tool that lets agents execute
//! TypeScript code in a sandboxed JavaScript runtime (Boa + SWC).

mod runner;
mod ts_check;

use std::sync::RwLock;

use nekobot_core::agent::{
    Context,
    middleware::{Middleware, MiddlewareFlow},
    tool::{Tool, ToolError, ToolResult, ToolSpec},
    types::ChatRequest,
};
use serde::Deserialize;
use serde_json::Value;

/// Deserialized from `MiddlewareConfig.data`.
///
/// ```yaml
/// - name: script
///   timeout_ms: 5000
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct ScriptConfig {
    /// Max JS execution time in milliseconds. Default 5000.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_timeout() -> u64 {
    5000
}

/// Middleware that registers the `eval_ts` tool.
pub struct ScriptMiddleware {
    tool_specs: RwLock<Vec<ToolSpec>>,
    timeout_ms: u64,
}

impl ScriptMiddleware {
    /// Create from a parsed [`ScriptConfig`].
    pub fn from_config(config: ScriptConfig) -> Self {
        Self {
            tool_specs: RwLock::new(Vec::new()),
            timeout_ms: config.timeout_ms,
        }
    }
}

#[async_trait::async_trait]
impl Middleware for ScriptMiddleware {
    fn name(&self) -> &'static str {
        "script"
    }

    async fn before_chat(
        &self,
        _ctx: &Context,
        request: &mut ChatRequest,
    ) -> Result<MiddlewareFlow, anyhow::Error> {
        let specs = self
            .tool_specs
            .read()
            .map_err(|e| anyhow::anyhow!("script tool_specs lock poisoned: {e}"))?;
        request.tools.extend(specs.iter().cloned());
        Ok(MiddlewareFlow::Continue)
    }

    async fn init(&self, ctx: &Context) -> Result<(), anyhow::Error> {
        let tool = Arc::new(EvalTsTool {
            timeout_ms: self.timeout_ms,
        });

        let spec = ToolSpec {
            name: tool.name().to_owned(),
            description: tool.description().to_owned(),
            parameters_schema: tool.parameters_schema(),
        };

        self.tool_specs
            .write()
            .map_err(|e| anyhow::anyhow!("script tool_specs lock poisoned: {e}"))?
            .push(spec);

        ctx.tool_registry().register(tool)?;
        Ok(())
    }
}

/// Tool that transpiles TypeScript → JavaScript via SWC and executes it via Boa.
struct EvalTsTool {
    timeout_ms: u64,
}

#[async_trait::async_trait]
impl Tool for EvalTsTool {
    fn name(&self) -> &str {
        "eval_ts"
    }

    fn description(&self) -> &str {
        "Execute TypeScript code in a Boajs runtime. \
         The code must use strict types (no `any`). \
         fetch, console and other Web APIs supported by boa_runtime are available. \
         Returns the result of the last expression as a string."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "code": {
                    "type": "string",
                    "description": "TypeScript code to execute. Must not use `any` type."
                }
            },
            "required": ["code"]
        })
    }

    async fn call(&self, args: Value) -> ToolResult<Value> {
        let code = args
            .get("code")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("missing 'code' parameter".to_owned()))?;

        let js_code = ts_check::transpile(code)
            .map_err(|e| ToolError::Execution(format!("TypeScript error: {e}")))?;

        let fut = runner::execute(js_code);
        let result = if self.timeout_ms > 0 {
            tokio::time::timeout(std::time::Duration::from_millis(self.timeout_ms), fut)
                .await
                .map_err(|_| ToolError::Execution("eval_ts timed out".to_owned()))?
        } else {
            fut.await
        };

        Ok(Value::String(result.map_err(ToolError::Execution)?))
    }
}

use std::sync::Arc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transpile_valid_ts() {
        let js = ts_check::transpile("let x: number = 1 + 2;").expect("should transpile");
        assert!(js.contains("let x"));
        assert!(!js.contains(": number"));
    }

    #[test]
    fn transpile_rejects_any() {
        let err = ts_check::transpile("let x: any = 1;").expect_err("should reject any");
        assert!(err.to_string().contains("`any` type is not allowed"));
    }

    #[test]
    fn transpile_rejects_any_in_function() {
        let err = ts_check::transpile("function f(x: any): any { return x; }")
            .expect_err("should reject any");
        assert!(err.to_string().contains("`any` type is not allowed"));
    }

    #[tokio::test]
    async fn runner_executes_js() {
        let result = runner::execute("1 + 2".to_owned())
            .await
            .expect("should execute");
        assert_eq!(result, "3");
    }

    #[tokio::test]
    async fn runner_executes_string() {
        let result = runner::execute("'hello'".to_owned())
            .await
            .expect("should execute");
        assert_eq!(result, "hello");
    }

    #[tokio::test]
    async fn runner_reports_js_error() {
        let err = runner::execute("throw new Error('boom')".to_owned())
            .await
            .expect_err("should fail");
        assert!(err.contains("boom") || err.contains("JS execution error"));
    }

    #[tokio::test]
    async fn eval_ts_end_to_end() {
        let js = ts_check::transpile("40 + 2").expect("transpile");
        let result = runner::execute(js).await.expect("execute");
        assert_eq!(result, "42");
    }
}
