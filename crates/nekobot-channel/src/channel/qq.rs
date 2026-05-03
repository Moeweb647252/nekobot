// QQ Bot Official API channel adapter.
//
// Connects to the QQ Open Platform Bot API via WebSocket gateway for event
// ingestion and HTTP API for message delivery.

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

// ── API endpoints ──

/// Token exchange endpoint
const TOKEN_URL: &str = "https://bots.qq.com/app/getAppAccessToken";
/// HTTP API base (non-sandbox)
const API_BASE: &str = "https://api.sgroup.qq.com";
/// Gateway URL discovery endpoint
const GATEWAY_PATH: &str = "/gateway";

// ── Gateway intents ──
// Bitmask sent during Identify to declare which event types the client wants to receive.

/// Guild public channel messages (public domain)
const INTENT_PUBLIC_GUILD_MESSAGES: u32 = 1 << 30;
/// Guild direct messages
const INTENT_DIRECT_MESSAGE: u32 = 1 << 12;
/// Group chat + C2C private chat
const INTENT_GROUP_AND_C2C: u32 = 1 << 25;
/// Button/menu interaction callbacks
const INTENT_INTERACTION: u32 = 1 << 26;
/// Full intents: groups, C2C, guild messages, and interactions
const FULL_INTENTS: u32 = INTENT_PUBLIC_GUILD_MESSAGES
    | INTENT_DIRECT_MESSAGE
    | INTENT_GROUP_AND_C2C
    | INTENT_INTERACTION;

// ── QQChannel ──

/// QQ Bot channel adapter implementing the [`Channel`] trait.
///
/// Holds an HTTP client and a mutex-protected token cache. On `register()` it
/// spawns a WebSocket event loop; `send()` pushes messages via HTTP API.
pub struct QQChannel {
    name: String,
    app_id: String,
    client_secret: String,
    http: Client,
    state: Arc<Mutex<ChannelState>>,
}

/// Shared mutable state protected by `Arc<Mutex<_>>`.
struct ChannelState {
    token: Option<CachedToken>,
}

/// Cached access token with expiration timestamp.
struct CachedToken {
    value: String,
    expires_at: Instant,
}

impl QQChannel {
    /// Create a QQ channel adapter.
    ///
    /// `name` is the user-defined channel identifier from config.
    /// `app_id` and `client_secret` are obtained from the QQ Open Platform console.
    pub fn new(
        name: impl Into<String>,
        app_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            app_id: app_id.into(),
            client_secret: client_secret.into(),
            http: Client::new(),
            state: Arc::new(Mutex::new(ChannelState { token: None })),
        }
    }

    /// Fetch or return a cached access token.
    ///
    /// Returns the cached token if it is still valid with at least 60s headroom.
    /// Otherwise requests a new token from the QQ Open Platform, caches it, and
    /// returns it.
    async fn get_token(&self) -> anyhow::Result<String> {
        let mut state = self.state.lock().await;

        if let Some(cached) = &state.token {
            if cached.expires_at > Instant::now() + Duration::from_secs(60) {
                return Ok(cached.value.clone());
            }
        }

        let response = self
            .http
            .post(TOKEN_URL)
            .json(&serde_json::json!({
                "appId": self.app_id,
                "clientSecret": self.client_secret,
            }))
            .send()
            .await
            .context("failed to fetch access token")?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("failed to read token response body")?;

        let resp: TokenResponse = serde_json::from_str(&body).with_context(|| {
            format!("failed to parse access token response (status={status}): {body}")
        })?;

        let token = resp.access_token.clone();
        let expires_in = resp.expires_in.unwrap_or(7200).max(60);
        state.token = Some(CachedToken {
            value: token.clone(),
            expires_at: Instant::now() + Duration::from_secs(expires_in),
        });

        Ok(token)
    }

    /// Discover the WebSocket gateway URL using the access token.
    ///
    /// Exchange the access token for an actual WSS URL.
    async fn get_gateway_url(&self, token: &str) -> anyhow::Result<String> {
        let response = self
            .http
            .get(format!("{API_BASE}{GATEWAY_PATH}"))
            .header("Authorization", format!("QQBot {token}"))
            .send()
            .await
            .context("failed to get gateway url")?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("failed to read gateway response body")?;

        let resp: GatewayResponse = serde_json::from_str(&body).with_context(|| {
            format!("failed to parse gateway response (status={status}): {body}")
        })?;

        Ok(resp.url)
    }

    /// Clear the cached token so the next `get_token` call re-fetches.
    async fn clear_token(&self) {
        self.state.lock().await.token = None;
    }

    /// Send a request with the given token (used for retry after token refresh).
    async fn send_with_token(&self, token: &str, request: &Request) -> anyhow::Result<()> {
        match request {
            Request::SendMessage { target, content } => {
                if is_markdown(content) {
                    if let Some(openid) = parse_c2c_target(target) {
                        send_c2c_markdown(&self.http, token, openid, content).await?;
                    } else if let Some(group_openid) = parse_group_target(target) {
                        send_group_markdown(&self.http, token, group_openid, content).await?;
                    } else {
                        anyhow::bail!("unknown QQ target format: {target}");
                    }
                } else if let Some(openid) = parse_c2c_target(target) {
                    send_c2c_message(&self.http, token, openid, content).await?;
                } else if let Some(group_openid) = parse_group_target(target) {
                    send_group_message(&self.http, token, group_openid, content).await?;
                } else {
                    anyhow::bail!("unknown QQ target format: {target}");
                }
            }
            Request::StartTyping { target } => {
                if let Some(openid) = parse_c2c_target(target) {
                    if let Err(e) = send_c2c_input_notify(&self.http, token, openid).await {
                        tracing::debug!(target: "qqbot", "input notify failed: {e}");
                    }
                }
            }
            Request::StopTyping { .. } => {}
        }
        Ok(())
    }
}

// ── API response models ──

/// Response from `POST /app/getAppAccessToken`
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default, deserialize_with = "deserialize_expires_in")]
    expires_in: Option<u64>,
}

fn deserialize_expires_in<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;
    use serde_json::Value;

    let v = Value::deserialize(deserializer)?;
    match v {
        Value::Number(n) => n
            .as_u64()
            .map(Some)
            .ok_or_else(|| de::Error::custom("invalid number for expires_in")),
        Value::String(s) => s
            .parse()
            .map(Some)
            .map_err(|_| de::Error::custom("invalid string for expires_in")),
        Value::Null => Ok(None),
        _ => Err(de::Error::custom(
            "expected number or string for expires_in",
        )),
    }
}

/// Response from `GET /gateway`
#[derive(Deserialize)]
struct GatewayResponse {
    url: String,
}

// ── WebSocket frame ──

/// QQ Bot Gateway message frame.
///
/// | op | meaning |
/// |----|---------|
/// | 0  | Dispatch (server pushes events) |
/// | 1  | Heartbeat (client → server) |
/// | 10 | Hello (server handshake) |
/// | 11 | Heartbeat ACK (server → client) |
#[derive(Deserialize)]
struct WSPayload {
    op: u32,
    #[serde(default)]
    d: Value,
    /// Sequence number for session resume
    #[serde(default)]
    s: Option<u64>,
    /// Event type name, e.g. `C2C_MESSAGE_CREATE`, `GROUP_AT_MESSAGE_CREATE`
    #[serde(default)]
    t: Option<String>,
}

// ── Event payloads ──

/// C2C private message event payload.
#[derive(Deserialize, Debug)]
struct C2CMessageEvent {
    id: String,
    content: String,
    timestamp: String,
    author: AuthorInfo,
}

/// Group @-message event payload.
#[derive(Deserialize, Debug)]
struct GroupMessageEvent {
    id: String,
    content: String,
    timestamp: String,
    group_openid: String,
    author: AuthorInfo,
}

/// Message author information.
#[derive(Deserialize, Debug)]
struct AuthorInfo {
    user_openid: String,
    #[serde(default)]
    username: Option<String>,
}

// ── ReplyTarget encoding / decoding ──
// ReplyTarget is a string newtype. We prefix it with the conversation type so
// that send() can route to the correct HTTP endpoint.

/// Encode a C2C target: `c2c:{openid}`
fn build_reply_target_c2c(openid: &str) -> ReplyTarget {
    ReplyTarget::from(format!("c2c:{openid}"))
}

/// Encode a group target: `group:{group_openid}`
fn build_reply_target_group(group_openid: &str) -> ReplyTarget {
    ReplyTarget::from(format!("group:{group_openid}"))
}

/// Decode the openid from a C2C target. Returns `None` if not a C2C target.
fn parse_c2c_target(target: &ReplyTarget) -> Option<&str> {
    target.as_str().strip_prefix("c2c:")
}

/// Decode the group_openid from a group target. Returns `None` if not a group target.
fn parse_group_target(target: &ReplyTarget) -> Option<&str> {
    target.as_str().strip_prefix("group:")
}

// ── Error types ──

/// Sentinel error returned by HTTP send helpers when the server responds 401.
/// The caller (`send()`) uses downcast to detect this and trigger a token refresh.
#[derive(Debug)]
struct TokenExpiredError;

impl std::fmt::Display for TokenExpiredError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "QQ bot access token has expired")
    }
}

impl std::error::Error for TokenExpiredError {}

// ── Channel trait impl ──

#[async_trait::async_trait]
impl Channel for QQChannel {
    /// Register the channel: fetch token → discover gateway → spawn event loop.
    ///
    /// The WebSocket event loop runs in a background tokio task. Incoming
    /// messages are forwarded through `event_tx` as [`Event::IncomingMessage`].
    /// Returns [`ChannelInfo`] immediately without waiting for connection.
    async fn register(&self, event_tx: mpsc::Sender<Event>, _app_db: Option<turso::Connection>) -> anyhow::Result<ChannelInfo> {
        let state = self.state.clone();
        let app_id = self.app_id.clone();
        let client_secret = self.client_secret.clone();
        let http = self.http.clone();
        let event_tx_ws = event_tx.clone();

        let token = self.get_token().await?;
        let gateway_url = self.get_gateway_url(&token).await?;

        tokio::spawn(async move {
            if let Err(e) = run_gateway_loop(
                &http,
                &app_id,
                &client_secret,
                &gateway_url,
                &token,
                state,
                event_tx_ws,
            )
            .await
            {
                tracing::error!(target: "qqbot", "gateway loop exited: {e}");
            }
        });

        Ok(ChannelInfo {
            id: crate::ChannelId::from(self.app_id.as_str()),
            name: crate::ChannelName::from(self.name.as_str()),
        })
    }

    /// Send a message or typing indicator via HTTP API.
    ///
    /// Routes based on the [`ReplyTarget`] prefix:
    /// - `c2c:{openid}` → `POST /v2/users/{openid}/messages`
    /// - `group:{group_openid}` → `POST /v2/groups/{group_openid}/messages`
    ///
    /// `StartTyping` sends an input_notify (C2C only; no-op for groups).
    /// `StopTyping` is a no-op (QQ has no stop-typing API).
    async fn send(&self, request: Request) -> anyhow::Result<()> {
        let token = self.get_token().await?;
        if let Err(e) = self.send_with_token(&token, &request).await {
            if e.downcast_ref::<TokenExpiredError>().is_some() {
                tracing::info!(target: "qqbot", "token expired, refreshing and retrying");
                self.clear_token().await;
                let new_token = self.get_token().await?;
                self.send_with_token(&new_token, &request).await?;
            } else {
                return Err(e);
            }
        }
        Ok(())
    }
}

// ── Gateway event loop ──

/// Fetch a new access token via HTTP. Returns `(token, expires_in)`.
async fn fetch_token(
    http: &Client,
    app_id: &str,
    client_secret: &str,
) -> anyhow::Result<(String, Option<u64>)> {
    let resp: TokenResponse = http
        .post(TOKEN_URL)
        .json(&serde_json::json!({
            "appId": app_id,
            "clientSecret": client_secret,
        }))
        .send()
        .await
        .context("failed to fetch access token")?
        .json()
        .await
        .context("failed to parse access token response")?;
    Ok((resp.access_token, resp.expires_in))
}

/// Fetch the WebSocket gateway URL.
async fn fetch_gateway_url(http: &Client, token: &str) -> anyhow::Result<String> {
    let response = http
        .get(format!("{API_BASE}{GATEWAY_PATH}"))
        .header("Authorization", format!("QQBot {token}"))
        .send()
        .await
        .context("failed to get gateway url")?;

    let status = response.status();
    let body = response
        .text()
        .await
        .context("failed to read gateway response body")?;

    let resp: GatewayResponse = serde_json::from_str(&body)
        .with_context(|| format!("failed to parse gateway response (status={status}): {body}"))?;

    Ok(resp.url)
}

/// WebSocket event loop with automatic reconnection.
///
/// On disconnect or error, waits with exponential backoff (1s → 2s → 5s → ... → 60s max),
/// re-fetches a fresh token and gateway URL, then reconnects.
async fn run_gateway_loop(
    http: &Client,
    app_id: &str,
    client_secret: &str,
    gateway_url: &str,
    token: &str,
    state: Arc<Mutex<ChannelState>>,
    event_tx: mpsc::Sender<Event>,
) -> anyhow::Result<()> {
    let mut current_token = token.to_owned();
    let mut current_gateway_url = gateway_url.to_owned();
    let mut attempt: u32 = 0;

    loop {
        tracing::info!(
            target: "qqbot",
            "connecting to gateway (attempt {attempt}): {current_gateway_url}"
        );
        match run_gateway_session(http, &current_gateway_url, &current_token, &event_tx).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                tracing::error!(target: "qqbot", "gateway session ended ({attempt}): {e:#}");
            }
        }

        // Exponential backoff: 1s, 2s, 5s, 10s, 20s, 40s, 60s (capped)
        let delay_secs = (1u64 << attempt.min(6)).min(60);
        tracing::info!(target: "qqbot", "reconnecting in {delay_secs}s (attempt {attempt})...");
        tokio::time::sleep(Duration::from_secs(delay_secs)).await;

        // Refresh token and gateway URL before reconnecting
        match fetch_token(http, app_id, client_secret).await {
            Ok((token, expires_in)) => {
                let expires_in = expires_in.unwrap_or(7200).max(60);
                state.lock().await.token = Some(CachedToken {
                    value: token.clone(),
                    expires_at: Instant::now() + Duration::from_secs(expires_in),
                });
                current_token = token;
            }
            Err(e) => {
                tracing::error!(target: "qqbot", "failed to refresh token: {e}");
                attempt += 1;
                continue;
            }
        }
        match fetch_gateway_url(http, &current_token).await {
            Ok(url) => current_gateway_url = url,
            Err(e) => {
                tracing::error!(target: "qqbot", "failed to refresh gateway url: {e}");
                attempt += 1;
                continue;
            }
        }

        attempt += 1;
    }
}

/// Single gateway session — returns when the connection drops.
async fn run_gateway_session(
    _http: &Client,
    gateway_url: &str,
    token: &str,
    event_tx: &mpsc::Sender<Event>,
) -> anyhow::Result<()> {
    let (ws, _resp) = tokio_tungstenite::connect_async(gateway_url)
        .await
        .with_context(|| format!("failed to connect to QQ gateway: {gateway_url}"))?;

    use std::pin::pin;
    let mut ws = pin!(ws);
    let current_token = token.to_owned();
    let mut heartbeat_interval = Duration::from_secs(41);

    loop {
        tokio::select! {
            msg = ws.next() => {
                let msg = match msg {
                    Some(Ok(msg)) => msg,
                    Some(Err(e)) => {
                        tracing::error!(target: "qqbot", "ws error: {e}");
                        break;
                    }
                    None => break,
                };

                match msg {
                    Message::Text(text) => {
                        let Ok(payload) = serde_json::from_str::<WSPayload>(&text) else {
                            tracing::debug!(target: "qqbot", "unparseable ws payload: {text}");
                            continue;
                        };

                        match payload.op {
                            10 => {
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
                                if let Err(e) = dispatch_event(t, payload.d, event_tx).await {
                                    tracing::error!(target: "qqbot", "dispatch error: {e}");
                                }
                            }
                            11 => {}
                            _ => {}
                        }
                    }
                    Message::Close(_) => break,
                    Message::Ping(data) => {
                        if let Err(e) = ws.send(Message::Pong(data)).await {
                            tracing::debug!(target: "qqbot", "failed to send pong: {e}");
                        }
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

/// Convert a gateway dispatch payload into [`Event::IncomingMessage`].
///
/// Supported event types:
/// - `C2C_MESSAGE_CREATE` — C2C private message
/// - `GROUP_AT_MESSAGE_CREATE` — group @-message
///
/// The `ChatInfo.reply_target` carries a type prefix so `send()` can route
/// replies to the right HTTP endpoint.
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

/// Send a C2C private message.
///
/// `POST /v2/users/{openid}/messages` with `msg_type=0` (plain text).
/// `msg_seq` is hardcoded to 1 for non-reply scenarios.
async fn send_c2c_message(
    http: &Client,
    token: &str,
    openid: &str,
    content: &str,
) -> anyhow::Result<()> {
    let resp = http
        .post(format!("{API_BASE}/v2/users/{openid}/messages"))
        .header("Authorization", format!("QQBot {token}"))
        .json(&serde_json::json!({
            "content": content,
            "msg_type": 0,
            "msg_seq": 1,
        }))
        .send()
        .await
        .context("failed to send C2C message")?;

    if resp.status().as_u16() == 401 {
        return Err(anyhow::Error::new(TokenExpiredError));
    }
    Ok(())
}

/// Send a group message.
///
/// `POST /v2/groups/{group_openid}/messages` with `msg_type=0` (plain text).
async fn send_group_message(
    http: &Client,
    token: &str,
    group_openid: &str,
    content: &str,
) -> anyhow::Result<()> {
    let resp = http
        .post(format!("{API_BASE}/v2/groups/{group_openid}/messages"))
        .header("Authorization", format!("QQBot {token}"))
        .json(&serde_json::json!({
            "content": content,
            "msg_type": 0,
            "msg_seq": 1,
        }))
        .send()
        .await
        .context("failed to send group message")?;

    if resp.status().as_u16() == 401 {
        return Err(anyhow::Error::new(TokenExpiredError));
    }
    Ok(())
}

/// Detect if content contains markdown syntax that would benefit from `msg_type=2`.
fn is_markdown(content: &str) -> bool {
    content.contains("##")
        || content.contains("**")
        || content.starts_with("# ")
        || content.contains("\n- ")
        || content.contains("\n1. ")
        || content.contains("`")
}

/// Send a C2C markdown message.
///
/// `POST /v2/users/{openid}/messages` with `msg_type=2`.
async fn send_c2c_markdown(
    http: &Client,
    token: &str,
    openid: &str,
    content: &str,
) -> anyhow::Result<()> {
    let resp = http
        .post(format!("{API_BASE}/v2/users/{openid}/messages"))
        .header("Authorization", format!("QQBot {token}"))
        .json(&serde_json::json!({
            "msg_type": 2,
            "markdown": { "content": content },
            "msg_seq": 1,
        }))
        .send()
        .await
        .context("failed to send C2C markdown")?;

    if resp.status().as_u16() == 401 {
        return Err(anyhow::Error::new(TokenExpiredError));
    }
    Ok(())
}

/// Send a group markdown message.
///
/// `POST /v2/groups/{group_openid}/messages` with `msg_type=2`.
async fn send_group_markdown(
    http: &Client,
    token: &str,
    group_openid: &str,
    content: &str,
) -> anyhow::Result<()> {
    let resp = http
        .post(format!("{API_BASE}/v2/groups/{group_openid}/messages"))
        .header("Authorization", format!("QQBot {token}"))
        .json(&serde_json::json!({
            "msg_type": 2,
            "markdown": { "content": content },
            "msg_seq": 1,
        }))
        .send()
        .await
        .context("failed to send group markdown")?;

    if resp.status().as_u16() == 401 {
        return Err(anyhow::Error::new(TokenExpiredError));
    }
    Ok(())
}

/// Send a C2C typing indicator ("...is typing").
///
/// `msg_type=6` with `input_notify.input_type=1` shows a persistent typing
/// indicator for up to `input_second` seconds. Not supported for group chats.
async fn send_c2c_input_notify(http: &Client, token: &str, openid: &str) -> anyhow::Result<()> {
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
