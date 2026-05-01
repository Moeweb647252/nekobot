//! Generic factory registry used by Provider, Channel, and Middleware registries.

use std::collections::HashMap;
use std::sync::Arc;

/// Generic factory registry mapping string keys to factory functions.
///
/// Shared by [`ProviderRegistry`](crate::provider::ProviderRegistry),
/// [`ChannelRegistry`](crate::channel_registry::ChannelRegistry), and
/// [`MiddlewareRegistry`](crate::agent::MiddlewareRegistry).
pub struct FactoryRegistry<C, O> {
    factories: HashMap<String, Arc<dyn Fn(&C) -> anyhow::Result<O> + Send + Sync>>,
}

// Manual Clone impl avoids adding O: Clone bound (O is only in fn signature, never stored).
impl<C, O> Clone for FactoryRegistry<C, O> {
    fn clone(&self) -> Self {
        Self {
            factories: self.factories.clone(),
        }
    }
}

impl<C, O> FactoryRegistry<C, O> {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
        }
    }

    /// Register a factory function under the given key.
    ///
    /// Returns an error if the key is empty or already registered.
    pub fn register<F>(&mut self, key: impl Into<String>, factory: F) -> anyhow::Result<()>
    where
        F: Fn(&C) -> anyhow::Result<O> + Send + Sync + 'static,
    {
        let key = key.into();
        if key.trim().is_empty() {
            anyhow::bail!("factory key cannot be empty");
        }
        if self.factories.contains_key(&key) {
            anyhow::bail!("duplicate factory: {key}");
        }
        self.factories.insert(key, Arc::new(factory));
        Ok(())
    }

    /// Look up a factory by key.
    pub fn get(&self, key: &str) -> Option<&Arc<dyn Fn(&C) -> anyhow::Result<O> + Send + Sync>> {
        self.factories.get(key)
    }
}

impl<C, O> Default for FactoryRegistry<C, O> {
    fn default() -> Self {
        Self::new()
    }
}
