use async_trait::async_trait;
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),

    #[error("tool execution failed: {0}")]
    Execution(String),

    #[error("tool not found: {0}")]
    NotFound(String),
}

pub type ToolResult<T> = Result<T, ToolError>;

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;

    fn description(&self) -> &'static str;

    fn parameters_schema(&self) -> Value;

    async fn call(&self, args: Value) -> ToolResult<Value>;
}
