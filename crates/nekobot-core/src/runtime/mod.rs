pub mod channel;
pub mod shell;

pub trait Runtime {
    async fn run(&mut self) -> Result<(), anyhow::Error>;
}
