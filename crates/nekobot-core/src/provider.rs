use std::sync::mpsc::Sender;

use crate::agent::types::{ChatRequest, ChatResponse};

pub struct ModelOptions {}

pub enum ChatEvent {}

#[async_trait::async_trait]
pub trait Provider {
    async fn chat(
        &self,
        request: ChatRequest,
        option: ModelOptions,
        event_sender: Option<Sender<ChatEvent>>,
    ) -> Result<ChatResponse, anyhow::Error>;
}
