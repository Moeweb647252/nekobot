pub mod middleware;
pub mod types;

pub struct Context {
    pub(crate) agent_id: String,
}

impl Context {
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }
}
