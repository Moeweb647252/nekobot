use std::sync::Arc;

use tokio::sync::mpsc::Sender;

use crate::{
    agent::middleware::{Middleware, MiddlewareEvent, MiddlewareFlow},
    provider::{ModelOptions, Provider},
};

pub mod middleware;
pub mod tool;
pub mod types;

#[derive(Clone)]
pub struct Context {
    pub agent_id: String,
    pub event_sender: Sender<MiddlewareEvent>,
}

impl Context {
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }
}

pub struct Agent {
    pub(crate) id: String,
    pub(crate) middlewares: Vec<Arc<dyn Middleware>>,
    pub(crate) provider: Arc<dyn Provider>,
    pub(crate) model_options: ModelOptions,
}

pub enum AgentEvent {}

impl Agent {
    pub async fn init(&self, ctx: &Context) -> anyhow::Result<()> {
        for middleware in &self.middlewares {
            middleware.init(ctx).await?;
        }

        Ok(())
    }

    pub async fn interact(
        &self,
        ctx: Context,
        mut request: types::ChatRequest,
        _event_sender: Option<std::sync::mpsc::Sender<AgentEvent>>,
    ) -> anyhow::Result<types::ChatResponse> {
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

        let mut response = match self
            .provider
            .chat(request, self.model_options.clone(), None)
            .await
        {
            Ok(response) => response,
            Err(error) => {
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

    async fn run_after_chat_hooks(
        &self,
        ctx: &Context,
        response: &mut types::ChatResponse,
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

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use crate::agent::{
        middleware::{Middleware, MiddlewareEvent, MiddlewareFlow},
        types::ChatResponse,
    };

    use super::*;

    struct StaticProvider {
        called: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl Provider for StaticProvider {
        async fn chat(
            &self,
            _request: types::ChatRequest,
            _option: ModelOptions,
            _event_sender: Option<std::sync::mpsc::Sender<crate::provider::ChatEvent>>,
        ) -> Result<types::ChatResponse, anyhow::Error> {
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
            _request: &mut types::ChatRequest,
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

    #[tokio::test]
    async fn middleware_can_short_circuit_provider() {
        let provider_called = Arc::new(AtomicBool::new(false));
        let (event_sender, _event_receiver) = tokio::sync::mpsc::channel(16);
        let agent = Agent {
            id: "test-agent".to_owned(),
            middlewares: vec![Arc::new(ShortCircuitMiddleware)],
            provider: Arc::new(StaticProvider {
                called: Arc::clone(&provider_called),
            }),
            model_options: ModelOptions::default(),
        };

        let response = agent
            .interact(
                Context {
                    agent_id: "test-agent".to_owned(),
                    event_sender,
                },
                types::ChatRequest {
                    messages: Vec::new(),
                    system_prompt: String::new(),
                    tools: Vec::new(),
                },
                None,
            )
            .await
            .expect("middleware response should succeed");

        assert_eq!(response.content, "middleware-after");
        assert!(!provider_called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn middleware_can_activate_agent_from_init() {
        let provider_called = Arc::new(AtomicBool::new(false));
        let (event_sender, mut event_receiver) = tokio::sync::mpsc::channel(16);
        let agent = Agent {
            id: "test-agent".to_owned(),
            middlewares: vec![Arc::new(ActivateOnInitMiddleware)],
            provider: Arc::new(StaticProvider {
                called: Arc::clone(&provider_called),
            }),
            model_options: ModelOptions::default(),
        };

        agent
            .init(&Context {
                agent_id: "test-agent".to_owned(),
                event_sender,
            })
            .await
            .expect("middleware init should succeed");

        assert_eq!(
            event_receiver.recv().await,
            Some(MiddlewareEvent::Activate("wake up".to_owned()))
        );
        assert!(!provider_called.load(Ordering::SeqCst));
    }

    fn chat_response(content: impl Into<String>) -> types::ChatResponse {
        types::ChatResponse {
            content: content.into(),
            reasoning_content: None,
            images: Vec::new(),
            usage: None,
        }
    }
}
