use std::sync::Arc;

use tokio::sync::mpsc::Sender;

use crate::{
    agent::middleware::MiddlewareEvent,
    provider::{ModelOptions, Provider},
};

pub mod middleware;
pub mod tool;
pub mod types;

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
    pub(crate) middlewares: Vec<Arc<dyn middleware::Middleware>>,
    pub(crate) provider: Arc<dyn Provider>,
    pub(crate) model_options: ModelOptions,
}

pub enum AgentEvent {}

impl Agent {
    pub async fn interact(
        &self,
        ctx: Context,
        request: types::ChatRequest,
        event_sender: Option<std::sync::mpsc::Sender<AgentEvent>>,
    ) -> anyhow::Result<types::ChatResponse> {
        todo!()
    }
}
