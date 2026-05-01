//! Channel factory registry — maps channel type names to constructors.

use anyhow::Context;
use nekobot_channel::Channel;

use crate::config::ChannelConfig;
use crate::registry::FactoryRegistry;

/// Registry of channel factories, mapping type names (e.g. `"QQ"`) to factory closures
/// registered by external crates like `nekobot-channel`.
#[derive(Clone, Default)]
pub struct ChannelRegistry {
    inner: FactoryRegistry<ChannelConfig, Box<dyn Channel>>,
}

impl ChannelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a channel factory under the given name.
    ///
    /// The name must match the `type` tag in [`ChannelConfig`] JSON (e.g. `"QQ"`).
    pub fn register<F>(&mut self, name: impl Into<String>, create: F) -> anyhow::Result<()>
    where
        F: Fn(&ChannelConfig) -> anyhow::Result<Box<dyn Channel>> + Send + Sync + 'static,
    {
        self.inner.register(name, create)
    }

    /// Create a channel from its config.
    ///
    /// Returns `Ok(None)` if no factory is registered for this config's type.
    pub fn create(&self, config: &ChannelConfig) -> anyhow::Result<Option<Box<dyn Channel>>> {
        let name = config.type_name();
        let Some(factory) = self.inner.get(name) else {
            return Ok(None);
        };
        factory(config)
            .with_context(|| format!("failed to create channel {name}"))
            .map(Some)
    }
}
