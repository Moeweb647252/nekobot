pub mod agent;
pub mod config;
pub mod entity;
pub mod provider;
pub mod runtime;
pub mod session;

pub struct NekoBot<S = ()> {
    config: config::Config,
    state: S,
    middleware_registry: agent::MiddlewareRegistry,
    provider_registry: provider::ProviderRegistry,
}

impl NekoBot<()> {
    pub fn new(config: config::Config) -> Self {
        Self {
            config,
            state: (),
            middleware_registry: agent::MiddlewareRegistry::new(),
            provider_registry: provider::ProviderRegistry::new(),
        }
    }
}

impl<S> NekoBot<S> {
    pub fn with_middleware<F>(mut self, name: impl Into<String>, create: F) -> anyhow::Result<Self>
    where
        F: Fn(
                &config::MiddlewareConfig,
            ) -> anyhow::Result<std::sync::Arc<dyn agent::middleware::Middleware>>
            + Send
            + Sync
            + 'static,
    {
        self.middleware_registry.register(name, create)?;
        Ok(self)
    }

    pub fn with_provider<F>(mut self, name: impl Into<String>, create: F) -> anyhow::Result<Self>
    where
        F: Fn(&config::ProviderConfig) -> anyhow::Result<std::sync::Arc<dyn provider::Provider>>
            + Send
            + Sync
            + 'static,
    {
        self.provider_registry.register(name, create)?;
        Ok(self)
    }

    pub fn with_state<T>(self, state: T) -> NekoBot<T> {
        NekoBot {
            config: self.config,
            state,
            middleware_registry: self.middleware_registry,
            provider_registry: self.provider_registry,
        }
    }

    pub fn middleware_registry(&self) -> &agent::MiddlewareRegistry {
        &self.middleware_registry
    }

    pub fn provider_registry(&self) -> &provider::ProviderRegistry {
        &self.provider_registry
    }

    pub fn provider_registry_mut(&mut self) -> &mut provider::ProviderRegistry {
        &mut self.provider_registry
    }

    async fn init(&mut self) -> Result<(), anyhow::Error> {
        todo!("initialize db connections, agents, and runtimes")
    }

    pub async fn run(&mut self) -> Result<(), anyhow::Error> {
        todo!("assemble db connections, agents into runtimes, and run the hole system")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{
        NekoBot,
        agent::{self},
        config::{self, MiddlewareConfig},
    };

    struct TestMiddleware;

    #[async_trait::async_trait]
    impl agent::middleware::Middleware for TestMiddleware {}

    #[test]
    fn with_middleware_registry_survives_with_state() -> anyhow::Result<()> {
        let bot = NekoBot::new(config::Config {
            channels: Vec::new(),
            providers: Vec::new(),
            agents: Vec::new(),
        })
        .with_middleware("test", |_config| {
            Ok(Arc::new(TestMiddleware) as Arc<dyn agent::middleware::Middleware>)
        })?
        .with_state("state");
        let config = MiddlewareConfig {
            name: "test".to_owned(),
            data: serde_json::Map::new(),
        };

        let middlewares = agent::middlewares_from_config(&[config], bot.middleware_registry())?;

        assert_eq!(middlewares.len(), 1);
        Ok(())
    }
}
