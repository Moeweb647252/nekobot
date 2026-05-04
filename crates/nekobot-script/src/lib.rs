//! Scripting middleware — registers `eval_ts` and `reset_ts` tools that let
//! agents execute TypeScript code in a persistent JavaScript runtime (Boa + SWC).

mod actor;
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

use actor::ActorHandle;

/// Deserialized from `MiddlewareConfig.data`.
#[derive(Debug, Clone, Deserialize)]
pub struct ScriptConfig {
    /// Max JS execution time in milliseconds. Default 5000.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_timeout() -> u64 { 5000 }

/// Middleware that registers the `eval_ts` tool.
pub struct ScriptMiddleware {
    tool_specs: RwLock<Vec<ToolSpec>>,
    timeout_ms: u64,
    actor: ActorHandle,
}

impl ScriptMiddleware {
    /// Create from a parsed [`ScriptConfig`].
    pub fn from_config(config: ScriptConfig) -> Self {
        Self {
            tool_specs: RwLock::new(Vec::new()),
            timeout_ms: config.timeout_ms,
            actor: ActorHandle::spawn(),
        }
    }
}

#[async_trait::async_trait]
impl Middleware for ScriptMiddleware {
    fn name(&self) -> &'static str { "script" }

    async fn before_chat(
        &self,
        _ctx: &Context,
        request: &mut ChatRequest,
    ) -> Result<MiddlewareFlow, anyhow::Error> {
        let specs = self.tool_specs.read()
            .map_err(|e| anyhow::anyhow!("script lock: {e}"))?;
        request.tools.extend(specs.iter().cloned());
        Ok(MiddlewareFlow::Continue)
    }

    async fn init(&self, ctx: &Context) -> Result<(), anyhow::Error> {
        let eval = Arc::new(EvalTsTool { handle: self.actor.clone(), timeout_ms: self.timeout_ms });
        let reset = Arc::new(ResetTsTool { handle: self.actor.clone() });

        let mut specs = self.tool_specs.write()
            .map_err(|e| anyhow::anyhow!("script lock: {e}"))?;
        for tool in [eval.as_ref() as &dyn Tool, reset.as_ref()] {
            specs.push(ToolSpec {
                name: tool.name().to_owned(),
                description: tool.description().to_owned(),
                parameters_schema: tool.parameters_schema(),
            });
        }
        drop(specs);

        ctx.tool_registry().register(eval)?;
        ctx.tool_registry().register(reset)?;
        Ok(())
    }
}

struct EvalTsTool { handle: ActorHandle, timeout_ms: u64 }

#[async_trait::async_trait]
impl Tool for EvalTsTool {
    fn name(&self) -> &str { "eval_ts" }
    fn description(&self) -> &str {
        "Execute TypeScript code in a persistent Boajs runtime. Variables and \
         imports persist across calls. Use `reset_ts` to clear all state. \
         fetch, console and other Web APIs are available."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "code": { "type": "string", "description": "TypeScript code. Must not use `any`." }
            },
            "required": ["code"]
        })
    }
    async fn call(&self, args: Value) -> ToolResult<Value> {
        let code = args.get("code").and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("missing 'code'".to_owned()))?;
        let js_code = ts_check::transpile(code)
            .map_err(|e| ToolError::Execution(format!("TypeScript error: {e}")))?;
        let fut = self.handle.eval(js_code);
        let result = if self.timeout_ms > 0 {
            tokio::time::timeout(std::time::Duration::from_millis(self.timeout_ms), fut)
                .await
                .map_err(|_| ToolError::Execution("eval_ts timed out".to_owned()))?
        } else {
            fut.await
        };
        Ok(result.map_err(ToolError::Execution)?)
    }
}

struct ResetTsTool { handle: ActorHandle }

#[async_trait::async_trait]
impl Tool for ResetTsTool {
    fn name(&self) -> &str { "reset_ts" }
    fn description(&self) -> &str {
        "Reset the TypeScript context. All variables, imports, and functions are cleared."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    async fn call(&self, _args: Value) -> ToolResult<Value> {
        self.handle.reset().await;
        Ok(Value::String("TypeScript context reset.".into()))
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
    async fn persistent_context_keeps_variables() {
        let handle = ActorHandle::spawn();
        let js = ts_check::transpile("let x: number = 1 + 2;").unwrap();
        handle.eval(js).await.unwrap();
        let result = handle.eval("x + 3".to_owned()).await.unwrap();
        assert_eq!(result, serde_json::json!(6));
    }

    #[tokio::test]
    async fn reset_clears_context() {
        let handle = ActorHandle::spawn();
        let js = ts_check::transpile("let x: number = 1;").unwrap();
        handle.eval(js).await.unwrap();
        handle.reset().await;
        let err = handle.eval("x".to_owned()).await.unwrap_err();
        assert!(err.contains("not defined") || err.contains("JS execution"));
    }

    #[tokio::test]
    async fn eval_ts_end_to_end() {
        let js = ts_check::transpile("40 + 2").expect("transpile");
        let handle = ActorHandle::spawn();
        let result = handle.eval(js).await.expect("execute");
        assert_eq!(result, serde_json::json!(42));
    }
}
