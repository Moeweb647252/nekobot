use std::sync::mpsc::Sender;

use crate::agent::types::{ChatRequest, ChatResponse};

#[derive(Clone, Debug, Default)]
pub struct ModelOptions {}

#[derive(Debug)]
pub enum ChatEvent {}

#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    async fn chat(
        &self,
        request: ChatRequest,
        option: ModelOptions,
        event_sender: Option<Sender<ChatEvent>>,
    ) -> Result<ChatResponse, anyhow::Error>;
}
