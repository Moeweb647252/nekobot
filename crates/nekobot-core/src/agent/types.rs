use std::fmt::Display;

use crate::agent::tool::ToolSpec;

#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub data: Vec<u8>,
    pub mime_type: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatMessageContent {
    pub content: String,
    pub reasoning_content: Option<String>,
    pub images: Vec<Image>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: Role,
    pub content: ChatMessageContent,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub system_prompt: Option<String>,
    pub tools: Vec<ToolSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatResponse {
    pub content: String,
    pub reasoning_content: Option<String>,
    pub images: Vec<Image>,
    pub usage: Option<Usage>,
}
