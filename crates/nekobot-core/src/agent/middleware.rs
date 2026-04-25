use crate::agent::{
    Context,
    types::{ChatRequest, ChatResponse},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MiddlewareEvent {
    /// Ask the owning agent runtime to start one agent interaction with this prompt.
    Activate(String),
}

impl MiddlewareEvent {
    pub fn activate(prompt: impl Into<String>) -> Self {
        Self::Activate(prompt.into())
    }
}

pub enum MiddlewareFlow {
    Continue,
    Respond(ChatResponse),
}

// Middleware hooks into the agent processing pipeline.
#[async_trait::async_trait]
pub trait Middleware: Send + Sync {
    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    async fn init(&self, _ctx: &Context) -> Result<(), anyhow::Error> {
        Ok(())
    }

    async fn before_chat(
        &self,
        _ctx: &Context,
        _request: &mut ChatRequest,
    ) -> Result<MiddlewareFlow, anyhow::Error> {
        Ok(MiddlewareFlow::Continue)
    }

    async fn after_chat(
        &self,
        _ctx: &Context,
        _response: &mut ChatResponse,
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }

    async fn on_error(&self, _ctx: &Context, _error: &anyhow::Error) -> Result<(), anyhow::Error> {
        Ok(())
    }
}
