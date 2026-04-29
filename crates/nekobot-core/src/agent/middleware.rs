//! Middleware trait and activation types that hook into the agent processing pipeline.

use crate::agent::{
    Context,
    types::{ChatRequest, ChatResponse},
};

/// Events that middleware can emit during its lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MiddlewareEvent {
    /// Request the agent to process a given prompt as if the user sent it.
    Activate { prompt: String },
}

impl MiddlewareEvent {
    pub fn activate(prompt: impl Into<String>) -> Self {
        Self::Activate {
            prompt: prompt.into(),
        }
    }
}

/// Ways an agent session can be activated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentActivation {
    /// A real message from a channel user.
    ChannelMessage {
        chat_name: String,
        sender_name: String,
        content: String,
    },
    /// A synthetic activation from middleware.
    Middleware(MiddlewareEvent),
}

/// Controls the agent processing pipeline after a middleware hook.
pub enum MiddlewareFlow {
    /// Proceed normally to the next middleware or provider call.
    Continue,
    /// Short-circuit: return this response immediately, skipping the provider.
    Respond(ChatResponse),
}

/// Middleware hooks into the agent processing pipeline.
///
/// Middleware is called in registration order for `before_chat`, then in
/// reverse order for `after_chat`. Any middleware can short-circuit by
/// returning [`MiddlewareFlow::Respond`] from `before_chat`.
#[async_trait::async_trait]
pub trait Middleware: Send + Sync {
    /// Return a human-readable name for this middleware instance.
    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    /// Called once when the agent session starts. Can register tools or emit events.
    async fn init(&self, _ctx: &Context) -> Result<(), anyhow::Error> {
        Ok(())
    }

    /// Called before the chat request is sent to the provider.
    ///
    /// Can mutate the request (e.g. inject context) or short-circuit with
    /// [`MiddlewareFlow::Respond`].
    async fn before_chat(
        &self,
        _ctx: &Context,
        _request: &mut ChatRequest,
    ) -> Result<MiddlewareFlow, anyhow::Error> {
        Ok(MiddlewareFlow::Continue)
    }

    /// Called after the provider returns a response.
    ///
    /// Can mutate the response before it is sent back to the channel.
    async fn after_chat(
        &self,
        _ctx: &Context,
        _response: &mut ChatResponse,
    ) -> Result<(), anyhow::Error> {
        Ok(())
    }

    /// Called when the provider or a previous middleware hook returned an error.
    ///
    /// Called in reverse order for each middleware that had its `before_chat`
    /// already invoked.
    async fn on_error(&self, _ctx: &Context, _error: &anyhow::Error) -> Result<(), anyhow::Error> {
        Ok(())
    }
}
