//! Channel runtime — connects a channel adapter to an agent session, routing incoming events
//! and outgoing replies.

use std::collections::HashMap;
use std::sync::Arc;

use nekobot_channel::{Channel, ChannelInfo, ChannelId, ChatInfo, ChatId, Event, ReplyTarget, Request};
use tokio::sync::mpsc::Sender;
use turso::Connection;

use super::Runtime;
use crate::{
    agent::{
        AgentOutput, AgentSession, AgentSessionConfig, AgentSessionHandle,
        middleware::AgentActivation,
    },
    entity::{
        Entity,
        channel_chat_agent::{AgentName, ChannelChatAgent, NewChannelChatAgent, SessionId},
        message::Message,
        session::Session,
    },
};

use super::session_gate::{InterceptResult, SessionGate};

type ChannelAgentKey = (
    ChannelId,
    ChatId,
    AgentName,
);

/// Decides which agent handles a given chat.
///
/// Receives `(channel_id, chat_id)` and returns the agent name. The default
/// route returns the first configured agent for every chat.
pub type AgentRoute = Arc<dyn Fn(&ChannelId, &ChatId) -> String + Send + Sync>;

/// Runtime that ties a [`Channel`] adapter to one or more agent sessions,
/// routing each chat to an agent via [`AgentRoute`].
pub struct ChannelRuntime {
    channel: Box<dyn Channel>,
    context: ChannelContext,
    agent_configs: Vec<AgentSessionConfig>,
    route: AgentRoute,
    gate: Option<Arc<SessionGate>>,
    sessions: HashMap<ChannelAgentKey, AgentSessionHandle>,
    session_targets: HashMap<SessionId, ReplyTarget>,
}

/// Database handle and shared context for a [`ChannelRuntime`].
pub struct ChannelContext {
    pub(crate) app_db: Connection,
}

impl ChannelRuntime {
    /// Create a new [`ChannelRuntime`] for the given channel, context, and agent configs.
    ///
    /// The default [`AgentRoute`] always picks the first agent in `agent_configs`.
    /// Use [`with_route`](ChannelRuntime::with_route) to customize.
    pub fn new(
        channel: Box<dyn Channel>,
        context: ChannelContext,
        agent_configs: Vec<AgentSessionConfig>,
    ) -> Self {
        let first = agent_configs
            .first()
            .map(|c| c.agent_name.clone())
            .unwrap_or_default();
        let route: AgentRoute = Arc::new(move |_, _| first.clone());
        Self {
            channel,
            context,
            agent_configs,
            route,
            gate: None,
            sessions: HashMap::new(),
            session_targets: HashMap::new(),
        }
    }

    /// Override the agent routing function.
    pub fn with_route(mut self, route: AgentRoute) -> Self {
        self.route = route;
        self
    }

    /// Attach a [`SessionGate`] for C2C access control.
    pub fn with_gate(mut self, gate: Arc<SessionGate>) -> Self {
        self.gate = Some(gate);
        self
    }

    async fn prepare_tables(&self) -> anyhow::Result<()> {
        Message::create_table(&self.context.app_db).await?;
        ChannelChatAgent::create_table(&self.context.app_db).await?;
        Ok(())
    }

    async fn handle_channel_event(
        &mut self,
        channel_info: &ChannelInfo,
        event: Event,
        output_sender: Sender<AgentOutput>,
    ) -> anyhow::Result<()> {
        match event {
            Event::IncomingMessage {
                chat,
                sender,
                content,
            } => {
                // C2C gate interception — login / connect before agent.
                // Only applies to C2C private chats (chat id prefixed with "c2c:").
                let is_c2c = chat.id.as_str().starts_with("c2c:");
                let agent_name_override = if is_c2c {
                    if let Some(gate) = &self.gate {
                        match gate
                            .intercept(channel_info.id.as_str(), sender.id.as_str(), &content)
                            .await?
                        {
                        InterceptResult::Reject { reply } => {
                            self.channel
                                .send(Request::SendMessage {
                                    target: chat.reply_target.clone(),
                                    content: reply,
                                })
                                .await?;
                            return Ok(());
                        }
                        InterceptResult::Pass { agent_name } => Some(agent_name),
                    }
                } else {
                    None
                }
            } else {
                None
            };

            let handle = self
                .ensure_agent_session(
                    channel_info,
                    &chat,
                    output_sender,
                    agent_name_override,
                    )
                    .await?;

                handle
                    .activation_sender
                    .send(AgentActivation::ChannelMessage {
                        chat_name: chat.name.into_string(),
                        sender_name: sender.name.into_string(),
                        content,
                    })
                    .await?;
            }
        }

        Ok(())
    }

    async fn ensure_agent_session(
        &mut self,
        channel_info: &ChannelInfo,
        chat: &ChatInfo,
        output_sender: Sender<AgentOutput>,
        agent_name_override: Option<String>,
    ) -> anyhow::Result<AgentSessionHandle> {
        let agent_name_str = agent_name_override
            .unwrap_or_else(|| (self.route)(&channel_info.id, &chat.id));
        let config = self
            .agent_configs
            .iter()
            .find(|c| c.agent_name == agent_name_str)
            .ok_or_else(|| {
                anyhow::anyhow!("no agent config found for agent '{agent_name_str}'")
            })?;

        let agent_name = AgentName::from(agent_name_str);
        let mapping = match ChannelChatAgent::get_by_channel_chat_agent(
            &self.context.app_db,
            &channel_info.id,
            &chat.id,
            &agent_name,
        )
        .await?
        {
            Some(mapping) => {
                if mapping.channel_name != channel_info.name
                    || mapping.chat_name != chat.name
                    || mapping.reply_target != chat.reply_target
                {
                    ChannelChatAgent::update_chat_cache(
                        &self.context.app_db,
                        mapping.id,
                        channel_info.name.clone(),
                        chat.name.clone(),
                        chat.reply_target.clone(),
                    )
                    .await?
                    .unwrap_or(mapping)
                } else {
                    mapping
                }
            }
            None => {
                let session =
                    Session::create(&self.context.app_db, agent_name.as_str()).await?;
                ChannelChatAgent::create(
                    &self.context.app_db,
                    NewChannelChatAgent {
                        channel_id: channel_info.id.clone(),
                        channel_name: channel_info.name.clone(),
                        chat_id: chat.id.clone(),
                        chat_name: chat.name.clone(),
                        reply_target: chat.reply_target.clone(),
                        agent_name: agent_name.clone(),
                        session_id: SessionId::from(session.id),
                    },
                )
                .await?
            }
        };

        self.session_targets
            .insert(mapping.session_id, mapping.reply_target.clone());

        let key = (
            mapping.channel_id.clone(),
            mapping.chat_id.clone(),
            mapping.agent_name.clone(),
        );

        if let Some(handle) = self.sessions.get(&key) {
            return Ok(handle.clone());
        }

        let agent_session =
            AgentSession::new(mapping.session_id.as_i64(), config.clone());
        let handle = agent_session
            .start(self.context.app_db.clone(), output_sender)
            .await?;
        self.sessions.insert(key, handle.clone());

        Ok(handle)
    }

    async fn handle_agent_output(&self, output: AgentOutput) -> anyhow::Result<()> {
        match output {
            AgentOutput::SendMessage {
                session_id,
                content,
            } => {
                let target = self
                    .session_targets
                    .get(&SessionId::from(session_id))
                    .ok_or_else(|| {
                        anyhow::anyhow!("missing channel target for session {session_id}")
                    })?;

                self.channel
                    .send(Request::SendMessage {
                        target: target.clone(),
                        content,
                    })
                    .await?;
            }
        }

        Ok(())
    }
}

impl Runtime for ChannelRuntime {
    /// Prepare tables, register with the channel, and enter the event loop.
    async fn run(&mut self) -> anyhow::Result<()> {
        self.prepare_tables().await?;

        let (event_sender, mut event_receiver) = tokio::sync::mpsc::channel(64);
        let (output_sender, mut output_receiver) = tokio::sync::mpsc::channel(64);
        let channel_info = self.channel.register(event_sender).await?;

        loop {
            tokio::select! {
                event = event_receiver.recv() => {
                    let Some(event) = event else {
                        break;
                    };
                    self.handle_channel_event(&channel_info, event, output_sender.clone()).await?;
                }
                output = output_receiver.recv() => {
                    let Some(output) = output else {
                        break;
                    };
                    self.handle_agent_output(output).await?;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use nekobot_channel::{
        ChannelId, ChannelInfo, ChannelName, ChatId, ChatInfo, ChatName, ReplyTarget, SenderId,
        SenderInfo, SenderName,
    };
    use tokio::sync::{Mutex, Notify};
    use turso::Builder;

    use crate::{
        agent::types::ChatResponse,
        entity::{
            channel_chat_agent::{AgentName, ChannelChatAgent},
            message::Message,
            session::Session,
        },
        provider::{ModelOptions, Provider, ProviderError, ProviderRequest},
    };

    use super::*;

    #[derive(Clone)]
    struct TestChannel {
        state: Arc<TestChannelState>,
    }

    struct TestChannelState {
        event_sender: Mutex<Option<tokio::sync::mpsc::Sender<Event>>>,
        sent_requests: Mutex<Vec<Request>>,
        registered: Notify,
    }

    impl TestChannel {
        fn new() -> Self {
            Self {
                state: Arc::new(TestChannelState {
                    event_sender: Mutex::new(None),
                    sent_requests: Mutex::new(Vec::new()),
                    registered: Notify::new(),
                }),
            }
        }

        async fn emit(&self, event: Event) -> anyhow::Result<()> {
            loop {
                if let Some(sender) = self.state.event_sender.lock().await.clone() {
                    sender.send(event).await?;
                    return Ok(());
                }

                self.state.registered.notified().await;
            }
        }

        async fn sent_requests(&self) -> Vec<Request> {
            self.state.sent_requests.lock().await.clone()
        }
    }

    #[async_trait::async_trait]
    impl Channel for TestChannel {
        async fn register(
            &self,
            sender: tokio::sync::mpsc::Sender<Event>,
        ) -> anyhow::Result<ChannelInfo> {
            *self.state.event_sender.lock().await = Some(sender);
            self.state.registered.notify_waiters();
            Ok(ChannelInfo {
                id: ChannelId::from("test-channel"),
                name: ChannelName::from("Test Channel"),
            })
        }

        async fn send(&self, request: Request) -> anyhow::Result<()> {
            self.state.sent_requests.lock().await.push(request);
            Ok(())
        }
    }

    struct EchoProvider {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl Provider for EchoProvider {
        async fn complete(&self, request: ProviderRequest) -> Result<ChatResponse, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let content = request
                .chat
                .messages
                .last()
                .map(|message| format!("echo: {}", message.content.content))
                .unwrap_or_else(|| "echo".to_owned());

            Ok(ChatResponse {
                content,
                reasoning_content: None,
                images: Vec::new(),
                usage: None,
            })
        }
    }

    async fn runtime(
        channel: TestChannel,
    ) -> anyhow::Result<(ChannelRuntime, Connection, Arc<AtomicUsize>)> {
        let db = Builder::new_local(":memory:").build().await?;
        let conn = db.connect()?;
        let (runtime, calls) = runtime_with_connection(channel, conn.clone(), "Neko");
        Ok((runtime, conn, calls))
    }

    fn runtime_with_connection(
        channel: TestChannel,
        conn: Connection,
        agent_name: &str,
    ) -> (ChannelRuntime, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let agent_config = crate::config::AgentConfig {
            name: agent_name.to_owned(),
            provider: "test-provider".to_owned(),
            model: "test-model".to_owned(),
            middlewares: Vec::new(),
        };
        let middleware_registry = crate::agent::MiddlewareRegistry::new();
        let agent_session_config = AgentSessionConfig::from_agent_config(
            &agent_config,
            Arc::new(EchoProvider {
                calls: Arc::clone(&calls),
            }),
            ModelOptions::default(),
            &middleware_registry,
        )
        .unwrap();
        let runtime = ChannelRuntime::new(
            Box::new(channel),
            ChannelContext {
                app_db: conn.clone(),
            },
            vec![agent_session_config],
        );

        (runtime, calls)
    }

    fn channel_info() -> ChannelInfo {
        ChannelInfo {
            id: ChannelId::from("test-channel"),
            name: ChannelName::from("Test Channel"),
        }
    }

    fn chat(id: &str, name: &str, reply_target: &str) -> ChatInfo {
        ChatInfo {
            id: ChatId::from(id),
            name: ChatName::from(name),
            reply_target: ReplyTarget::from(reply_target),
        }
    }

    fn sender(id: &str, name: &str) -> SenderInfo {
        SenderInfo {
            id: SenderId::from(id),
            name: SenderName::from(name),
        }
    }

    #[tokio::test]
    async fn channel_message_creates_session_records_messages_and_routes_output()
    -> anyhow::Result<()> {
        let channel = TestChannel::new();
        let (mut runtime, conn, calls) = runtime(channel.clone()).await?;
        let runtime_task = tokio::spawn(async move { runtime.run().await });

        channel
            .emit(Event::IncomingMessage {
                chat: chat("chat-alice", "Alice", "alice-target"),
                sender: sender("sender-alice", "Alice"),
                content: "hello".to_owned(),
            })
            .await?;

        wait_for_sent_requests(&channel, 1).await;

        let mapping = ChannelChatAgent::get_by_channel_chat_agent(
            &conn,
            &ChannelId::from("test-channel"),
            &ChatId::from("chat-alice"),
            &AgentName::from("Neko"),
        )
        .await?
        .expect("channel chat agent mapping should exist");
        let session = Session::get(&conn, mapping.session_id.as_i64())
            .await?
            .expect("session should exist");
        let messages = Message::list_by_session(&conn, mapping.session_id.as_i64()).await?;

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(session.agent_name, "Neko");
        assert_eq!(mapping.channel_id, ChannelId::from("test-channel"));
        assert_eq!(mapping.chat_id, ChatId::from("chat-alice"));
        assert_eq!(mapping.reply_target, ReplyTarget::from("alice-target"));
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "hello");
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].content, "echo: hello");
        assert_eq!(
            channel.sent_requests().await,
            vec![Request::SendMessage {
                target: ReplyTarget::from("alice-target"),
                content: "echo: hello".to_owned(),
            }]
        );

        runtime_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn same_chat_id_reuses_session_and_different_chat_id_creates_new_session()
    -> anyhow::Result<()> {
        let channel = TestChannel::new();
        let (mut runtime, conn, _calls) = runtime(channel.clone()).await?;
        let runtime_task = tokio::spawn(async move { runtime.run().await });

        for (chat_id, chat_name, content) in [
            ("chat-1", "Alice", "first"),
            ("chat-1", "Alice Updated", "second"),
            ("chat-2", "Alice", "third"),
        ] {
            channel
                .emit(Event::IncomingMessage {
                    chat: chat(chat_id, chat_name, &format!("{chat_id}-target")),
                    sender: sender("sender-alice", chat_name),
                    content: content.to_owned(),
                })
                .await?;
        }

        wait_for_sent_requests(&channel, 3).await;

        let first_chat = ChannelChatAgent::get_by_channel_chat_agent(
            &conn,
            &ChannelId::from("test-channel"),
            &ChatId::from("chat-1"),
            &AgentName::from("Neko"),
        )
        .await?
        .expect("first chat mapping should exist");
        let second_chat = ChannelChatAgent::get_by_channel_chat_agent(
            &conn,
            &ChannelId::from("test-channel"),
            &ChatId::from("chat-2"),
            &AgentName::from("Neko"),
        )
        .await?
        .expect("second chat mapping should exist");

        assert_ne!(first_chat.session_id, second_chat.session_id);
        assert_eq!(first_chat.chat_name, ChatName::from("Alice Updated"));
        assert_eq!(
            Message::list_by_session(&conn, first_chat.session_id.as_i64())
                .await?
                .len(),
            4
        );
        assert_eq!(
            Message::list_by_session(&conn, second_chat.session_id.as_i64())
                .await?
                .len(),
            2
        );

        runtime_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn same_chat_can_bind_to_different_agents() -> anyhow::Result<()> {
        let db = Builder::new_local(":memory:").build().await?;
        let conn = db.connect()?;
        let channel = TestChannel::new();
        let (mut neko_runtime, _neko_calls) =
            runtime_with_connection(channel.clone(), conn.clone(), "Neko");
        let (mut mimi_runtime, _mimi_calls) =
            runtime_with_connection(channel, conn.clone(), "Mimi");
        let (output_sender, _output_receiver) = tokio::sync::mpsc::channel(16);
        let chat = chat("chat-1", "Alice", "alice-target");
        let channel_info = channel_info();

        neko_runtime.prepare_tables().await?;
        neko_runtime
            .ensure_agent_session(&channel_info, &chat, output_sender.clone(), None)
            .await?;
        mimi_runtime
            .ensure_agent_session(&channel_info, &chat, output_sender, None)
            .await?;

        let neko_mapping = ChannelChatAgent::get_by_channel_chat_agent(
            &conn,
            &ChannelId::from("test-channel"),
            &ChatId::from("chat-1"),
            &AgentName::from("Neko"),
        )
        .await?
        .expect("Neko mapping should exist");
        let mimi_mapping = ChannelChatAgent::get_by_channel_chat_agent(
            &conn,
            &ChannelId::from("test-channel"),
            &ChatId::from("chat-1"),
            &AgentName::from("Mimi"),
        )
        .await?
        .expect("Mimi mapping should exist");

        assert_ne!(neko_mapping.session_id, mimi_mapping.session_id);

        Ok(())
    }

    #[tokio::test]
    async fn reply_target_change_updates_routing_cache() -> anyhow::Result<()> {
        let channel = TestChannel::new();
        let (mut runtime, conn, _calls) = runtime(channel.clone()).await?;
        let runtime_task = tokio::spawn(async move { runtime.run().await });

        channel
            .emit(Event::IncomingMessage {
                chat: chat("chat-1", "Alice", "target-1"),
                sender: sender("sender-alice", "Alice"),
                content: "first".to_owned(),
            })
            .await?;
        wait_for_sent_requests(&channel, 1).await;

        channel
            .emit(Event::IncomingMessage {
                chat: chat("chat-1", "Alice", "target-2"),
                sender: sender("sender-alice", "Alice"),
                content: "second".to_owned(),
            })
            .await?;
        wait_for_sent_requests(&channel, 2).await;

        let mapping = ChannelChatAgent::get_by_channel_chat_agent(
            &conn,
            &ChannelId::from("test-channel"),
            &ChatId::from("chat-1"),
            &AgentName::from("Neko"),
        )
        .await?
        .expect("mapping should exist");

        assert_eq!(mapping.reply_target, ReplyTarget::from("target-2"));
        assert_eq!(
            channel.sent_requests().await,
            vec![
                Request::SendMessage {
                    target: ReplyTarget::from("target-1"),
                    content: "echo: first".to_owned(),
                },
                Request::SendMessage {
                    target: ReplyTarget::from("target-2"),
                    content: "echo: second".to_owned(),
                },
            ]
        );

        runtime_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn agent_output_without_mapping_returns_error() -> anyhow::Result<()> {
        let channel = TestChannel::new();
        let (runtime, _conn, _calls) = runtime(channel).await?;

        let error = runtime
            .handle_agent_output(AgentOutput::SendMessage {
                session_id: 999,
                content: "hello".to_owned(),
            })
            .await
            .expect_err("missing mapping should fail");

        assert_eq!(error.to_string(), "missing channel target for session 999");
        Ok(())
    }

    async fn wait_for_sent_requests(channel: &TestChannel, len: usize) {
        for _ in 0..100 {
            if channel.sent_requests().await.len() >= len {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        panic!("timed out waiting for sent requests");
    }
}
