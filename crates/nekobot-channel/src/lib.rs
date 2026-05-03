//! Channel abstraction layer for communication backends (QQ, Discord, etc.).
//!
//! Defines the [`Channel`] trait that all channel adapters implement, plus the
//! domain types for events, requests, and entity identifiers.

pub mod channel;
pub mod entity;
mod types;

pub use types::{ChannelId, ChannelName, ChatId, ChatName, ReplyTarget, SenderId, SenderName};

/// Inbound event from a channel, forwarded to the agent runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A user sent a message in a chat.
    IncomingMessage {
        chat: ChatInfo,
        sender: SenderInfo,
        content: String,
    },
}

/// Outbound request from the agent runtime to a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Send a text message to the given target.
    SendMessage {
        target: ReplyTarget,
        content: String,
    },
    /// Show a typing indicator in the given target.
    StartTyping { target: ReplyTarget },
    /// Stop the typing indicator in the given target.
    StopTyping { target: ReplyTarget },
}

/// Metadata about a registered channel, returned by [`Channel::register`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelInfo {
    pub id: ChannelId,
    pub name: ChannelName,
}

/// Identifies a conversation within a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatInfo {
    pub id: ChatId,
    pub name: ChatName,
    /// Opaque token used by [`Request::SendMessage`] to route replies back to the right conversation.
    pub reply_target: ReplyTarget,
}

/// Identifies the sender of a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderInfo {
    pub id: SenderId,
    pub name: SenderName,
}

/// Trait that every channel adapter must implement.
///
/// A channel is a communication backend — QQ, Discord, Telegram, etc.
/// It receives events from the platform and sends requests back.
#[async_trait::async_trait]
pub trait Channel: Send + Sync {
    /// Register this channel with an event sender.
    ///
    /// The channel starts listening for inbound events (WebSocket, polling, etc.)
    /// and forwards them through `sender`. Returns static metadata about the channel.
    async fn register(
        &self,
        sender: tokio::sync::mpsc::Sender<Event>,
        app_db: Option<turso::Connection>,
    ) -> anyhow::Result<ChannelInfo>;

    /// Send a request (message, typing indicator, etc.) to the platform.
    async fn send(&self, request: Request) -> anyhow::Result<()>;

    /// List active conversations in this channel.
    ///
    /// Returns an empty list by default; platforms that support chat discovery
    /// should override this.
    async fn list_chats(&self) -> anyhow::Result<Vec<ChatInfo>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_models_use_domain_types() {
        let chat = ChatInfo {
            id: ChatId::from("chat-1"),
            name: ChatName::from("Alice"),
            reply_target: ReplyTarget::from("target-1"),
        };
        let request = Request::SendMessage {
            target: chat.reply_target.clone(),
            content: "hello".to_owned(),
        };

        assert_eq!(chat.id.as_str(), "chat-1");
        assert_eq!(chat.name.as_str(), "Alice");
        assert_eq!(
            request,
            Request::SendMessage {
                target: ReplyTarget::from("target-1"),
                content: "hello".to_owned(),
            }
        );
    }
}
