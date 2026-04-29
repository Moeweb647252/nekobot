use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::tungstenite::Message;

use crate::{Channel, ChannelInfo, ChatInfo, Event, ReplyTarget, Request, SenderInfo};

// ── API constants ──

const TOKEN_URL: &str = "https://bots.qq.com/app/getAppAccessToken";
const API_BASE: &str = "https://api.sgroup.qq.com";
const GATEWAY_PATH: &str = "/gateway";

// ── Intents ──

const INTENT_PUBLIC_GUILD_MESSAGES: u32 = 1 << 30;
const INTENT_DIRECT_MESSAGE: u32 = 1 << 12;
const INTENT_GROUP_AND_C2C: u32 = 1 << 25;
const INTENT_INTERACTION: u32 = 1 << 26;
const FULL_INTENTS: u32 =
    INTENT_PUBLIC_GUILD_MESSAGES | INTENT_DIRECT_MESSAGE | INTENT_GROUP_AND_C2C | INTENT_INTERACTION;

// ── QQChannel ──

pub struct QQChannel {
    app_id: String,
    client_secret: String,
    http: Client,
    state: Arc<Mutex<ChannelState>>,
}

struct ChannelState {
    token: Option<CachedToken>,
}

struct CachedToken {
    value: String,
    expires_at: Instant,
}

impl QQChannel {
    pub fn new(app_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self {
            app_id: app_id.into(),
            client_secret: client_secret.into(),
            http: Client::new(),
            state: Arc::new(Mutex::new(ChannelState { token: None })),
        }
    }

    async fn get_token(&self) -> anyhow::Result<String> {
        let mut state = self.state.lock().await;

        if let Some(cached) = &state.token {
            if cached.expires_at > Instant::now() + Duration::from_secs(60) {
                return Ok(cached.value.clone());
            }
        }

        let resp: TokenResponse = self
            .http
            .post(TOKEN_URL)
            .json(&serde_json::json!({
                "appId": self.app_id,
                "clientSecret": self.client_secret,
            }))
            .send()
            .await
            .context("failed to fetch access token")?
            .json()
            .await
            .context("failed to parse access token response")?;

        let token = resp.access_token.clone();
        let expires_in = resp.expires_in.unwrap_or(7200).max(60) as u64;
        state.token = Some(CachedToken {
            value: token.clone(),
            expires_at: Instant::now() + Duration::from_secs(expires_in),
        });

        Ok(token)
    }

    async fn get_gateway_url(&self, token: &str) -> anyhow::Result<String> {
        let resp: GatewayResponse = self
            .http
            .get(format!("{API_BASE}{GATEWAY_PATH}"))
            .header("Authorization", format!("QQBot {token}"))
            .send()
            .await
            .context("failed to get gateway url")?
            .json()
            .await
            .context("failed to parse gateway response")?;

        Ok(resp.url)
    }
}

// ── API response types ──

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: Option<i64>,
}

#[derive(Deserialize)]
struct GatewayResponse {
    url: String,
}

// ── WebSocket payload ──

#[derive(Deserialize)]
struct WSPayload {
    op: u32,
    #[serde(default)]
    d: Value,
    #[serde(default)]
    s: Option<u64>,
    #[serde(default)]
    t: Option<String>,
}

// ── Dispatch event types ──

#[derive(Deserialize, Debug)]
struct C2CMessageEvent {
    id: String,
    content: String,
    timestamp: String,
    author: AuthorInfo,
}

#[derive(Deserialize, Debug)]
struct GroupMessageEvent {
    id: String,
    content: String,
    timestamp: String,
    group_openid: String,
    author: AuthorInfo,
}

#[derive(Deserialize, Debug)]
struct AuthorInfo {
    user_openid: String,
    #[serde(default)]
    username: Option<String>,
}

fn build_reply_target_c2c(openid: &str) -> ReplyTarget {
    ReplyTarget::from(format!("c2c:{openid}"))
}

fn build_reply_target_group(group_openid: &str) -> ReplyTarget {
    ReplyTarget::from(format!("group:{group_openid}"))
}

fn parse_c2c_target(target: &ReplyTarget) -> Option<&str> {
    target.as_str().strip_prefix("c2c:")
}

fn parse_group_target(target: &ReplyTarget) -> Option<&str> {
    target.as_str().strip_prefix("group:")
}

// ── Channel trait impl ──

#[async_trait::async_trait]
impl Channel for QQChannel {
    async fn register(
        &self,
        event_tx: mpsc::Sender<Event>,
    ) -> anyhow::Result<ChannelInfo> {
        // Store event sender for use by send() (StartTyping needs event_tx for ref_idx etc.)
        // Actually no — keep it simple: event_tx is consumed by the ws loop.
        let state = self.state.clone();
        let app_id = self.app_id.clone();
        let client_secret = self.client_secret.clone();
        let http = self.http.clone();
        let event_tx_ws = event_tx.clone();

        let token = self.get_token().await?;
        let gateway_url = self.get_gateway_url(&token).await?;

        tokio::spawn(async move {
            if let Err(e) = run_gateway_loop(&http, &app_id, &client_secret, &gateway_url, &token, state, event_tx_ws).await {
                eprintln!("[QQChannel] gateway loop exited: {e}");
            }
        });

        Ok(ChannelInfo {
            id: crate::ChannelId::from(self.app_id.as_str()),
            name: crate::ChannelName::from("QQ"),
        })
    }

    async fn send(&self, request: Request) -> anyhow::Result<()> {
        let token = self.get_token().await?;

        match request {
            Request::SendMessage { target, content } => {
                if let Some(openid) = parse_c2c_target(&target) {
                    send_c2c_message(&self.http, &token, openid, &content).await?;
                } else if let Some(group_openid) = parse_group_target(&target) {
                    send_group_message(&self.http, &token, group_openid, &content).await?;
                } else {
                    anyhow::bail!("unknown QQ target format: {target}");
                }
            }
            Request::StartTyping { target } => {
                if let Some(openid) = parse_c2c_target(&target) {
                    let _ = send_c2c_input_notify(&self.http, &token, openid).await;
                }
                // Group doesn't support input notify, just ignore
            }
            Request::StopTyping { .. } => {
                // QQ doesn't need explicit stop typing
            }
        }

        Ok(())
    }
}

// ── Gateway loop ──

async fn run_gateway_loop(
    _http: &Client,
    _app_id: &str,
    _client_secret: &str,
    gateway_url: &str,
    token: &str,
    _state: Arc<Mutex<ChannelState>>,
    event_tx: mpsc::Sender<Event>,
) -> anyhow::Result<()> {
    let (ws, _resp) = tokio_tungstenite::connect_async(gateway_url)
        .await
        .context("failed to connect to QQ gateway")?;

    use std::pin::pin;
    let mut ws = pin!(ws);
    let current_token = token.to_owned();
    let mut heartbeat_interval = Duration::from_secs(41); // default

    loop {
        tokio::select! {
            msg = ws.next() => {
                let msg = match msg {
                    Some(Ok(msg)) => msg,
                    Some(Err(e)) => {
                        eprintln!("[QQChannel] ws error: {e}");
                        break;
                    }
                    None => break,
                };

                match msg {
                    Message::Text(text) => {
                        let Ok(payload) = serde_json::from_str::<WSPayload>(&text) else {
                            continue;
                        };

                        match payload.op {
                            10 => {
                                // Hello
                                if let Some(interval) = payload.d["heartbeat_interval"].as_u64() {
                                    heartbeat_interval = Duration::from_millis(interval);
                                }
                                let identify = serde_json::json!({
                                    "op": 2,
                                    "d": {
                                        "token": format!("QQBot {current_token}"),
                                        "intents": FULL_INTENTS,
                                        "shard": [0, 1],
                                    },
                                });
                                ws.send(Message::Text(identify.to_string().into())).await?;
                            }
                            0 => {
                                let t = payload.t.as_deref().unwrap_or("");
                                if let Err(e) = dispatch_event(t, payload.d, &event_tx).await {
                                    eprintln!("[QQChannel] dispatch error: {e}");
                                }
                            }
                            11 => {} // Heartbeat ACK
                            _ => {}
                        }
                    }
                    Message::Close(_) => break,
                    Message::Ping(data) => {
                        let _ = ws.send(Message::Pong(data)).await;
                    }
                    _ => {}
                }
            }
            _ = tokio::time::sleep(heartbeat_interval) => {
                let heartbeat = serde_json::json!({ "op": 1, "d": null });
                if ws.send(Message::Text(heartbeat.to_string().into())).await.is_err() {
                    break;
                }
            }
        }
    }

    Ok(())
}

async fn dispatch_event(
    event_type: &str,
    data: Value,
    event_tx: &mpsc::Sender<Event>,
) -> anyhow::Result<()> {
    match event_type {
        "C2C_MESSAGE_CREATE" => {
            let event = serde_json::from_value::<C2CMessageEvent>(data)?;
            event_tx
                .send(Event::IncomingMessage {
                    chat: ChatInfo {
                        id: crate::ChatId::from(format!("c2c:{}", event.author.user_openid)),
                        name: crate::ChatName::from(
                            event.author.username.as_deref().unwrap_or("私聊"),
                        ),
                        reply_target: build_reply_target_c2c(&event.author.user_openid),
                    },
                    sender: SenderInfo {
                        id: crate::SenderId::from(event.author.user_openid.clone()),
                        name: crate::SenderName::from(
                            event.author.username.unwrap_or(event.author.user_openid),
                        ),
                    },
                    content: event.content,
                })
                .await?;
        }
        "GROUP_AT_MESSAGE_CREATE" => {
            let event = serde_json::from_value::<GroupMessageEvent>(data)?;
            event_tx
                .send(Event::IncomingMessage {
                    chat: ChatInfo {
                        id: crate::ChatId::from(format!("group:{}", event.group_openid)),
                        name: crate::ChatName::from(format!("群聊:{}", event.group_openid)),
                        reply_target: build_reply_target_group(&event.group_openid),
                    },
                    sender: SenderInfo {
                        id: crate::SenderId::from(event.author.user_openid.clone()),
                        name: crate::SenderName::from(
                            event.author.username.unwrap_or(event.author.user_openid),
                        ),
                    },
                    content: event.content,
                })
                .await?;
        }
        _ => {}
    }
    Ok(())
}

// ── HTTP send helpers ──

async fn send_c2c_message(
    http: &Client,
    token: &str,
    openid: &str,
    content: &str,
) -> anyhow::Result<()> {
    http.post(format!("{API_BASE}/v2/users/{openid}/messages"))
        .header("Authorization", format!("QQBot {token}"))
        .json(&serde_json::json!({
            "content": content,
            "msg_type": 0,
            "msg_seq": 1,
        }))
        .send()
        .await
        .context("failed to send C2C message")?;
    Ok(())
}

async fn send_group_message(
    http: &Client,
    token: &str,
    group_openid: &str,
    content: &str,
) -> anyhow::Result<()> {
    http.post(format!("{API_BASE}/v2/groups/{group_openid}/messages"))
        .header("Authorization", format!("QQBot {token}"))
        .json(&serde_json::json!({
            "content": content,
            "msg_type": 0,
            "msg_seq": 1,
        }))
        .send()
        .await
        .context("failed to send group message")?;
    Ok(())
}

async fn send_c2c_input_notify(
    http: &Client,
    token: &str,
    openid: &str,
) -> anyhow::Result<()> {
    http.post(format!("{API_BASE}/v2/users/{openid}/messages"))
        .header("Authorization", format!("QQBot {token}"))
        .json(&serde_json::json!({
            "msg_type": 6,
            "input_notify": {
                "input_type": 1,
                "input_second": 60,
            },
            "msg_seq": 1,
        }))
        .send()
        .await
        .context("failed to send input notify")?;
    Ok(())
}
