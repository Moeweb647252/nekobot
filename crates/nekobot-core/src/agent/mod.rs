use std::sync::Arc;

use tokio::sync::mpsc::{Receiver, Sender};
use turso::Connection;

use crate::{
    agent::{
        middleware::{AgentActivation, Middleware, MiddlewareEvent, MiddlewareFlow},
        types::{ChatMessage, ChatMessageContent, ChatRequest, ChatResponse, Role},
    },
    entity::message::{Message, Role as MessageRole},
    provider::{ModelOptions, Provider, ProviderRequest},
    session::Session as SessionStore,
};

pub mod middleware;
pub mod tool;
pub mod types;

#[derive(Clone)]
pub struct Context {
    pub agent_id: i64,
    pub session_id: i64,
    pub event_sender: Sender<MiddlewareEvent>,
}

impl Context {
    pub fn agent_id(&self) -> i64 {
        self.agent_id
    }

    pub fn session_id(&self) -> i64 {
        self.session_id
    }
}

#[derive(Clone)]
pub struct AgentSessionConfig {
    pub agent_id: i64,
    pub middlewares: Vec<Arc<dyn Middleware>>,
    pub provider: Arc<dyn Provider>,
    pub model_options: ModelOptions,
}

impl AgentSessionConfig {
    pub fn new(
        agent_id: i64,
        provider: Arc<dyn Provider>,
        model_options: ModelOptions,
        middlewares: Vec<Arc<dyn Middleware>>,
    ) -> Self {
        Self {
            agent_id,
            middlewares,
            provider,
            model_options,
        }
    }
}

pub struct AgentSession {
    pub session_id: i64,
    pub agent_id: i64,
    pub(crate) middlewares: Vec<Arc<dyn Middleware>>,
    pub(crate) provider: Arc<dyn Provider>,
    pub(crate) model_options: ModelOptions,
}

impl AgentSession {
    pub fn new(session_id: i64, config: AgentSessionConfig) -> Self {
        Self {
            session_id,
            agent_id: config.agent_id,
            middlewares: config.middlewares,
            provider: config.provider,
            model_options: config.model_options,
        }
    }

    pub async fn start(
        self,
        app_db: Connection,
        output_sender: Sender<AgentOutput>,
    ) -> anyhow::Result<AgentSessionHandle> {
        let (activation_sender, activation_receiver) = tokio::sync::mpsc::channel(32);
        let (event_sender, event_receiver) = tokio::sync::mpsc::channel(32);
        let ctx = Context {
            agent_id: self.agent_id,
            session_id: self.session_id,
            event_sender,
        };

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

    pub async fn init(&self, ctx: &Context) -> anyhow::Result<()> {
        for middleware in &self.middlewares {
            middleware.init(ctx).await?;
        }

        Ok(())
    }

    pub async fn interact(
        &self,
        ctx: Context,
        mut request: ChatRequest,
    ) -> anyhow::Result<ChatResponse> {
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

#[derive(Clone)]
pub struct AgentSessionHandle {
    pub session_id: i64,
    pub activation_sender: Sender<AgentActivation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentOutput {
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

    use turso::Builder;

    use crate::{
        agent::{
            middleware::{AgentActivation, Middleware, MiddlewareEvent, MiddlewareFlow},
            types::ChatResponse,
        },
        entity::{Entity, agent::Agent, message::Message, session::Session},
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

    async fn connection() -> anyhow::Result<(Connection, Session)> {
        let db = Builder::new_local(":memory:").build().await?;
        let conn = db.connect()?;
        Message::create_table(&conn).await?;
        let agent = Agent::create(&conn, "Neko", "gpt-5.4").await?;
        let session = Session::create(&conn, agent.id).await?;
        Ok((conn, session))
    }

    #[tokio::test]
    async fn middleware_can_short_circuit_provider() -> anyhow::Result<()> {
        let provider_called = Arc::new(AtomicBool::new(false));
        let (event_sender, _event_receiver) = tokio::sync::mpsc::channel(16);
        let agent = AgentSession {
            agent_id: 1,
            session_id: 1,
            middlewares: vec![Arc::new(ShortCircuitMiddleware)],
            provider: Arc::new(StaticProvider {
                called: Arc::clone(&provider_called),
            }),
            model_options: ModelOptions::default(),
        };

        let response = agent
            .interact(
                Context {
                    agent_id: 1,
                    session_id: 1,
                    event_sender,
                },
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
        let agent = AgentSession {
            agent_id: 1,
            session_id: 1,
            middlewares: vec![Arc::new(ActivateOnInitMiddleware)],
            provider: Arc::new(StaticProvider {
                called: Arc::clone(&provider_called),
            }),
            model_options: ModelOptions::default(),
        };

        agent
            .init(&Context {
                agent_id: 1,
                session_id: 1,
                event_sender,
            })
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
            agent_id: session.agent_id,
            session_id: session.id,
            middlewares: Vec::new(),
            provider: Arc::new(StaticProvider {
                called: Arc::clone(&provider_called),
            }),
            model_options: ModelOptions::default(),
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
            agent_id: session.agent_id,
            session_id: session.id,
            middlewares: vec![Arc::new(ActivateOnInitMiddleware)],
            provider: Arc::new(StaticProvider {
                called: Arc::clone(&provider_called),
            }),
            model_options: ModelOptions::default(),
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
        let agent = AgentSession {
            agent_id: 1,
            session_id: 1,
            middlewares: Vec::new(),
            provider: Arc::new(CapturingProvider {
                capabilities: Arc::clone(&observed_capabilities),
            }),
            model_options: ModelOptions {
                capabilities: capabilities.clone(),
                ..ModelOptions::default()
            },
        };

        agent
            .interact(
                Context {
                    agent_id: 1,
                    session_id: 1,
                    event_sender,
                },
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
        let agent = AgentSession {
            agent_id: 1,
            session_id: 1,
            middlewares: vec![Arc::new(ErrorObserverMiddleware {
                called: Arc::clone(&error_hook_called),
            })],
            provider: Arc::new(FailingProvider),
            model_options: ModelOptions::default(),
        };

        let result = agent
            .interact(
                Context {
                    agent_id: 1,
                    session_id: 1,
                    event_sender,
                },
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
