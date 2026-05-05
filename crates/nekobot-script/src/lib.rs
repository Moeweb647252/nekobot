//! Scripting middleware — registers `eval_ts` and `reset_ts` tools that let
//! agents execute TypeScript code in a persistent JavaScript runtime (Boa + SWC).

mod actor;
mod ts_check;

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use nekobot_core::agent::{
    Context,
    middleware::{Middleware, MiddlewareFlow},
    tool::{Tool, ToolError, ToolResult, ToolSpec},
    types::ChatRequest,
};
use serde::Deserialize;
use serde_json::Value;

use actor::{ActorHandle, ExecTaskState};

/// Deserialized from `MiddlewareConfig.data`.
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
///
/// # Field ownership
/// - `actor` is created in [`init`] and moved into tool structs via clone.
/// - `tasks` is `Arc`-wrapped so it can be shared with `SpawnTsTool`
///   and `TaskResultTool` which hold their own references.
pub struct ScriptMiddleware {
    tool_specs: RwLock<Vec<ToolSpec>>,
    timeout_ms: u64,
    actor: RwLock<Option<ActorHandle>>,
    tasks: Arc<RwLock<HashMap<String, ExecTaskState>>>,
}

impl ScriptMiddleware {
    /// Create from a parsed [`ScriptConfig`].
    /// The actor is spawned lazily in [`init`](Middleware::init) with session context.
    pub fn from_config(config: ScriptConfig) -> Self {
        Self {
            tool_specs: RwLock::new(Vec::new()),
            timeout_ms: config.timeout_ms,
            actor: RwLock::new(None),
            tasks: Arc::new(RwLock::new(HashMap::new())),
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
            .map_err(|e| anyhow::anyhow!("script lock: {e}"))?;
        request.tools.extend(specs.iter().cloned());

        let existing = request.system_prompt.take().unwrap_or_default();
        request.system_prompt = Some(format!(
            "{existing}\n\n\
            The `nekobot` global is available in eval_ts / spawn_ts:\n\
            ```ts\n\
            interface Nekobot {{\n\
              readonly session: {{\n\
                readonly id: number;\n\
                readonly agentName: string;\n\
              }};\n\
              notify(message: string): void;  // triggers agent interaction\n\
            }}\n\
            declare const nekobot: Nekobot;\n\
            ```"
        ));
        Ok(MiddlewareFlow::Continue)
    }

    async fn init(&self, ctx: &Context) -> Result<(), anyhow::Error> {
        let handle = ActorHandle::spawn(ctx.clone());
        let eval = Arc::new(EvalTsTool {
            handle: handle.clone(),
            timeout_ms: self.timeout_ms,
        });
        let reset = Arc::new(ResetTsTool {
            handle: handle.clone(),
        });
        let spawn = Arc::new(SpawnTsTool {
            ctx: ctx.clone(),
            timeout_ms: self.timeout_ms,
            tasks: Arc::clone(&self.tasks),
        });
        let task_result = Arc::new(TaskResultTool {
            tasks: Arc::clone(&self.tasks),
        });

        let mut specs = self
            .tool_specs
            .write()
            .map_err(|e| anyhow::anyhow!("script lock: {e}"))?;
        for tool in [
            eval.as_ref() as &dyn Tool,
            reset.as_ref(),
            spawn.as_ref(),
            task_result.as_ref(),
        ] {
            specs.push(ToolSpec {
                name: tool.name().to_owned(),
                description: tool.description().to_owned(),
                parameters_schema: tool.parameters_schema(),
            });
        }
        drop(specs);

        ctx.tool_registry().register(eval)?;
        ctx.tool_registry().register(reset)?;
        ctx.tool_registry().register(spawn)?;
        ctx.tool_registry().register(task_result)?;

        *self
            .actor
            .write()
            .map_err(|e| anyhow::anyhow!("script lock: {e}"))? = Some(handle);
        Ok(())
    }
}

struct EvalTsTool {
    handle: ActorHandle,
    timeout_ms: u64,
}

#[async_trait::async_trait]
impl Tool for EvalTsTool {
    fn name(&self) -> &str {
        "eval_ts"
    }
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
        let code = args
            .get("code")
            .and_then(Value::as_str)
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

struct ResetTsTool {
    handle: ActorHandle,
}

#[async_trait::async_trait]
impl Tool for ResetTsTool {
    fn name(&self) -> &str {
        "reset_ts"
    }
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

struct SpawnTsTool {
    ctx: Context,
    timeout_ms: u64,
    tasks: Arc<RwLock<HashMap<String, ExecTaskState>>>,
}

#[async_trait::async_trait]
impl Tool for SpawnTsTool {
    fn name(&self) -> &str {
        "spawn_ts"
    }
    fn description(&self) -> &str {
        "Launch TypeScript code as a background task. Returns a taskId immediately. \
         The script runs in its own JavaScript context with `nekobot.notify()` available. \
         Use `ts_task_result` to check the result."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "code": { "type": "string", "description": "TypeScript code to run in background." }
            },
            "required": ["code"]
        })
    }
    async fn call(&self, args: Value) -> ToolResult<Value> {
        let code = args
            .get("code")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("missing 'code'".to_owned()))?;
        let js_code = ts_check::transpile(code)
            .map_err(|e| ToolError::Execution(format!("TypeScript error: {e}")))?;
        let task_id = ActorHandle::spawn_background(
            self.ctx.clone(),
            js_code,
            self.timeout_ms,
            Arc::clone(&self.tasks),
        );
        Ok(serde_json::json!({ "taskId": task_id }))
    }
}

struct TaskResultTool {
    tasks: Arc<RwLock<HashMap<String, ExecTaskState>>>,
}

#[async_trait::async_trait]
impl Tool for TaskResultTool {
    fn name(&self) -> &str {
        "ts_task_result"
    }
    fn description(&self) -> &str {
        "Check the result of a background TypeScript task launched by `spawn_ts`. \
         Returns status 'running' or 'done' with the output."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "taskId": { "type": "string", "description": "Task ID returned by spawn_ts." }
            },
            "required": ["taskId"]
        })
    }
    async fn call(&self, args: Value) -> ToolResult<Value> {
        let task_id = args
            .get("taskId")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("missing 'taskId'".to_owned()))?;
        let mut tasks = self
            .tasks
            .write()
            .map_err(|e| ToolError::Execution(format!("tasks lock: {e}")))?;
        match tasks.get(task_id).cloned() {
            Some(ExecTaskState::Done { output }) => {
                tasks.remove(task_id);
                Ok(serde_json::json!({ "status": "done", "output": output }))
            }
            Some(ExecTaskState::Running) => {
                Ok(serde_json::json!({ "status": "running" }))
            }
            None => Ok(serde_json::json!({ "status": "unknown", "message": "task not found" })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nekobot_core::agent::tool::ToolRegistry;

    async fn test_ctx() -> Context {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let db = turso::Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        Context::new("test_agent", 1, tx, Arc::new(ToolRegistry::new()), conn)
    }

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
        let handle = ActorHandle::spawn(test_ctx().await);
        let js = ts_check::transpile("let x: number = 1 + 2;").unwrap();
        handle.eval(js).await.unwrap();
        let result = handle.eval("x + 3".to_owned()).await.unwrap();
        assert_eq!(result, serde_json::json!(6));
    }

    #[tokio::test]
    async fn reset_clears_context() {
        let handle = ActorHandle::spawn(test_ctx().await);
        let js = ts_check::transpile("let x: number = 1;").unwrap();
        handle.eval(js).await.unwrap();
        handle.reset().await;
        let err = handle.eval("x".to_owned()).await.unwrap_err();
        assert!(err.contains("not defined") || err.contains("JS execution"));
    }

    #[tokio::test]
    async fn eval_ts_end_to_end() {
        let js = ts_check::transpile("40 + 2").expect("transpile");
        let handle = ActorHandle::spawn(test_ctx().await);
        let result = handle.eval(js).await.expect("execute");
        assert_eq!(result, serde_json::json!(42));
    }
}
