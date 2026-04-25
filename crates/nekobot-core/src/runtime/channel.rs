use super::Runtime;
use nekobot_channel::Channel;
use turso::Connection;

use crate::agent::Agent;

pub struct ChannelRuntime {}
pub struct ChannelContext {
    pub(crate) app_db: Connection,
}

impl ChannelRuntime {
    pub fn new(channel: Box<dyn Channel>, context: ChannelContext, agent: Agent) -> Self {
        todo!()
    }
}

impl Runtime for ChannelRuntime {
    async fn run(&mut self) -> anyhow::Result<()> {
        todo!("Use the channel input/output to drive an agent loop")
    }
}
