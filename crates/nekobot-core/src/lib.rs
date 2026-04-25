pub mod agent;
pub mod config;
pub mod entity;
pub mod provider;
pub mod runtime;
pub mod session;

pub struct NekoBot<S> {
    state: S,
}

impl<S> NekoBot<S> {
    pub fn new(state: S) -> Self {
        Self { state }
    }

    pub async fn run(&mut self) -> Result<(), anyhow::Error> {
        todo!("assemble db connections, agents into runtimes, and run the hole system")
    }
}
