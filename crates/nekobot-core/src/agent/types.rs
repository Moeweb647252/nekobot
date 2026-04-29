//! Chat message types shared across agents, providers, and middleware.

use std::fmt::Display;

use crate::agent::tool::ToolSpec;

/// The role of a chat message participant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    /// Provider-specific role name (e.g. "system", "developer").
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

/// An image attachment with inline binary data and MIME type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub data: Vec<u8>,
    pub mime_type: String,
}

/// Token usage statistics returned by a provider.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

/// The content portion of a chat message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatMessageContent {
    pub content: String,
    pub reasoning_content: Option<String>,
    pub images: Vec<Image>,
}

/// A single message in a chat conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: Role,
    pub content: ChatMessageContent,
}

/// A complete chat completion request sent to a provider.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub system_prompt: Option<String>,
    pub tools: Vec<ToolSpec>,
}

/// A chat completion response from a provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatResponse {
    pub content: String,
    pub reasoning_content: Option<String>,
    pub images: Vec<Image>,
    pub usage: Option<Usage>,
}
