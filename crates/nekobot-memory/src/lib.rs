//! Vector memory middleware — persistent semantic memory for agents.

mod embedding;
mod entity;

use std::sync::{Arc, RwLock};

use nekobot_core::agent::{
    Context,
    middleware::{Middleware, MiddlewareFlow},
    tool::{Tool, ToolError, ToolResult, ToolSpec},
    types::ChatRequest,
};
use serde::Deserialize;
use serde_json::Value;
use turso::Connection;

use embedding::EmbeddingClient;

/// Deserialized from `MiddlewareConfig.data`.
#[derive(Debug, Clone, Deserialize)]
pub struct MemoryConfig {
    pub embedding_url: String,
    pub embedding_key: String,
    pub embedding_model: String,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
}

fn default_max_results() -> usize {
    5
}

pub struct MemoryMiddleware {
    config: MemoryConfig,
    tool_specs: RwLock<Vec<ToolSpec>>,
    app_db: RwLock<Option<Connection>>,
}

impl MemoryMiddleware {
    pub fn from_config(config: MemoryConfig) -> Self {
        Self {
            config,
            tool_specs: RwLock::new(Vec::new()),
            app_db: RwLock::new(None),
        }
    }

    fn conn(&self) -> anyhow::Result<Connection> {
        self.app_db
            .read()
            .map_err(|e| anyhow::anyhow!("memory lock poisoned: {e}"))?
            .clone()
            .ok_or_else(|| anyhow::anyhow!("memory not initialized"))
    }

    fn embed_client(&self) -> EmbeddingClient {
        EmbeddingClient::new(
            self.config.embedding_url.clone(),
            self.config.embedding_key.clone(),
            self.config.embedding_model.clone(),
        )
    }
}

#[async_trait::async_trait]
impl Middleware for MemoryMiddleware {
    fn name(&self) -> &'static str {
        "memory"
    }

    async fn before_chat(
        &self,
        _ctx: &Context,
        request: &mut ChatRequest,
    ) -> Result<MiddlewareFlow, anyhow::Error> {
        let specs = self
            .tool_specs
            .read()
            .map_err(|e| anyhow::anyhow!("memory lock poisoned: {e}"))?;
        request.tools.extend(specs.iter().cloned());
        Ok(MiddlewareFlow::Continue)
    }

    async fn init(&self, ctx: &Context) -> Result<(), anyhow::Error> {
        let conn = ctx.app_db.clone();
        entity::create_table(&conn).await?;

        *self
            .app_db
            .write()
            .map_err(|e| anyhow::anyhow!("memory lock: {e}"))? = Some(conn);

        let ec = self.embed_client();
        let db = self.conn()?;

        let agent = ctx.agent_name.clone();
        let remember = Arc::new(RememberTool {
            app_db: db.clone(),
            embed_client: ec.clone(),
            agent_name: agent.clone(),
        });
        let search = Arc::new(SearchMemoryTool {
            app_db: db.clone(),
            embed_client: ec.clone(),
            agent_name: agent.clone(),
            max_results: self.config.max_results,
        });
        let forget = Arc::new(ForgetTool {
            app_db: db,
            embed_client: ec,
            agent_name: agent,
        });

        let mut specs = self
            .tool_specs
            .write()
            .map_err(|e| anyhow::anyhow!("memory lock: {e}"))?;
        for tool in [
            remember.as_ref() as &dyn Tool,
            search.as_ref(),
            forget.as_ref(),
        ] {
            specs.push(ToolSpec {
                name: tool.name().to_owned(),
                description: tool.description().to_owned(),
                parameters_schema: tool.parameters_schema(),
            });
        }
        drop(specs);

        ctx.tool_registry().register(remember)?;
        ctx.tool_registry().register(search)?;
        ctx.tool_registry().register(forget)?;
        Ok(())
    }
}

// ── Tools ──

#[derive(Clone)]
struct RememberTool {
    app_db: Connection,
    embed_client: EmbeddingClient,
    agent_name: String,
}

#[async_trait::async_trait]
impl Tool for RememberTool {
    fn name(&self) -> &str {
        "remember"
    }
    fn description(&self) -> &str {
        "Store a new memory. The content will be embedded and saved. \
         If a very similar memory already exists, it will not be duplicated."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": { "type": "string", "description": "The information to remember" }
            },
            "required": ["content"]
        })
    }
    async fn call(&self, args: Value) -> ToolResult<Value> {
        let content = args
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("missing 'content'".to_owned()))?;

        let embedding = self
            .embed_client
            .embed(content)
            .await
            .map_err(|e| ToolError::Execution(format!("embed: {e}")))?;

        // Dedup check
        let existing = entity::search(&self.app_db, &self.agent_name, &embedding, 1)
            .await
            .map_err(|e| ToolError::Execution(format!("search: {e}")))?;
        if let Some(row) = existing.first() {
            return Ok(Value::String(format!(
                "Similar memory already exists: \"{}\"",
                row.content
            )));
        }

        entity::insert(&self.app_db, &self.agent_name, content, &embedding)
            .await
            .map_err(|e| ToolError::Execution(format!("insert: {e}")))?;

        Ok(Value::String(format!("Remembered: \"{content}\"")))
    }
}

#[derive(Clone)]
struct SearchMemoryTool {
    app_db: Connection,
    embed_client: EmbeddingClient,
    agent_name: String,
    max_results: usize,
}

#[async_trait::async_trait]
impl Tool for SearchMemoryTool {
    fn name(&self) -> &str {
        "search_memory"
    }
    fn description(&self) -> &str {
        "Search stored memories by semantic similarity."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "The search query" }
            },
            "required": ["query"]
        })
    }
    async fn call(&self, args: Value) -> ToolResult<Value> {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("missing 'query'".to_owned()))?;

        let embedding = self
            .embed_client
            .embed(query)
            .await
            .map_err(|e| ToolError::Execution(format!("embed: {e}")))?;

        let results = entity::search(&self.app_db, &self.agent_name, &embedding, self.max_results)
            .await
            .map_err(|e| ToolError::Execution(format!("search: {e}")))?;

        let items: Vec<Value> = results
            .iter()
            .map(|r| serde_json::json!({"id": r.id, "content": r.content}))
            .collect();
        Ok(Value::Array(items))
    }
}

#[derive(Clone)]
struct ForgetTool {
    app_db: Connection,
    embed_client: EmbeddingClient,
    agent_name: String,
}

#[async_trait::async_trait]
impl Tool for ForgetTool {
    fn name(&self) -> &str {
        "forget"
    }
    fn description(&self) -> &str {
        "Forget a memory. Searches for the most similar match and deletes it."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": { "type": "string", "description": "The memory content to forget" }
            },
            "required": ["content"]
        })
    }
    async fn call(&self, args: Value) -> ToolResult<Value> {
        let content = args
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("missing 'content'".to_owned()))?;

        let embedding = self
            .embed_client
            .embed(content)
            .await
            .map_err(|e| ToolError::Execution(format!("embed: {e}")))?;

        let results = entity::search(&self.app_db, &self.agent_name, &embedding, 1)
            .await
            .map_err(|e| ToolError::Execution(format!("search: {e}")))?;

        let Some(row) = results.first() else {
            return Ok(Value::String("No matching memory found.".to_owned()));
        };

        entity::delete(&self.app_db, row.id)
            .await
            .map_err(|e| ToolError::Execution(format!("delete: {e}")))?;

        Ok(Value::String(format!("Forgot: \"{}\"", row.content)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_config_defaults() {
        let cfg: MemoryConfig = serde_json::from_value(serde_json::json!({
            "embedding_url": "https://api.example.com/embeddings",
            "embedding_key": "sk-test",
            "embedding_model": "test-model",
        }))
        .unwrap();
        assert_eq!(cfg.max_results, 5);
    }
}
