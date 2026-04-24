use crate::agent::{Context, types::ChatRequest};

#[async_trait::async_trait]
pub trait Middleware: Send + Sync {
    async fn on_send(
        &mut self,
        ctx: Context,
        request: &mut ChatRequest,
    ) -> Result<(), anyhow::Error>;

    async fn on_receive(
        &mut self,
        ctx: Context,
        response: &mut crate::agent::types::ChatResponse,
    ) -> Result<(), anyhow::Error>;
}
