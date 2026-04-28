mod channel;

pub mod entity;

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
    SendMessage { target: String, content: String },
    StartTyping { target: String },
    StopTyping { target: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelInfo {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatInfo {
    pub name: String,
    pub reply_target: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderInfo {
    pub name: String,
}

#[async_trait::async_trait]
pub trait Channel: Send + Sync {
    async fn register(
        &self,
        sender: tokio::sync::mpsc::Sender<Event>,
    ) -> anyhow::Result<ChannelInfo>;

    async fn send(&self, request: Request) -> anyhow::Result<()>;

    async fn get_contact_list(&self, agent_name: &str) -> anyhow::Result<Vec<String>>;
}
