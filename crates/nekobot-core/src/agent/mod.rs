//! Agent session management, middleware pipeline, tool injection, and provider-based request handling.

use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

use anyhow::Context as _;
use tokio::sync::mpsc::{Receiver, Sender};
use turso::Connection;

use crate::{
    agent::{
        middleware::{AgentActivation, Middleware, MiddlewareEvent, MiddlewareFlow},
        tool::ToolRegistry,
        types::{ChatMessage, ChatMessageContent, ChatRequest, ChatResponse, Role},
    },
    config::MiddlewareConfig,
    entity::message::{Message, Role as MessageRole},
    provider::{ModelOptions, Provider, ProviderRequest},
    session::Session as SessionStore,
};

pub mod middleware;
pub mod tool;
pub mod types;

/// Immutable snapshot of an agent session's identity and channels, passed to middleware and tools.
#[derive(Clone)]
pub struct Context {
    /// Name of the agent that owns this session.
    pub agent_name: String,
    /// Persistent database identifier for the session.
    pub session_id: i64,
    /// Sender for middleware-triggered events (activations, etc.).
    pub event_sender: Sender<MiddlewareEvent>,
    /// Registry of runtime-registered tools available to this agent.
    pub tool_registry: Arc<ToolRegistry>,
}

impl Context {
    /// Creates a new context with the given agent name, session id, event sender, and tool registry.
    pub fn new(
        agent_name: impl Into<String>,
        session_id: i64,
        event_sender: Sender<MiddlewareEvent>,
        tool_registry: Arc<ToolRegistry>,
    ) -> Self {
        Self {
            agent_name: agent_name.into(),
            session_id,
            event_sender,
            tool_registry,
        }
    }

    /// Returns the agent name associated with this context.
    pub fn agent_name(&self) -> &str {
        &self.agent_name
    }

    /// Returns the persistent session identifier.
    pub fn session_id(&self) -> i64 {
        self.session_id
    }

    /// Returns a reference to the tool registry for runtime tool lookups.
    pub fn tool_registry(&self) -> &ToolRegistry {
        self.tool_registry.as_ref()
    }
}

/// Factory function type that creates a middleware instance from its serialized configuration.
pub type MiddlewareCreateFn =
    Arc<dyn Fn(&MiddlewareConfig) -> anyhow::Result<Arc<dyn Middleware>> + Send + Sync>;

/// Registry that maps middleware names to factory functions for dynamic instantiation.
#[derive(Clone, Default)]
pub struct MiddlewareRegistry {
    factories: HashMap<String, MiddlewareCreateFn>,
}

impl MiddlewareRegistry {
    /// Creates an empty middleware registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a factory function under the given name; returns an error on duplicates or empty names.
    pub fn register<F>(&mut self, name: impl Into<String>, create: F) -> anyhow::Result<()>
    where
        F: Fn(&MiddlewareConfig) -> anyhow::Result<Arc<dyn Middleware>> + Send + Sync + 'static,
    {
        let name = name.into();
        if name.trim().is_empty() {
            anyhow::bail!("middleware factory name cannot be empty");
        }

        if self.factories.contains_key(&name) {
            anyhow::bail!("duplicate middleware factory: {name}");
        }

        self.factories.insert(name, Arc::new(create));
        Ok(())
    }

    /// Looks up the factory for the given config's name and creates a middleware instance, returning `None` if not found.
    pub fn create(&self, config: &MiddlewareConfig) -> anyhow::Result<Option<Arc<dyn Middleware>>> {
        let Some(factory) = self.factories.get(&config.name) else {
            return Ok(None);
        };

        factory(config)
            .with_context(|| format!("failed to create middleware {}", config.name))
            .map(Some)
    }
}

/// Configuration bundle used to construct an `AgentSession`, holding the resolved provider, middleware list, and model options.
#[derive(Clone)]
pub struct AgentSessionConfig {
    pub agent_name: String,
    pub middlewares: Vec<Arc<dyn Middleware>>,
    pub provider: Arc<dyn Provider>,
    pub model_options: ModelOptions,
}

impl AgentSessionConfig {
    /// Builds a session config from a static `AgentConfig`, resolving middlewares through the registry.
    pub fn from_agent_config(
        agent: &crate::config::AgentConfig,
        provider: Arc<dyn Provider>,
        model_options: ModelOptions,
        middleware_registry: &MiddlewareRegistry,
    ) -> anyhow::Result<Self> {
        let middlewares = middlewares_from_config(&agent.middlewares, middleware_registry)?;
        Ok(Self::new(
            agent.name.clone(),
            provider,
            model_options,
            middlewares,
        ))
    }

    /// Creates a session config directly from its constituent parts.
    pub fn new(
        agent_name: impl Into<String>,
        provider: Arc<dyn Provider>,
        model_options: ModelOptions,
        middlewares: Vec<Arc<dyn Middleware>>,
    ) -> Self {
        Self {
            agent_name: agent_name.into(),
            middlewares,
            provider,
            model_options,
        }
    }
}

/// Resolves a slice of `MiddlewareConfig` entries into middleware instances via the registry, skipping unrecognized names.
pub fn middlewares_from_config(
    configs: &[MiddlewareConfig],
    middleware_registry: &MiddlewareRegistry,
) -> anyhow::Result<Vec<Arc<dyn Middleware>>> {
    let mut middlewares = Vec::with_capacity(configs.len());
    for config in configs {
        if let Some(middleware) = middleware_registry.create(config)? {
            middlewares.push(middleware);
        }
    }
    Ok(middlewares)
}

/// An agent session that orchestrates the middleware pipeline, provider calls, and event loop for a single conversation.
pub struct AgentSession {
    /// Persistent database identifier for this session.
    pub session_id: i64,
    /// Name of the agent that owns this session.
    pub agent_name: String,
    pub(crate) middlewares: Vec<Arc<dyn Middleware>>,
    pub(crate) provider: Arc<dyn Provider>,
    pub(crate) model_options: ModelOptions,
    pub(crate) tool_registry: Arc<ToolRegistry>,
}

impl AgentSession {
    /// Creates a new agent session from a session id and configuration.
    pub fn new(session_id: i64, config: AgentSessionConfig) -> Self {
        Self {
            session_id,
            agent_name: config.agent_name,
            middlewares: config.middlewares,
            provider: config.provider,
            model_options: config.model_options,
            tool_registry: Arc::new(ToolRegistry::new()),
        }
    }

    /// Builds a `Context` from this session, binding the given event sender.
    pub fn context(&self, event_sender: Sender<MiddlewareEvent>) -> Context {
        Context::new(
            self.agent_name.clone(),
            self.session_id,
            event_sender,
            Arc::clone(&self.tool_registry),
        )
    }

    /// Initializes middlewares and spawns the background event loop, returning a handle that can send activations.
    pub async fn start(
        self,
        app_db: Connection,
        output_sender: Sender<AgentOutput>,
    ) -> anyhow::Result<AgentSessionHandle> {
        let (activation_sender, activation_receiver) = tokio::sync::mpsc::channel(32);
        let (event_sender, event_receiver) = tokio::sync::mpsc::channel(32);
        let ctx = self.context(event_sender);

        self.init(&ctx).await?;

        let session_id = self.session_id;
        tokio::spawn(async move {
            self.run_loop(
                app_db,
                ctx,
                activation_receiver,
                event_receiver,
                output_sender,
            )
            .await;
        });

        Ok(AgentSessionHandle {
            session_id,
            activation_sender,
        })
    }

    /// Runs the `init` hook on every middleware in order.
    pub async fn init(&self, ctx: &Context) -> anyhow::Result<()> {
        for middleware in &self.middlewares {
            middleware.init(ctx).await?;
        }

        Ok(())
    }

    /// Runs the full middleware pipeline and, if not short-circuited, calls the provider to generate a response.
    pub async fn interact(
        &self,
        ctx: Context,
        mut request: ChatRequest,
    ) -> anyhow::Result<ChatResponse> {
        if let Err(error) = self.inject_registered_tool_specs(&ctx, &mut request) {
            self.run_error_hooks(&ctx, &error, 0).await;
            return Err(error);
        }

        for (index, middleware) in self.middlewares.iter().enumerate() {
            match middleware.before_chat(&ctx, &mut request).await {
                Ok(MiddlewareFlow::Continue) => {}
                Ok(MiddlewareFlow::Respond(mut response)) => {
                    if let Err(error) = self
                        .run_after_chat_hooks(&ctx, &mut response, index + 1)
                        .await
                    {
                        self.run_error_hooks(&ctx, &error, index + 1).await;
                        return Err(error);
                    }

                    return Ok(response);
                }
                Err(error) => {
                    self.run_error_hooks(&ctx, &error, index + 1).await;
                    return Err(error);
                }
            }
        }

        let provider_request = ProviderRequest {
            chat: request,
            options: self.model_options.clone(),
        };
        let mut response = match self.provider.complete(provider_request).await {
            Ok(response) => response,
            Err(error) => {
                let error = anyhow::Error::from(error);
                self.run_error_hooks(&ctx, &error, self.middlewares.len())
                    .await;
                return Err(error);
            }
        };

        if let Err(error) = self
            .run_after_chat_hooks(&ctx, &mut response, self.middlewares.len())
            .await
        {
            self.run_error_hooks(&ctx, &error, self.middlewares.len())
                .await;
            return Err(error);
        }

        Ok(response)
    }

    fn inject_registered_tool_specs(
        &self,
        ctx: &Context,
        request: &mut ChatRequest,
    ) -> anyhow::Result<()> {
        if !self.model_options.capabilities.tools {
            return Ok(());
        }

        let mut names = request
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<BTreeSet<_>>();

        for spec in ctx.tool_registry().tool_specs()? {
            if names.insert(spec.name.clone()) {
                request.tools.push(spec);
            }
        }

        Ok(())
    }

    async fn run_loop(
        self,
        app_db: Connection,
        ctx: Context,
        mut activation_receiver: Receiver<AgentActivation>,
        mut event_receiver: Receiver<MiddlewareEvent>,
        output_sender: Sender<AgentOutput>,
    ) {
        let mut activation_open = true;
        let mut event_open = true;

        while activation_open || event_open {
            tokio::select! {
                activation = activation_receiver.recv(), if activation_open => {
                    match activation {
                        Some(activation) => {
                            let _ = self
                                .handle_activation(&app_db, ctx.clone(), activation, &output_sender)
                                .await;
                        }
                        None => {
                            activation_open = false;
                        }
                    }
                }
                event = event_receiver.recv(), if event_open => {
                    match event {
                        Some(event) => {
                            let _ = self
                                .handle_activation(
                                    &app_db,
                                    ctx.clone(),
                                    AgentActivation::Middleware(event),
                                    &output_sender,
                                )
                                .await;
                        }
                        None => {
                            event_open = false;
                        }
                    }
                }
            }
        }
    }

    async fn handle_activation(
        &self,
        app_db: &Connection,
        ctx: Context,
        activation: AgentActivation,
        output_sender: &Sender<AgentOutput>,
    ) -> anyhow::Result<()> {
        let session = SessionStore {
            session_id: self.session_id,
            app_db: app_db.clone(),
        };

        let should_interact = match activation {
            AgentActivation::ChannelMessage { content, .. } => {
                session
                    .add_message(MessageRole::User.to_string(), content, None)
                    .await?;
                true
            }
            AgentActivation::Middleware(event) => {
                self.handle_middleware_event(&session, event).await?
            }
        };

        if !should_interact {
            return Ok(());
        }

        let request = self.build_chat_request(app_db).await?;
        let response = self.interact(ctx, request).await?;
        session
            .add_message(
                MessageRole::Assistant.to_string(),
                response.content.clone(),
                response.reasoning_content.clone(),
            )
            .await?;

        output_sender
            .send(AgentOutput::SendMessage {
                session_id: self.session_id,
                content: response.content,
            })
            .await?;

        Ok(())
    }

    async fn handle_middleware_event(
        &self,
        session: &SessionStore,
        event: MiddlewareEvent,
    ) -> anyhow::Result<bool> {
        match event {
            MiddlewareEvent::Activate { prompt } => {
                session
                    .add_message(
                        MessageRole::Custom("internal".to_owned()).to_string(),
                        prompt,
                        None,
                    )
                    .await?;
                Ok(true)
            }
        }
    }

    /// Loads persisted messages from the database and builds a `ChatRequest` for the current session.
    async fn build_chat_request(&self, app_db: &Connection) -> anyhow::Result<ChatRequest> {
        let messages = Message::list_by_session(app_db, self.session_id)
            .await?
            .into_iter()
            .map(|message| ChatMessage {
                role: chat_role(message.role),
                content: ChatMessageContent {
                    content: message.content,
                    reasoning_content: message.reasoning_content,
                    images: Vec::new(),
                },
            })
            .collect();

        Ok(ChatRequest {
            messages,
            system_prompt: None,
            tools: Vec::new(),
        })
    }

    async fn run_after_chat_hooks(
        &self,
        ctx: &Context,
        response: &mut ChatResponse,
        applied_middleware_count: usize,
    ) -> anyhow::Result<()> {
        for middleware in self.middlewares[..applied_middleware_count].iter().rev() {
            middleware.after_chat(ctx, response).await?;
        }

        Ok(())
    }

    async fn run_error_hooks(
        &self,
        ctx: &Context,
        error: &anyhow::Error,
        applied_middleware_count: usize,
    ) {
        for middleware in self.middlewares[..applied_middleware_count].iter().rev() {
            let _ = middleware.on_error(ctx, error).await;
        }
    }
}

/// A handle to a running agent session that can send activations (channel messages or middleware events).
#[derive(Clone)]
pub struct AgentSessionHandle {
    /// Persistent database identifier for the session.
    pub session_id: i64,
    /// Sender for triggering activations in the background event loop.
    pub activation_sender: Sender<AgentActivation>,
}

/// Output produced by an agent session, sent through the output channel to the application layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentOutput {
    /// Instructs the application to send a message to the chat with the given content.
    SendMessage { session_id: i64, content: String },
}

fn chat_role(role: String) -> Role {
    match role.as_str() {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        _ => Role::Custom(role),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };

    use serde_json::{Value, json};
    use turso::Builder;

    use crate::{
        agent::{
            middleware::{AgentActivation, Middleware, MiddlewareEvent, MiddlewareFlow},
            tool::{Tool, ToolRegistry, ToolResult, ToolSpec},
            types::ChatResponse,
        },
        entity::{Entity, message::Message, session::Session},
        provider::{ModelCapabilities, ProviderError},
    };

    use super::*;

    struct StaticProvider {
        called: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl Provider for StaticProvider {
        async fn complete(&self, _request: ProviderRequest) -> Result<ChatResponse, ProviderError> {
            self.called.store(true, Ordering::SeqCst);
            Ok(chat_response("provider"))
        }
    }

    struct ShortCircuitMiddleware;

    #[async_trait::async_trait]
    impl Middleware for ShortCircuitMiddleware {
        async fn before_chat(
            &self,
            _ctx: &Context,
            _request: &mut ChatRequest,
        ) -> Result<MiddlewareFlow, anyhow::Error> {
            Ok(MiddlewareFlow::Respond(chat_response("middleware")))
        }

        async fn after_chat(
            &self,
            _ctx: &Context,
            response: &mut ChatResponse,
        ) -> Result<(), anyhow::Error> {
            response.content.push_str("-after");
            Ok(())
        }
    }

    struct ActivateOnInitMiddleware;

    #[async_trait::async_trait]
    impl Middleware for ActivateOnInitMiddleware {
        async fn init(&self, ctx: &Context) -> Result<(), anyhow::Error> {
            ctx.event_sender
                .send(MiddlewareEvent::activate("wake up"))
                .await?;
            Ok(())
        }
    }

    struct RegisteredTool;

    #[async_trait::async_trait]
    impl Tool for RegisteredTool {
        fn name(&self) -> &'static str {
            "registered_tool"
        }

        fn description(&self) -> &'static str {
            "registered test tool"
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

    struct RegisterToolMiddleware;

    #[async_trait::async_trait]
    impl Middleware for RegisterToolMiddleware {
        async fn init(&self, ctx: &Context) -> Result<(), anyhow::Error> {
            ctx.tool_registry().register(Arc::new(RegisteredTool))
        }
    }

    struct ToolCapturingProvider {
        tools: Arc<Mutex<Option<Vec<ToolSpec>>>>,
    }

    #[async_trait::async_trait]
    impl Provider for ToolCapturingProvider {
        async fn complete(&self, request: ProviderRequest) -> Result<ChatResponse, ProviderError> {
            *self.tools.lock().unwrap() = Some(request.chat.tools);
            Ok(chat_response("provider"))
        }
    }

    async fn connection() -> anyhow::Result<(Connection, Session)> {
        let db = Builder::new_local(":memory:").build().await?;
        let conn = db.connect()?;
        Message::create_table(&conn).await?;
        let session = Session::create(&conn, "Neko").await?;
        Ok((conn, session))
    }

    #[test]
    fn agent_session_config_builds_empty_middlewares_from_config() -> anyhow::Result<()> {
        let provider_called = Arc::new(AtomicBool::new(false));
        let registry = MiddlewareRegistry::new();
        let agent_config = crate::config::AgentConfig {
            name: "Neko".to_owned(),
            provider: "deepseek".to_owned(),
            model: "deepseek-v4-pro".to_owned(),
            middlewares: Vec::new(),
        };

        let session_config = AgentSessionConfig::from_agent_config(
            &agent_config,
            Arc::new(StaticProvider {
                called: Arc::clone(&provider_called),
            }),
            ModelOptions::default(),
            &registry,
        )?;

        assert_eq!(session_config.agent_name, "Neko");
        assert!(session_config.middlewares.is_empty());
        assert!(!provider_called.load(Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn middlewares_from_config_skips_unknown_names() -> anyhow::Result<()> {
        let registry = MiddlewareRegistry::new();
        let config: crate::config::MiddlewareConfig = serde_json::from_value(json!({
            "name": "memory",
            "path": "./memory.db"
        }))?;

        let middlewares = middlewares_from_config(&[config], &registry)?;

        assert!(middlewares.is_empty());
        Ok(())
    }

    #[test]
    fn middlewares_from_config_uses_registered_factory() -> anyhow::Result<()> {
        let captured_config = Arc::new(Mutex::new(None));
        let mut registry = MiddlewareRegistry::new();
        let captured_config_for_factory = Arc::clone(&captured_config);
        registry.register("memory", move |config| {
            *captured_config_for_factory.lock().unwrap() = Some(config.clone());
            Ok(Arc::new(ShortCircuitMiddleware) as Arc<dyn Middleware>)
        })?;
        let config: crate::config::MiddlewareConfig = serde_json::from_value(json!({
            "name": "memory",
            "path": "./memory.db"
        }))?;

        let middlewares = middlewares_from_config(&[config.clone()], &registry)?;

        assert_eq!(middlewares.len(), 1);
        assert_eq!(*captured_config.lock().unwrap(), Some(config));
        Ok(())
    }

    #[test]
    fn agent_session_config_returns_factory_errors() {
        let mut registry = MiddlewareRegistry::new();
        registry
            .register("broken", |_config| anyhow::bail!("factory failed"))
            .unwrap();
        let agent_config = crate::config::AgentConfig {
            name: "Neko".to_owned(),
            provider: "deepseek".to_owned(),
            model: "deepseek-v4-pro".to_owned(),
            middlewares: vec![serde_json::from_value(json!({ "name": "broken" })).unwrap()],
        };

        let result = AgentSessionConfig::from_agent_config(
            &agent_config,
            Arc::new(StaticProvider {
                called: Arc::new(AtomicBool::new(false)),
            }),
            ModelOptions::default(),
            &registry,
        );

        let error = match result {
            Ok(_) => panic!("factory error should fail agent session config creation"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("failed to create middleware broken"));
    }

    #[test]
    fn middleware_registry_rejects_duplicate_names() {
        let mut registry = MiddlewareRegistry::new();
        registry
            .register("memory", |_config| {
                Ok(Arc::new(ShortCircuitMiddleware) as Arc<dyn Middleware>)
            })
            .unwrap();

        let result = registry.register("memory", |_config| {
            Ok(Arc::new(ShortCircuitMiddleware) as Arc<dyn Middleware>)
        });

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn middleware_can_short_circuit_provider() -> anyhow::Result<()> {
        let provider_called = Arc::new(AtomicBool::new(false));
        let (event_sender, _event_receiver) = tokio::sync::mpsc::channel(16);
        let tool_registry = Arc::new(ToolRegistry::new());
        let agent = AgentSession {
            agent_name: "Neko".to_owned(),
            session_id: 1,
            middlewares: vec![Arc::new(ShortCircuitMiddleware)],
            provider: Arc::new(StaticProvider {
                called: Arc::clone(&provider_called),
            }),
            model_options: ModelOptions::default(),
            tool_registry: Arc::clone(&tool_registry),
        };

        let response = agent
            .interact(
                Context::new("Neko", 1, event_sender, tool_registry),
                ChatRequest {
                    messages: Vec::new(),
                    system_prompt: None,
                    tools: Vec::new(),
                },
            )
            .await?;

        assert_eq!(response.content, "middleware-after");
        assert!(!provider_called.load(Ordering::SeqCst));
        Ok(())
    }

    #[tokio::test]
    async fn middleware_can_activate_agent_from_init() -> anyhow::Result<()> {
        let provider_called = Arc::new(AtomicBool::new(false));
        let (event_sender, mut event_receiver) = tokio::sync::mpsc::channel(16);
        let tool_registry = Arc::new(ToolRegistry::new());
        let agent = AgentSession {
            agent_name: "Neko".to_owned(),
            session_id: 1,
            middlewares: vec![Arc::new(ActivateOnInitMiddleware)],
            provider: Arc::new(StaticProvider {
                called: Arc::clone(&provider_called),
            }),
            model_options: ModelOptions::default(),
            tool_registry: Arc::clone(&tool_registry),
        };

        agent
            .init(&Context::new("Neko", 1, event_sender, tool_registry))
            .await?;

        assert_eq!(
            event_receiver.recv().await,
            Some(MiddlewareEvent::Activate {
                prompt: "wake up".to_owned(),
            })
        );
        assert!(!provider_called.load(Ordering::SeqCst));
        Ok(())
    }

    #[tokio::test]
    async fn agent_session_records_activation_and_response() -> anyhow::Result<()> {
        let (conn, session) = connection().await?;
        let provider_called = Arc::new(AtomicBool::new(false));
        let agent = AgentSession {
            agent_name: session.agent_name.clone(),
            session_id: session.id,
            middlewares: Vec::new(),
            provider: Arc::new(StaticProvider {
                called: Arc::clone(&provider_called),
            }),
            model_options: ModelOptions::default(),
            tool_registry: Arc::new(ToolRegistry::new()),
        };
        let (output_sender, mut output_receiver) = tokio::sync::mpsc::channel(16);
        let handle = agent.start(conn.clone(), output_sender).await?;

        handle
            .activation_sender
            .send(AgentActivation::ChannelMessage {
                chat_name: "Alice".to_owned(),
                sender_name: "Alice".to_owned(),
                content: "hello".to_owned(),
            })
            .await?;

        assert_eq!(
            output_receiver.recv().await,
            Some(AgentOutput::SendMessage {
                session_id: session.id,
                content: "provider".to_owned(),
            })
        );

        let messages = Message::list_by_session(&conn, session.id).await?;
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "hello");
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].content, "provider");
        assert!(provider_called.load(Ordering::SeqCst));

        Ok(())
    }

    #[tokio::test]
    async fn middleware_activation_records_internal_message_and_response() -> anyhow::Result<()> {
        let (conn, session) = connection().await?;
        let provider_called = Arc::new(AtomicBool::new(false));
        let agent = AgentSession {
            agent_name: session.agent_name.clone(),
            session_id: session.id,
            middlewares: vec![Arc::new(ActivateOnInitMiddleware)],
            provider: Arc::new(StaticProvider {
                called: Arc::clone(&provider_called),
            }),
            model_options: ModelOptions::default(),
            tool_registry: Arc::new(ToolRegistry::new()),
        };
        let (output_sender, mut output_receiver) = tokio::sync::mpsc::channel(16);
        let _handle = agent.start(conn.clone(), output_sender).await?;

        assert_eq!(
            output_receiver.recv().await,
            Some(AgentOutput::SendMessage {
                session_id: session.id,
                content: "provider".to_owned(),
            })
        );

        let messages = Message::list_by_session(&conn, session.id).await?;
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "internal");
        assert_eq!(messages[0].content, "wake up");
        assert_eq!(messages[1].role, "assistant");
        assert!(provider_called.load(Ordering::SeqCst));

        Ok(())
    }

    #[tokio::test]
    async fn registered_tools_are_injected_when_model_supports_tools() -> anyhow::Result<()> {
        let (event_sender, _event_receiver) = tokio::sync::mpsc::channel(16);
        let captured_tools = Arc::new(Mutex::new(None));
        let tool_registry = Arc::new(ToolRegistry::new());
        let agent = AgentSession {
            agent_name: "Neko".to_owned(),
            session_id: 1,
            middlewares: vec![Arc::new(RegisterToolMiddleware)],
            provider: Arc::new(ToolCapturingProvider {
                tools: Arc::clone(&captured_tools),
            }),
            model_options: ModelOptions {
                capabilities: ModelCapabilities {
                    tools: true,
                    ..ModelCapabilities::default()
                },
                ..ModelOptions::default()
            },
            tool_registry: Arc::clone(&tool_registry),
        };
        let ctx = Context::new("Neko", 1, event_sender, tool_registry);

        agent.init(&ctx).await?;
        agent.interact(ctx, ChatRequest::default()).await?;

        let tools = captured_tools
            .lock()
            .unwrap()
            .clone()
            .expect("provider should receive a request");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "registered_tool");
        assert_eq!(tools[0].description, "registered test tool");
        assert_eq!(tools[0].parameters_schema["type"], "object");
        Ok(())
    }

    #[tokio::test]
    async fn registered_tools_are_not_injected_when_model_lacks_tools() -> anyhow::Result<()> {
        let (event_sender, _event_receiver) = tokio::sync::mpsc::channel(16);
        let captured_tools = Arc::new(Mutex::new(None));
        let tool_registry = Arc::new(ToolRegistry::new());
        let agent = AgentSession {
            agent_name: "Neko".to_owned(),
            session_id: 1,
            middlewares: vec![Arc::new(RegisterToolMiddleware)],
            provider: Arc::new(ToolCapturingProvider {
                tools: Arc::clone(&captured_tools),
            }),
            model_options: ModelOptions::default(),
            tool_registry: Arc::clone(&tool_registry),
        };
        let ctx = Context::new("Neko", 1, event_sender, tool_registry);

        agent.init(&ctx).await?;
        agent.interact(ctx, ChatRequest::default()).await?;

        let tools = captured_tools
            .lock()
            .unwrap()
            .clone()
            .expect("provider should receive a request");
        assert!(tools.is_empty());
        Ok(())
    }

    struct CapturingProvider {
        capabilities: Arc<Mutex<Option<ModelCapabilities>>>,
    }

    #[async_trait::async_trait]
    impl Provider for CapturingProvider {
        async fn complete(&self, request: ProviderRequest) -> Result<ChatResponse, ProviderError> {
            *self.capabilities.lock().unwrap() = Some(request.options.capabilities);
            Ok(chat_response("provider"))
        }
    }

    #[tokio::test]
    async fn provider_request_carries_model_capabilities() -> anyhow::Result<()> {
        let (event_sender, _event_receiver) = tokio::sync::mpsc::channel(16);
        let capabilities = ModelCapabilities {
            streaming: true,
            tools: true,
            vision: false,
            reasoning: true,
        };
        let observed_capabilities = Arc::new(Mutex::new(None));
        let tool_registry = Arc::new(ToolRegistry::new());
        let agent = AgentSession {
            agent_name: "Neko".to_owned(),
            session_id: 1,
            middlewares: Vec::new(),
            provider: Arc::new(CapturingProvider {
                capabilities: Arc::clone(&observed_capabilities),
            }),
            model_options: ModelOptions {
                capabilities: capabilities.clone(),
                ..ModelOptions::default()
            },
            tool_registry: Arc::clone(&tool_registry),
        };

        agent
            .interact(
                Context::new("Neko", 1, event_sender, tool_registry),
                ChatRequest::default(),
            )
            .await?;

        assert_eq!(*observed_capabilities.lock().unwrap(), Some(capabilities));
        Ok(())
    }

    struct FailingProvider;

    #[async_trait::async_trait]
    impl Provider for FailingProvider {
        async fn complete(&self, _request: ProviderRequest) -> Result<ChatResponse, ProviderError> {
            Err(ProviderError::InvalidRequest("bad request".to_owned()))
        }
    }

    struct ErrorObserverMiddleware {
        called: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl Middleware for ErrorObserverMiddleware {
        async fn on_error(
            &self,
            _ctx: &Context,
            _error: &anyhow::Error,
        ) -> Result<(), anyhow::Error> {
            self.called.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn provider_error_triggers_middleware_error_hook() {
        let error_hook_called = Arc::new(AtomicBool::new(false));
        let (event_sender, _event_receiver) = tokio::sync::mpsc::channel(16);
        let tool_registry = Arc::new(ToolRegistry::new());
        let agent = AgentSession {
            agent_name: "Neko".to_owned(),
            session_id: 1,
            middlewares: vec![Arc::new(ErrorObserverMiddleware {
                called: Arc::clone(&error_hook_called),
            })],
            provider: Arc::new(FailingProvider),
            model_options: ModelOptions::default(),
            tool_registry: Arc::clone(&tool_registry),
        };

        let result = agent
            .interact(
                Context::new("Neko", 1, event_sender, tool_registry),
                ChatRequest::default(),
            )
            .await;

        assert!(result.is_err());
        assert!(error_hook_called.load(Ordering::SeqCst));
    }

    fn chat_response(content: impl Into<String>) -> ChatResponse {
        ChatResponse {
            content: content.into(),
            reasoning_content: None,
            images: Vec::new(),
            usage: None,
        }
    }
}
