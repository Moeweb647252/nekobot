//! Runtime abstraction — drives the main event loop for a channel+agent pair.

pub mod channel;
pub mod session_gate;

/// A long-running task that processes events for a channel+agent combination.
///
/// Implementations are expected to loop until shutdown, processing inbound
/// channel events and routing agent output back.
pub trait Runtime {
    /// Run the runtime loop. Returns when the runtime shuts down.
    async fn run(&mut self) -> Result<(), anyhow::Error>;
}
