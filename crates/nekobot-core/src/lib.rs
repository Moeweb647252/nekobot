//! NekoBot — a modular, multi-agent chatbot framework.
//!
//! The [`NekoBot`] struct is the top-level application entry point. It holds
//! configuration, middleware/provider/channel registries, and optional
//! user-defined state, then wires everything together via [`run`](NekoBot::run).

use nekobot_channel::Channel;

pub mod agent;
pub mod channel_registry;
pub mod config;
pub mod entity;
pub mod provider;
pub mod registry;
pub mod runtime;
pub mod session;

/// Top-level application struct.
///
/// Created via [`NekoBot::new`] with a [`config::Config`], then configured
/// with middleware, provider, and channel registrations before calling
/// [`run`](NekoBot::run).
///
/// The type parameter `S` allows injecting user-defined shared state
/// (e.g. database pools, external service handles) via [`with_state`](NekoBot::with_state).
pub struct NekoBot<S = ()> {
    config: config::Config,
    state: S,
    middleware_registry: agent::MiddlewareRegistry,
    provider_registry: provider::ProviderRegistry,
    channel_registry: channel_registry::ChannelRegistry,
}

impl NekoBot<()> {
    /// Create a new [`NekoBot`] from a parsed [`config::Config`].
    pub fn new(config: config::Config) -> Self {
        Self {
            config,
            state: (),
            middleware_registry: agent::MiddlewareRegistry::new(),
            provider_registry: provider::ProviderRegistry::new(),
            channel_registry: channel_registry::ChannelRegistry::new(),
        }
    }
}

impl<S> NekoBot<S> {
    /// Register a middleware factory by name.
    ///
    /// The factory closure receives a [`config::MiddlewareConfig`] and should
    /// return an `Arc<dyn Middleware>`.
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

    /// Register a provider factory by name.
    ///
    /// The factory closure receives a [`config::ProviderConfig`] and should
    /// return an `Arc<dyn Provider>`.
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

    /// Register a channel factory by name.
    ///
    /// The factory closure receives a [`config::ChannelConfig`] and should
    /// return a `Box<dyn Channel>`.
    pub fn with_channel<F>(mut self, name: impl Into<String>, create: F) -> anyhow::Result<Self>
    where
        F: Fn(&config::ChannelConfig) -> anyhow::Result<Box<dyn Channel>>
            + Send
            + Sync
            + 'static,
    {
        self.channel_registry.register(name, create)?;
        Ok(self)
    }

    /// Replace the user-defined state with a new value of a different type.
    ///
    /// This consumes the current `NekoBot<S>` and returns `NekoBot<T>`,
    /// preserving config and registries.
    pub fn with_state<T>(self, state: T) -> NekoBot<T> {
        NekoBot {
            config: self.config,
            state,
            middleware_registry: self.middleware_registry,
            provider_registry: self.provider_registry,
            channel_registry: self.channel_registry,
        }
    }

    /// Return a shared reference to the middleware registry.
    pub fn middleware_registry(&self) -> &agent::MiddlewareRegistry {
        &self.middleware_registry
    }

    /// Return a mutable reference to the middleware registry.
    pub fn middleware_registry_mut(&mut self) -> &mut agent::MiddlewareRegistry {
        &mut self.middleware_registry
    }

    /// Return a shared reference to the provider registry.
    pub fn provider_registry(&self) -> &provider::ProviderRegistry {
        &self.provider_registry
    }

    /// Return a mutable reference to the provider registry.
    pub fn provider_registry_mut(&mut self) -> &mut provider::ProviderRegistry {
        &mut self.provider_registry
    }

    /// Return a shared reference to the channel registry.
    pub fn channel_registry(&self) -> &channel_registry::ChannelRegistry {
        &self.channel_registry
    }

    /// Return a mutable reference to the channel registry.
    pub fn channel_registry_mut(&mut self) -> &mut channel_registry::ChannelRegistry {
        &mut self.channel_registry
    }

    async fn init(
        &self,
    ) -> Result<Vec<crate::runtime::channel::ChannelRuntime>, anyhow::Error> {
        self.config.validate()?;

        let conn = self.init_database().await?;
        let providers = self.init_providers()?;
        let channels = self.init_channels()?;
        let agent_configs = self.build_agent_configs(&providers)?;
        let gate = self.build_gate(&conn);

        Ok(self.build_runtimes(channels, conn, agent_configs, gate))
    }

    async fn init_database(&self) -> Result<turso::Connection, anyhow::Error> {
        use crate::entity::{
            Entity,
            channel_chat_agent::ChannelChatAgent,
            message::Message,
            sender_gate_state::SenderGateState,
            session::Session,
        };

        let db = turso::Builder::new_local(&self.config.database_path)
            .build()
            .await?;
        let conn = db.connect()?;
        crate::entity::enable_foreign_keys(&conn).await?;
        Session::create_table(&conn).await?;
        Message::create_table(&conn).await?;
        ChannelChatAgent::create_table(&conn).await?;
        SenderGateState::create_table(&conn).await?;
        Ok(conn)
    }

    fn init_providers(
        &self,
    ) -> Result<std::collections::HashMap<String, std::sync::Arc<dyn provider::Provider>>, anyhow::Error>
    {
        self.config
            .providers
            .iter()
            .filter_map(|pc| {
                self.provider_registry
                    .create(pc)
                    .map(|p| p.map(|p| (pc.name().to_owned(), p)))
                    .transpose()
            })
            .collect::<Result<_, anyhow::Error>>()
    }

    fn init_channels(&self) -> Result<Vec<Box<dyn Channel>>, anyhow::Error> {
        self.config
            .channels
            .iter()
            .filter_map(|cc| self.channel_registry.create(cc).transpose())
            .collect::<Result<_, anyhow::Error>>()
    }

    fn build_agent_configs(
        &self,
        providers: &std::collections::HashMap<String, std::sync::Arc<dyn provider::Provider>>,
    ) -> Result<Vec<crate::agent::AgentSessionConfig>, anyhow::Error> {
        use crate::agent::AgentSessionConfig;

        self.config
            .agents
            .iter()
            .map(|agent| {
                let provider = providers.get(&agent.provider).ok_or_else(|| {
                    anyhow::anyhow!("provider not found: {}", agent.provider)
                })?;
                AgentSessionConfig::from_agent_config(
                    agent,
                    std::sync::Arc::clone(provider),
                    self.config
                        .model_options_for_agent(&agent.name)
                        .cloned()
                        .unwrap_or_default(),
                    &self.middleware_registry,
                )
            })
            .collect::<Result<_, anyhow::Error>>()
    }

    fn build_gate(
        &self,
        conn: &turso::Connection,
    ) -> Option<std::sync::Arc<crate::runtime::session_gate::SessionGate>> {
        use crate::runtime::session_gate::SessionGate;

        self.config.password_hash.as_ref().map(|hash| {
            let valid_agents: Vec<String> =
                self.config.agents.iter().map(|a| a.name.clone()).collect();
            std::sync::Arc::new(SessionGate::new(hash.clone(), valid_agents, conn.clone()))
        })
    }

    fn build_runtimes(
        &self,
        channels: Vec<Box<dyn Channel>>,
        conn: turso::Connection,
        agent_configs: Vec<crate::agent::AgentSessionConfig>,
        gate: Option<std::sync::Arc<crate::runtime::session_gate::SessionGate>>,
    ) -> Vec<crate::runtime::channel::ChannelRuntime> {
        use crate::runtime::channel::{ChannelContext, ChannelRuntime};

        channels
            .into_iter()
            .map(|ch| {
                let mut rt = ChannelRuntime::new(
                    ch,
                    ChannelContext {
                        app_db: conn.clone(),
                    },
                    agent_configs.clone(),
                );
                if let Some(ref g) = gate {
                    rt = rt.with_gate(std::sync::Arc::clone(g));
                }
                rt
            })
            .collect()
    }

    /// Validate config, initialize the database, wire up channel runtimes
    /// for every channel×agent combination, and run them concurrently.
    ///
    /// Awaits all runtimes; returns early on first error or on SIGINT (Ctrl+C).
    pub async fn run(&mut self) -> Result<(), anyhow::Error> {
        use crate::runtime::Runtime;

        let runtimes = self.init().await?;
        let handles: Vec<_> = runtimes
            .into_iter()
            .map(|mut rt| tokio::spawn(async move { rt.run().await }))
            .collect();

        tokio::select! {
            result = async {
                for h in handles {
                    h.await??;
                }
                Ok::<_, anyhow::Error>(())
            } => result,
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("received SIGINT, shutting down");
                Ok(())
            },
        }
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
            password_hash: None,
            database_path: ":memory:".into(),
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
