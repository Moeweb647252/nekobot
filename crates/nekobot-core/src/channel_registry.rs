//! Channel factory registry — maps channel type names to constructors.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use nekobot_channel::Channel;

use crate::config::ChannelConfig;

/// Factory closure type for creating a channel from its config.
pub type ChannelCreateFn =
    Arc<dyn Fn(&ChannelConfig) -> anyhow::Result<Box<dyn Channel>> + Send + Sync>;

/// Registry of named channel factories.
///
/// Maps channel type names (e.g. `"QQ"`) to factory closures registered
/// by external crates like `nekobot-channel`.
#[derive(Clone, Default)]
pub struct ChannelRegistry {
    factories: HashMap<String, ChannelCreateFn>,
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
        let name = name.into();
        if name.trim().is_empty() {
            anyhow::bail!("channel factory name cannot be empty");
        }
        if self.factories.contains_key(&name) {
            anyhow::bail!("duplicate channel factory: {name}");
        }
        self.factories.insert(name, Arc::new(create));
        Ok(())
    }

    /// Create a channel from its config.
    ///
    /// Returns `Ok(None)` if no factory is registered for this config's type.
    pub fn create(&self, config: &ChannelConfig) -> anyhow::Result<Option<Box<dyn Channel>>> {
        let name = config.name();
        let Some(factory) = self.factories.get(name) else {
            return Ok(None);
        };
        factory(config)
            .with_context(|| format!("failed to create channel {name}"))
            .map(Some)
    }
}
