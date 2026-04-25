use std::sync::mpsc::Sender;
mod channel;
mod entity;
pub enum Event {
    IncomingMessage {
        channel: ChannelInfo,
        reply_target: String,
        content: String,
    },
}
pub enum Request {
    SendMessage { target: String, content: String },
    StartTyping { target: String },
    StopTyping { target: String },
}

pub struct ChannelInfo {
    pub name: &'static str,
}

#[async_trait::async_trait]
pub trait Channel: Send + Sync {
    async fn register(&self, sender: Sender<Event>) -> anyhow::Result<ChannelInfo>;
    async fn send(&self, request: Request) -> anyhow::Result<()>;
    async fn get_contact_list(&self, agent_id: i64) -> anyhow::Result<Vec<String>>;
}
