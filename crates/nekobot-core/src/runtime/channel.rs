use std::collections::HashMap;

use nekobot_channel::{Channel, ChatInfo, Event, Request, entity::contact::Contact};
use tokio::sync::mpsc::Sender;
use turso::Connection;

use super::Runtime;
use crate::{
    agent::{
        AgentOutput, AgentSession, AgentSessionConfig, AgentSessionHandle,
        middleware::AgentActivation,
    },
    entity::{Entity, message::Message, session::Session},
};

pub struct ChannelRuntime {
    channel: Box<dyn Channel>,
    context: ChannelContext,
    agent_config: AgentSessionConfig,
    sessions: HashMap<String, AgentSessionHandle>,
    session_targets: HashMap<i64, String>,
}

pub struct ChannelContext {
    pub(crate) app_db: Connection,
}

impl ChannelRuntime {
    pub fn new(
        channel: Box<dyn Channel>,
        context: ChannelContext,
        agent_config: AgentSessionConfig,
    ) -> Self {
        Self {
            channel,
            context,
            agent_config,
            sessions: HashMap::new(),
            session_targets: HashMap::new(),
        }
    }

    async fn prepare_tables(&self) -> anyhow::Result<()> {
        Message::create_table(&self.context.app_db).await?;
        Contact::create_table(&self.context.app_db).await?;
        Ok(())
    }

    async fn handle_channel_event(
        &mut self,
        event: Event,
        output_sender: Sender<AgentOutput>,
    ) -> anyhow::Result<()> {
        match event {
            Event::IncomingMessage {
                chat,
                sender,
                content,
            } => {
                let handle = self.ensure_agent_session(&chat, output_sender).await?;

                handle
                    .activation_sender
                    .send(AgentActivation::ChannelMessage {
                        chat_name: chat.name,
                        sender_name: sender.name,
                        content,
                    })
                    .await?;
            }
        }

        Ok(())
    }

    async fn ensure_agent_session(
        &mut self,
        chat: &ChatInfo,
        output_sender: Sender<AgentOutput>,
    ) -> anyhow::Result<AgentSessionHandle> {
        let contact = match Contact::get_by_name(&self.context.app_db, &chat.name).await? {
            Some(contact) => {
                if contact.target != chat.reply_target {
                    Contact::update_target(
                        &self.context.app_db,
                        contact.id,
                        chat.reply_target.clone(),
                    )
                    .await?
                    .unwrap_or(contact)
                } else {
                    contact
                }
            }
            None => {
                let session =
                    Session::create(&self.context.app_db, self.agent_config.agent_id).await?;
                Contact::create(
                    &self.context.app_db,
                    session.id,
                    chat.name.clone(),
                    chat.reply_target.clone(),
                )
                .await?
            }
        };

        self.session_targets
            .insert(contact.session_id, contact.target.clone());

        if let Some(handle) = self.sessions.get(&chat.name) {
            return Ok(handle.clone());
        }

        let agent_session = AgentSession::new(contact.session_id, self.agent_config.clone());
        let handle = agent_session
            .start(self.context.app_db.clone(), output_sender)
            .await?;
        self.sessions.insert(chat.name.clone(), handle.clone());

        Ok(handle)
    }

    async fn handle_agent_output(&self, output: AgentOutput) -> anyhow::Result<()> {
        match output {
            AgentOutput::SendMessage {
                session_id,
                content,
            } => {
                let target = self.session_targets.get(&session_id).ok_or_else(|| {
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
    async fn run(&mut self) -> anyhow::Result<()> {
        self.prepare_tables().await?;

        let (event_sender, mut event_receiver) = tokio::sync::mpsc::channel(64);
        let (output_sender, mut output_receiver) = tokio::sync::mpsc::channel(64);
        let _channel_info = self.channel.register(event_sender).await?;

        loop {
            tokio::select! {
                event = event_receiver.recv() => {
                    let Some(event) = event else {
                        break;
                    };
                    self.handle_channel_event(event, output_sender.clone()).await?;
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

    use nekobot_channel::{ChannelInfo, SenderInfo};
    use tokio::sync::{Mutex, Notify};
    use turso::Builder;

    use crate::{
        agent::types::{ChatRequest, ChatResponse},
        entity::{Entity, agent::Agent},
        provider::{ModelOptions, Provider},
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
                name: "test".to_owned(),
            })
        }

        async fn send(&self, request: Request) -> anyhow::Result<()> {
            self.state.sent_requests.lock().await.push(request);
            Ok(())
        }

        async fn get_contact_list(&self, _agent_id: i64) -> anyhow::Result<Vec<String>> {
            Ok(Vec::new())
        }
    }

    struct EchoProvider {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl Provider for EchoProvider {
        async fn chat(
            &self,
            request: ChatRequest,
            _option: ModelOptions,
            _event_sender: Option<std::sync::mpsc::Sender<crate::provider::ChatEvent>>,
        ) -> Result<ChatResponse, anyhow::Error> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let content = request
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
        Agent::create_table(&conn).await?;
        let agent = Agent::create(&conn, "Neko", "gpt-5.4").await?;
        let calls = Arc::new(AtomicUsize::new(0));
        let runtime = ChannelRuntime::new(
            Box::new(channel),
            ChannelContext {
                app_db: conn.clone(),
            },
            AgentSessionConfig::new(
                agent.id,
                Arc::new(EchoProvider {
                    calls: Arc::clone(&calls),
                }),
                ModelOptions::default(),
                Vec::new(),
            ),
        );

        Ok((runtime, conn, calls))
    }

    #[tokio::test]
    async fn channel_message_creates_session_records_messages_and_routes_output()
    -> anyhow::Result<()> {
        let channel = TestChannel::new();
        let (mut runtime, conn, calls) = runtime(channel.clone()).await?;
        let runtime_task = tokio::spawn(async move { runtime.run().await });

        channel
            .emit(Event::IncomingMessage {
                chat: ChatInfo {
                    name: "Alice".to_owned(),
                    reply_target: "alice-target".to_owned(),
                },
                sender: SenderInfo {
                    name: "Alice".to_owned(),
                },
                content: "hello".to_owned(),
            })
            .await?;

        wait_for_sent_requests(&channel, 1).await;

        let contact = Contact::get_by_name(&conn, "Alice")
            .await?
            .expect("contact should exist");
        let messages = Message::list_by_session(&conn, contact.session_id).await?;

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "hello");
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].content, "echo: hello");
        assert_eq!(
            channel.sent_requests().await,
            vec![Request::SendMessage {
                target: "alice-target".to_owned(),
                content: "echo: hello".to_owned(),
            }]
        );

        runtime_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn same_chat_reuses_session_and_different_chat_creates_new_session() -> anyhow::Result<()>
    {
        let channel = TestChannel::new();
        let (mut runtime, conn, _calls) = runtime(channel.clone()).await?;
        let runtime_task = tokio::spawn(async move { runtime.run().await });

        for (chat_name, content) in [("Alice", "first"), ("Alice", "second"), ("Bob", "third")] {
            channel
                .emit(Event::IncomingMessage {
                    chat: ChatInfo {
                        name: chat_name.to_owned(),
                        reply_target: format!("{chat_name}-target"),
                    },
                    sender: SenderInfo {
                        name: chat_name.to_owned(),
                    },
                    content: content.to_owned(),
                })
                .await?;
        }

        wait_for_sent_requests(&channel, 3).await;

        let alice = Contact::get_by_name(&conn, "Alice")
            .await?
            .expect("Alice contact should exist");
        let bob = Contact::get_by_name(&conn, "Bob")
            .await?
            .expect("Bob contact should exist");

        assert_ne!(alice.session_id, bob.session_id);
        assert_eq!(
            Message::list_by_session(&conn, alice.session_id)
                .await?
                .len(),
            4
        );
        assert_eq!(
            Message::list_by_session(&conn, bob.session_id).await?.len(),
            2
        );

        runtime_task.abort();
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
