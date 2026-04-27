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

#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters_schema: Value,
}

impl ToolSpec {
    pub fn from_tool(tool: &dyn Tool) -> Self {
        Self {
            name: tool.name().to_owned(),
            description: tool.description().to_owned(),
            parameters_schema: tool.parameters_schema(),
        }
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;

    fn description(&self) -> &'static str;

    fn parameters_schema(&self) -> Value;

    async fn call(&self, args: Value) -> ToolResult<Value>;
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    struct TestTool;

    #[async_trait]
    impl Tool for TestTool {
        fn name(&self) -> &'static str {
            "test"
        }

        fn description(&self) -> &'static str {
            "test tool"
        }

        fn parameters_schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "value": { "type": "string" }
                }
            })
        }

        async fn call(&self, _args: Value) -> ToolResult<Value> {
            Ok(json!({ "ok": true }))
        }
    }

    #[test]
    fn tool_spec_copies_metadata_without_executable_tool() {
        let tool = TestTool;
        let spec = ToolSpec::from_tool(&tool);

        assert_eq!(spec.name, "test");
        assert_eq!(spec.description, "test tool");
        assert_eq!(spec.parameters_schema["type"], "object");
    }
}
