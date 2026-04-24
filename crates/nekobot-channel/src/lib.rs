use std::sync::mpsc::Sender;

pub enum Event {
    IncomingMessage { channel: String },
}
pub enum Request {}

pub struct ChannelInfo {
    pub name: &'static str,
}

#[async_trait::async_trait]
pub trait Channel: Send + Sync {
    async fn register(&self, sender: Sender<Event>) -> anyhow::Result<ChannelInfo>;
    async fn send(&self, request: Request) -> anyhow::Result<()>;
}
