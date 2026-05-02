//! Chat message types shared across agents, providers, and middleware.

use std::fmt::Display;

use crate::agent::tool::ToolSpec;
use serde::{Deserialize, Serialize};

/// The role of a chat message participant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    /// Tool call result from tool execution.
    Tool,
    /// Provider-specific role name (e.g. "system", "developer").
    Custom(String),
}

impl Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::User => write!(f, "user"),
            Role::Assistant => write!(f, "assistant"),
            Role::Tool => write!(f, "tool"),
            Role::Custom(s) => write!(f, "{}", s),
        }
    }
}

/// A single tool call from an LLM response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub r#type: String,
    pub function: ToolCallFunction,
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

/// The content portion of a chat message, typed by role.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatMessageContent {
    /// A user message with optional inline images.
    User { text: String, images: Vec<Image> },
    /// An assistant response, optionally with reasoning and tool calls.
    Assistant {
        text: String,
        reasoning: Option<String>,
        tool_calls: Vec<ToolCall>,
    },
    /// A tool execution result.
    Tool {
        tool_call_id: String,
        result: String,
    },
}

impl ChatMessageContent {
    /// Returns the text body regardless of variant.
    pub fn text(&self) -> &str {
        match self {
            ChatMessageContent::User { text, .. }
            | ChatMessageContent::Assistant { text, .. }
            | ChatMessageContent::Tool { result: text, .. } => text,
        }
    }

    /// Returns the reasoning content, if this is an assistant message.
    pub fn reasoning(&self) -> Option<&str> {
        match self {
            ChatMessageContent::Assistant { reasoning, .. } => reasoning.as_deref(),
            _ => None,
        }
    }

    /// Returns the tool calls, if this is an assistant message.
    pub fn tool_calls(&self) -> &[ToolCall] {
        match self {
            ChatMessageContent::Assistant { tool_calls, .. } => tool_calls,
            _ => &[],
        }
    }

    /// Returns the tool call ID, if this is a tool result message.
    pub fn tool_call_id(&self) -> Option<&str> {
        match self {
            ChatMessageContent::Tool { tool_call_id, .. } => Some(tool_call_id),
            _ => None,
        }
    }

    /// Returns the inline images, if this is a user message.
    pub fn images(&self) -> &[Image] {
        match self {
            ChatMessageContent::User { images, .. } => images,
            _ => &[],
        }
    }
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
    /// Tool calls requested by the LLM.
    pub tool_calls: Vec<ToolCall>,
    pub images: Vec<Image>,
    pub usage: Option<Usage>,
}
