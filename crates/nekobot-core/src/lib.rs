pub mod agent;
pub mod config;
pub mod entity;

pub struct NekoBot<S> {
    state: S,
}
