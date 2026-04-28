use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

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

#[derive(Default)]
pub struct ToolRegistry {
    tools: RwLock<BTreeMap<String, Arc<dyn Tool>>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, tool: Arc<dyn Tool>) -> anyhow::Result<()> {
        let name = tool.name();
        if name.trim().is_empty() {
            anyhow::bail!("tool name cannot be empty");
        }

        let mut tools = self
            .tools
            .write()
            .map_err(|_| anyhow::anyhow!("tool registry lock poisoned"))?;

        if tools.contains_key(name) {
            anyhow::bail!("tool already registered: {name}");
        }

        tools.insert(name.to_owned(), tool);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.read().ok()?.get(name).cloned()
    }

    pub fn tool_specs(&self) -> anyhow::Result<Vec<ToolSpec>> {
        let tools = self
            .tools
            .read()
            .map_err(|_| anyhow::anyhow!("tool registry lock poisoned"))?;

        Ok(tools
            .values()
            .map(|tool| ToolSpec::from_tool(tool.as_ref()))
            .collect())
    }

    pub fn is_empty(&self) -> anyhow::Result<bool> {
        let tools = self
            .tools
            .read()
            .map_err(|_| anyhow::anyhow!("tool registry lock poisoned"))?;

        Ok(tools.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;

    struct TestTool {
        name: &'static str,
    }

    #[async_trait]
    impl Tool for TestTool {
        fn name(&self) -> &'static str {
            self.name
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
        let tool = TestTool { name: "test" };
        let spec = ToolSpec::from_tool(&tool);

        assert_eq!(spec.name, "test");
        assert_eq!(spec.description, "test tool");
        assert_eq!(spec.parameters_schema["type"], "object");
    }

    #[test]
    fn tool_registry_builds_specs_for_registered_tools() -> anyhow::Result<()> {
        let registry = ToolRegistry::new();

        registry.register(Arc::new(TestTool { name: "test" }))?;

        let specs = registry.tool_specs()?;
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "test");
        assert_eq!(specs[0].description, "test tool");
        assert_eq!(specs[0].parameters_schema["type"], "object");
        assert!(!registry.is_empty()?);
        Ok(())
    }

    #[test]
    fn tool_registry_rejects_duplicate_names() -> anyhow::Result<()> {
        let registry = ToolRegistry::new();

        registry.register(Arc::new(TestTool { name: "test" }))?;
        let result = registry.register(Arc::new(TestTool { name: "test" }));

        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn tool_registry_get_returns_registered_tool() -> anyhow::Result<()> {
        let registry = ToolRegistry::new();

        registry.register(Arc::new(TestTool { name: "test" }))?;

        let tool = registry.get("test").expect("tool should be registered");
        assert_eq!(tool.name(), "test");
        assert!(registry.get("missing").is_none());
        Ok(())
    }
}
