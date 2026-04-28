mod channel;
mod types;

pub use types::{ChannelId, ChannelName, ChatId, ChatName, ReplyTarget, SenderId, SenderName};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    IncomingMessage {
        chat: ChatInfo,
        sender: SenderInfo,
        content: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    SendMessage {
        target: ReplyTarget,
        content: String,
    },
    StartTyping {
        target: ReplyTarget,
    },
    StopTyping {
        target: ReplyTarget,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelInfo {
    pub id: ChannelId,
    pub name: ChannelName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatInfo {
    pub id: ChatId,
    pub name: ChatName,
    pub reply_target: ReplyTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderInfo {
    pub id: SenderId,
    pub name: SenderName,
}

#[async_trait::async_trait]
pub trait Channel: Send + Sync {
    async fn register(
        &self,
        sender: tokio::sync::mpsc::Sender<Event>,
    ) -> anyhow::Result<ChannelInfo>;

    async fn send(&self, request: Request) -> anyhow::Result<()>;

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
