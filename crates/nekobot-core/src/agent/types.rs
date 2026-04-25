use core::str;
use std::{fmt::Display, sync::Arc};

use crate::agent::tool;

pub enum Role {
    User,
    Assistant,
    Custom(String),
}

impl Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::User => write!(f, "user"),
            Role::Assistant => write!(f, "assistant"),
            Role::Custom(s) => write!(f, "{}", s),
        }
    }
}

pub struct Image {
    data: Vec<u8>,
    mime_type: String,
}

pub struct Usage {}

pub struct ChatMessageContent {
    pub content: String,
    pub reasoning_content: Option<String>,
    pub images: Vec<Image>,
}

pub struct ChatMessage {
    pub role: Role,
    pub content: ChatMessageContent,
}

pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub system_prompt: String,
    pub tools: Vec<Arc<dyn tool::Tool>>,
}

pub struct ChatResponse {
    pub content: String,
    pub reasoning_content: Option<String>,
    pub images: Vec<Image>,
    pub usage: Option<Usage>,
}
