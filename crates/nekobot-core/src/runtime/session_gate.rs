//! Session gate — pre-agent access control for C2C private chats.
//!
//! Implements a two-step state machine:
//! 1. `/login <password>` — authenticate, become logged in
//! 2. `/connect <agent>` — bind to a specific agent
//!
//! All interception happens before any agent session is created, so no
//! LLM tokens are consumed for login/connect flows.

use sha2::{Digest, Sha256};
use turso::Connection;

use crate::entity::sender_gate_state::SenderGateState;

/// Result of gate interception.
pub enum InterceptResult {
    /// Let the message through, handled by the named agent.
    Pass { agent_name: String },
    /// Block the message and send the given reply text back to the user.
    Reject { reply: String },
}

/// C2C access gate with persistent login state.
///
/// One instance can serve multiple channels — `channel_id` is passed
/// to [`intercept`](SessionGate::intercept) per-call.
pub struct SessionGate {
    password_hash: String,
    valid_agents: Vec<String>,
    conn: Connection,
}

impl SessionGate {
    pub fn new(
        password_hash: impl Into<String>,
        valid_agents: Vec<String>,
        conn: Connection,
    ) -> Self {
        Self {
            password_hash: password_hash.into(),
            valid_agents,
            conn,
        }
    }

    /// Intercept a message and return an [`InterceptResult`].
    ///
    /// - `/login <pw>` — validate password hash, persist login state
    /// - `/connect <agent>` — bind sender to an agent
    /// - other messages — pass through if logged in and connected, reject otherwise
    pub async fn intercept(
        &self,
        channel_id: &str,
        sender_id: &str,
        content: &str,
    ) -> anyhow::Result<InterceptResult> {
        // `/login <password>`
        if let Some(password) = content.strip_prefix("/login ").map(str::trim) {
            return self.handle_login(channel_id, sender_id, password).await;
        }

        let state = SenderGateState::get(&self.conn, channel_id, sender_id).await?;

        // Not logged in
        let Some(state) = state else {
            return Ok(InterceptResult::Reject {
                reply: "请先 /login <password>".into(),
            });
        };
        if !state.is_logged_in {
            return Ok(InterceptResult::Reject {
                reply: "请先 /login <password>".into(),
            });
        }

        // `/connect <agent>`
        if let Some(agent) = content.strip_prefix("/connect ").map(str::trim) {
            return self.handle_connect(channel_id, sender_id, agent).await;
        }

        // Connected — let through
        match state.connected_agent {
            Some(agent) => Ok(InterceptResult::Pass { agent_name: agent }),
            None => Ok(InterceptResult::Reject {
                reply: "请先 /connect <agent>".into(),
            }),
        }
    }

    async fn handle_login(
        &self,
        channel_id: &str,
        sender_id: &str,
        password: &str,
    ) -> anyhow::Result<InterceptResult> {
        let mut hasher = Sha256::new();
        hasher.update(password.as_bytes());
        let input_hash = format!("{:x}", hasher.finalize());

        if input_hash == self.password_hash {
            SenderGateState {
                channel_id: channel_id.to_owned(),
                sender_id: sender_id.to_owned(),
                is_logged_in: true,
                connected_agent: None,
            }
            .upsert(&self.conn)
            .await?;

            Ok(InterceptResult::Reject {
                reply: format!(
                    "登录成功，请 /connect <agent> 选择要连接的 agent。可用: {}",
                    self.valid_agents.join(", ")
                ),
            })
        } else {
            Ok(InterceptResult::Reject {
                reply: "密码错误".into(),
            })
        }
    }

    async fn handle_connect(
        &self,
        channel_id: &str,
        sender_id: &str,
        agent: &str,
    ) -> anyhow::Result<InterceptResult> {
        if !self.valid_agents.iter().any(|a| a == agent) {
            return Ok(InterceptResult::Reject {
                reply: format!(
                    "未知 agent: {agent}，可用: {}",
                    self.valid_agents.join(", ")
                ),
            });
        }

        SenderGateState {
            channel_id: channel_id.to_owned(),
            sender_id: sender_id.to_owned(),
            is_logged_in: true,
            connected_agent: Some(agent.to_owned()),
        }
        .upsert(&self.conn)
        .await?;

        Ok(InterceptResult::Reject {
            reply: format!("已连接到 {agent}，可以开始对话"),
        })
    }
}
