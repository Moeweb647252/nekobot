use std::sync::mpsc::Sender;

use nekobot_core::agent::types::{ChatRequest, ChatResponse};

pub struct ModelOptions {}

pub enum ChatEvent {}

#[async_trait::async_trait]
pub trait Provider {
    async fn chat(
        &self,
        request: ChatRequest,
        option: ModelOptions,
        event_sender: Sender<ChatEvent>,
    ) -> Result<ChatResponse, anyhow::Error>;
}
