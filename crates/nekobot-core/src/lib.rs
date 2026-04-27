pub mod agent;
pub mod config;
pub mod entity;
pub mod provider;
pub mod runtime;
pub mod session;

pub struct NekoBot<S = ()> {
    config: config::Config,
    state: S,
}

impl NekoBot<()> {
    pub fn new(config: config::Config) -> Self {
        Self { config, state: () }
    }
}

impl<S> NekoBot<S> {
    pub fn with_state<T>(self, state: T) -> NekoBot<T> {
        NekoBot {
            config: self.config,
            state,
        }
    }

    async fn init(&mut self) -> Result<(), anyhow::Error> {
        todo!("initialize db connections, agents, and runtimes")
    }

    pub async fn run(&mut self) -> Result<(), anyhow::Error> {
        todo!("assemble db connections, agents into runtimes, and run the hole system")
    }
}
