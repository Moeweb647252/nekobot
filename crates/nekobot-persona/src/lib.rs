//! Persona middleware — persistent agent personality.

use std::sync::{Arc, RwLock};

use nekobot_core::agent::{
    Context,
    middleware::{Middleware, MiddlewareFlow},
    tool::{Tool, ToolError, ToolResult, ToolSpec},
    types::ChatRequest,
};
use serde_json::Value;
use turso::Connection;

/// Middleware that persists and injects agent persona.
pub struct PersonaMiddleware {
    tool_specs: RwLock<Vec<ToolSpec>>,
    app_db: RwLock<Option<Connection>>,
}

impl PersonaMiddleware {
    pub fn new() -> Self {
        Self {
            tool_specs: RwLock::new(Vec::new()),
            app_db: RwLock::new(None),
        }
    }

    fn conn(&self) -> anyhow::Result<Connection> {
        self.app_db
            .read()
            .map_err(|e| anyhow::anyhow!("persona lock: {e}"))?
            .clone()
            .ok_or_else(|| anyhow::anyhow!("persona not initialized"))
    }
}

#[async_trait::async_trait]
impl Middleware for PersonaMiddleware {
    fn name(&self) -> &'static str {
        "persona"
    }

    async fn before_chat(
        &self,
        ctx: &Context,
        request: &mut ChatRequest,
    ) -> Result<MiddlewareFlow, anyhow::Error> {
        {
            let specs = self
                .tool_specs
                .read()
                .map_err(|e| anyhow::anyhow!("persona lock: {e}"))?;
            request.tools.extend(specs.iter().cloned());
        }

        if let Ok(conn) = self.conn() {
            if let Some(persona) =
                nekobot_core::entity::persona::get(&conn, &ctx.agent_name).await?
            {
                let existing = request.system_prompt.take().unwrap_or_default();
                request.system_prompt = Some(format!("{existing}\n\nYour persona:\n{persona}"));
            }
        }

        Ok(MiddlewareFlow::Continue)
    }

    async fn init(&self, ctx: &Context) -> Result<(), anyhow::Error> {
        let conn = ctx.app_db.clone();
        nekobot_core::entity::persona::create_table(&conn).await?;
        *self
            .app_db
            .write()
            .map_err(|e| anyhow::anyhow!("persona lock: {e}"))? = Some(conn);

        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(SetPersonaTool {
                agent_name: ctx.agent_name.clone(),
                app_db: self.conn()?,
            }),
            Arc::new(GetPersonaTool {
                agent_name: ctx.agent_name.clone(),
                app_db: self.conn()?,
            }),
        ];

        let mut specs = self
            .tool_specs
            .write()
            .map_err(|e| anyhow::anyhow!("persona lock: {e}"))?;
        for tool in &tools {
            specs.push(ToolSpec {
                name: tool.name().to_owned(),
                description: tool.description().to_owned(),
                parameters_schema: tool.parameters_schema(),
            });
            ctx.tool_registry().register(Arc::clone(tool))?;
        }

        Ok(())
    }
}

struct SetPersonaTool {
    agent_name: String,
    app_db: Connection,
}

#[async_trait::async_trait]
impl Tool for SetPersonaTool {
    fn name(&self) -> &str {
        "set_persona"
    }
    fn description(&self) -> &str {
        "Set the agent's personality. This persona will persist across all future conversations."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "persona": { "type": "string", "description": "The personality instructions" }
            },
            "required": ["persona"]
        })
    }
    async fn call(&self, args: Value) -> ToolResult<Value> {
        let persona = args
            .get("persona")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("missing 'persona'".to_owned()))?;
        nekobot_core::entity::persona::upsert(&self.app_db, &self.agent_name, persona)
            .await
            .map_err(|e| ToolError::Execution(format!("upsert: {e}")))?;
        Ok(Value::String(format!("Persona set: \"{persona}\"")))
    }
}

struct GetPersonaTool {
    agent_name: String,
    app_db: Connection,
}

#[async_trait::async_trait]
impl Tool for GetPersonaTool {
    fn name(&self) -> &str {
        "get_persona"
    }
    fn description(&self) -> &str {
        "Get the current persona of this agent."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }
    async fn call(&self, _args: Value) -> ToolResult<Value> {
        let persona = nekobot_core::entity::persona::get(&self.app_db, &self.agent_name)
            .await
            .map_err(|e| ToolError::Execution(format!("get: {e}")))?;
        match persona {
            Some(p) => Ok(Value::String(p)),
            None => Ok(Value::String("No persona set.".to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persona_middleware_name() {
        let mw = PersonaMiddleware::new();
        assert_eq!(mw.name(), "persona");
    }
}
