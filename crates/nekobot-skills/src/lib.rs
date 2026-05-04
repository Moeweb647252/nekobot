//! Agent Skills middleware — implements the [Agent Skills](https://agentskills.io) spec.
//!
//! Scans configured directories for `SKILL.md` files and provides progressive
//! disclosure: catalog (name+desc) → activation (full instructions) → resources (on demand).

mod loader;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use nekobot_core::agent::{
    Context,
    middleware::{Middleware, MiddlewareFlow},
    tool::{Tool, ToolResult, ToolSpec},
    types::ChatRequest,
};
use serde::Deserialize;
use serde_json::Value;

use loader::SkillMeta;

/// Deserialized from `MiddlewareConfig.data`.
#[derive(Debug, Clone, Deserialize)]
pub struct SkillConfig {
    /// Path to the skills directory. Default `"./skills"`.
    #[serde(default = "default_skills_dir")]
    pub skills_dir: String,
}

fn default_skills_dir() -> String {
    "./skills".to_owned()
}

/// Middleware that discovers and manages Agent Skills.
pub struct SkillMiddleware {
    /// All discovered skills (name + description).
    catalog: Vec<SkillMeta>,
    /// Root directory for skill loading.
    root_dir: PathBuf,
    /// Skills activated during this session (shared with UseSkillTool).
    activated: Arc<RwLock<HashSet<String>>>,
    /// Cached tool specs for injection into ChatRequest.
    tool_specs: RwLock<Vec<ToolSpec>>,
}

impl SkillMiddleware {
    /// Create a new SkillMiddleware by scanning `config.skills_dir`.
    pub fn from_config(config: SkillConfig) -> anyhow::Result<Self> {
        let root_dir = PathBuf::from(&config.skills_dir);
        let catalog = loader::discover(&root_dir)?;
        tracing::info!(
            target: "skill",
            "discovered {} skills in {}",
            catalog.len(),
            root_dir.display()
        );
        Ok(Self {
            catalog,
            root_dir,
            activated: Arc::new(RwLock::new(HashSet::new())),
            tool_specs: RwLock::new(Vec::new()),
        })
    }

    fn build_system_prompt(&self) -> String {
        let mut prompt = String::new();

        if !self.catalog.is_empty() {
            prompt.push_str("Available skills:\n");
            for skill in &self.catalog {
                prompt.push_str(&format!("- {}: {}\n", skill.name, skill.description));
            }
            prompt.push_str(
                "\nTo activate a skill, call the `use_skill` tool with the skill name.\n",
            );
        }

        // Append activated skill bodies
        let activated = self.activated.read().unwrap_or_else(|e| e.into_inner());
        for name in activated.iter() {
            if let Some(skill) = self.catalog.iter().find(|s| &s.name == name) {
                match loader::load(&skill.location) {
                    Ok(skill) => {
                        prompt.push_str(&format!(
                            "\n\n--- Activated skill: {} ---\n\n{}\n\nSkill directory: {}\n",
                            name,
                            skill.body,
                            skill.base_dir.display(),
                        ));
                    }
                    Err(e) => {
                        tracing::warn!(target: "skill", "failed to load skill {name}: {e}");
                    }
                }
            }
        }

        prompt
    }
}

#[async_trait::async_trait]
impl Middleware for SkillMiddleware {
    fn name(&self) -> &'static str {
        "skills"
    }

    async fn before_chat(
        &self,
        _ctx: &Context,
        request: &mut ChatRequest,
    ) -> Result<MiddlewareFlow, anyhow::Error> {
        let prompt = self.build_system_prompt();
        if !prompt.is_empty() {
            let existing = request.system_prompt.take().unwrap_or_default();
            request.system_prompt = Some(if existing.is_empty() { prompt } else { format!("{existing}\n\n{prompt}") });
        }
        let specs = self
            .tool_specs
            .read()
            .map_err(|e| anyhow::anyhow!("skill tool_specs lock poisoned: {e}"))?;
        request.tools.extend(specs.iter().cloned());
        Ok(MiddlewareFlow::Continue)
    }

    async fn init(&self, ctx: &Context) -> Result<(), anyhow::Error> {
        if self.catalog.is_empty() {
            return Ok(());
        }

        let skill_names: Vec<String> = self.catalog.iter().map(|s| s.name.clone()).collect();

        let use_tool = Arc::new(UseSkillTool {
            names: skill_names.clone(),
            activated: Arc::clone(&self.activated),
        });
        let deactivate_tool = Arc::new(DeactivateSkillTool {
            names: skill_names,
            activated: Arc::clone(&self.activated),
        });

        let mut specs = self
            .tool_specs
            .write()
            .map_err(|e| anyhow::anyhow!("skill tool_specs lock poisoned: {e}"))?;
        for tool in [
            use_tool.as_ref() as &dyn Tool,
            deactivate_tool.as_ref() as &dyn Tool,
        ] {
            specs.push(ToolSpec {
                name: tool.name().to_owned(),
                description: tool.description().to_owned(),
                parameters_schema: tool.parameters_schema(),
            });
        }
        drop(specs);

        ctx.tool_registry().register(use_tool)?;
        ctx.tool_registry().register(deactivate_tool)?;
        Ok(())
    }
}

/// Tool that activates a skill by name.
struct UseSkillTool {
    names: Vec<String>,
    activated: Arc<RwLock<HashSet<String>>>,
}

#[async_trait::async_trait]
impl Tool for UseSkillTool {
    fn name(&self) -> &str {
        "use_skill"
    }

    fn description(&self) -> &str {
        "Activate an available skill by name. After activation, the skill's full instructions \
         will be included in future requests. Call this when a task matches a skill's description."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The name of the skill to activate",
                    "enum": self.names,
                }
            },
            "required": ["name"]
        })
    }

    async fn call(&self, args: Value) -> ToolResult<Value> {
        let name = args.get("name").and_then(Value::as_str).ok_or_else(|| {
            nekobot_core::agent::tool::ToolError::InvalidArguments(
                "missing 'name' parameter".to_owned(),
            )
        })?;

        if !self.names.iter().any(|n| n == name) {
            return Err(nekobot_core::agent::tool::ToolError::Execution(format!(
                "unknown skill: {name}"
            )));
        }

        let mut activated = self.activated.write().map_err(|e| {
            nekobot_core::agent::tool::ToolError::Execution(format!("lock poisoned: {e}"))
        })?;

        if activated.contains(name) {
            return Ok(Value::String(format!("Skill '{name}' is already active.")));
        }

        activated.insert(name.to_owned());
        Ok(Value::String(format!(
            "Skill '{name}' activated. Its instructions will be included in future requests."
        )))
    }
}

/// Tool that deactivates a previously activated skill.
struct DeactivateSkillTool {
    names: Vec<String>,
    activated: Arc<RwLock<HashSet<String>>>,
}

#[async_trait::async_trait]
impl Tool for DeactivateSkillTool {
    fn name(&self) -> &str {
        "deactivate_skill"
    }

    fn description(&self) -> &str {
        "Deactivate a previously activated skill. Its instructions will be removed from future requests."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The name of the skill to deactivate",
                    "enum": self.names,
                }
            },
            "required": ["name"]
        })
    }

    async fn call(&self, args: Value) -> ToolResult<Value> {
        let name = args.get("name").and_then(Value::as_str).ok_or_else(|| {
            nekobot_core::agent::tool::ToolError::InvalidArguments(
                "missing 'name' parameter".to_owned(),
            )
        })?;

        let mut activated = self.activated.write().map_err(|e| {
            nekobot_core::agent::tool::ToolError::Execution(format!("lock poisoned: {e}"))
        })?;

        if activated.remove(name) {
            Ok(Value::String(format!("Skill '{name}' deactivated.")))
        } else {
            Ok(Value::String(format!("Skill '{name}' was not active.")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(dir: &std::path::Path, name: &str, description: &str, body: &str) {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        let content = format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}\n");
        std::fs::write(skill_dir.join("SKILL.md"), content).unwrap();
    }

    fn make_tool(names: Vec<String>) -> UseSkillTool {
        UseSkillTool {
            names,
            activated: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    #[test]
    fn middleware_builds_system_prompt_with_catalog() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "greeter",
            "Say hello",
            "# Greetings\nBe friendly.",
        );
        write_skill(
            dir.path(),
            "analyzer",
            "Analyze data",
            "# Analysis\nUse statistics.",
        );

        let mw = SkillMiddleware::from_config(SkillConfig {
            skills_dir: dir.path().to_string_lossy().into(),
        })
        .unwrap();

        let prompt = mw.build_system_prompt();
        assert!(prompt.contains("greeter"));
        assert!(prompt.contains("Say hello"));
        assert!(prompt.contains("analyzer"));
        assert!(prompt.contains("Analyze data"));
        assert!(prompt.contains("use_skill"));
        assert!(!prompt.contains("Be friendly"));
    }

    #[test]
    fn empty_catalog_produces_empty_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let mw = SkillMiddleware::from_config(SkillConfig {
            skills_dir: dir.path().to_string_lossy().into(),
        })
        .unwrap();
        assert!(mw.build_system_prompt().is_empty());
    }

    #[tokio::test]
    async fn use_skill_tool_activates_and_deduplicates() {
        let activated = Arc::new(RwLock::new(HashSet::new()));
        let tool = UseSkillTool {
            names: vec!["test-skill".to_owned()],
            activated: Arc::clone(&activated),
        };

        let result = tool
            .call(serde_json::json!({"name": "test-skill"}))
            .await
            .unwrap();
        assert!(result.as_str().unwrap().contains("activated"));

        let result = tool
            .call(serde_json::json!({"name": "test-skill"}))
            .await
            .unwrap();
        assert!(result.as_str().unwrap().contains("already active"));
    }

    #[tokio::test]
    async fn use_skill_tool_rejects_unknown() {
        let tool = make_tool(vec!["known".to_owned()]);
        let err = tool
            .call(serde_json::json!({"name": "unknown"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unknown"));
    }
}
