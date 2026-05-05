//! WeiXin channel adapter — connects via openclaw-weixin gateway over HTTP.
//!
//! Uses QR code login, long-polling for events, and Bearer token authentication.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::{Mutex, mpsc};
use tracing::debug;

use crate::{Channel, ChannelInfo, ChatInfo, Event, ReplyTarget, Request, SenderInfo, entity};

/// Encode a WeiXin ReplyTarget: `weixin:{user_id}|{context_token}`
fn build_reply_target(user_id: &str, context_token: &str) -> ReplyTarget {
    ReplyTarget::from(format!("weixin:{user_id}|{context_token}"))
}

/// Decode a WeiXin ReplyTarget into (user_id, context_token).
fn parse_weixin_target(target: &ReplyTarget) -> Option<(&str, &str)> {
    target
        .as_str()
        .strip_prefix("weixin:")
        .map(|s| s.split_once('|').unwrap_or((s, "")))
}

/// Shared mutable state.
struct ChannelState {
    credentials: Option<WeiXinCredentials>,
}

/// Credentials obtained after QR code login.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct WeiXinCredentials {
    bot_token: String,
    base_url: String,
    ilink_user_id: String,
    ilink_bot_id: String,
}

/// WeiXin channel implementing [`Channel`].
pub struct WeiXinChannel {
    name: String,
    http: Client,
    base_url: String,
    state: Arc<Mutex<ChannelState>>,
}

impl WeiXinChannel {
    pub fn new(name: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            http: Client::new(),
            base_url: base_url.into(),
            state: Arc::new(Mutex::new(ChannelState {
                credentials: None,
            })),
        }
    }

    /// Perform QR code login and return credentials.
    async fn login(base_url: &str, http: &Client) -> anyhow::Result<WeiXinCredentials> {
        // 1. Get QR code
        let qr_url = format!("{base_url}/ilink/bot/get_bot_qrcode?bot_type=3");
        let qr_resp: QrCodeResponse = http
            .get(&qr_url)
            .header("iLink-App-Id", "bot")
            .header("iLink-App-ClientVersion", CLIENT_VERSION_HEADER)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .context("failed to get QR code (timeout?)")?
            .json()
            .await
            .context("failed to parse QR response")?;

        tracing::info!(target: "weixin", "请用微信扫描二维码: {}", qr_resp.qrcode_img_content);

        // 2. Poll for scan
        let status_url = format!(
            "{base_url}/ilink/bot/get_qrcode_status?qrcode={}",
            qr_resp.qrcode
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(480);
        let mut current_base_url = base_url.to_owned();
        let mut scanned = false;

        loop {
            if std::time::Instant::now() > deadline {
                anyhow::bail!("登录超时（8分钟），请重启");
            }

            let resp = http
                .get(&format!("{current_base_url}/ilink/bot/get_qrcode_status?qrcode={}", qr_resp.qrcode))
                .header("iLink-App-Id", "bot")
            .header("iLink-App-ClientVersion", CLIENT_VERSION_HEADER)
                .timeout(Duration::from_secs(35))
                .send()
                .await;

            let status: QrCodeStatus = match resp {
                Ok(r) => match r.json().await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(target: "weixin", "failed to parse QR status: {e}");
                        continue;
                    }
                },
                Err(_) => continue,
            };

            match status.status.as_str() {
                "wait" => {}
                "scaned" => {
                    if !scanned {
                        tracing::info!(target: "weixin", "已扫码，请在手机上确认");
                        scanned = true;
                    }
                }
                "scaned_but_redirect" => {
                    if let Some(ref host) = status.redirect_host {
                        current_base_url = format!("https://{host}");
                        tracing::info!(target: "weixin", "重定向到 {current_base_url}");
                    }
                }
                "expired" => anyhow::bail!("二维码已过期，请重启"),
                "confirmed" => {
                    let bot_token = status
                        .bot_token
                        .ok_or_else(|| anyhow::anyhow!("missing bot_token"))?;
                    let ilink_bot_id = status
                        .ilink_bot_id
                        .ok_or_else(|| anyhow::anyhow!("missing ilink_bot_id"))?;
                    return Ok(WeiXinCredentials {
                        bot_token,
                        base_url: current_base_url,
                        ilink_user_id: status.ilink_user_id.unwrap_or_default(),
                        ilink_bot_id,
                    });
                }
                _ => {}
            }

            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// Verify stored credentials are still valid.
    async fn verify_credentials(&self) -> anyhow::Result<bool> {
        let state = self.state.lock().await;
        let Some(ref creds) = state.credentials else {
            return Ok(false);
        };

        let resp = match self
            .http
            .post(format!("{}/ilink/bot/getconfig", creds.base_url))
            .header("Content-Type", "application/json")
            .header("AuthorizationType", "ilink_bot_token")
            .header("Authorization", format!("Bearer {}", creds.bot_token))
            .header("X-WECHAT-UIN", &random_uin())
            .header("iLink-App-Id", "bot")
            .header("iLink-App-ClientVersion", CLIENT_VERSION_HEADER)
            .json(&serde_json::json!({"ilink_user_id": creds.ilink_user_id}))
            .send()
            .await
        {
            Ok(r) => r,
            Err(_) => return Ok(false),
        };

        if !resp.status().is_success() {
            return Ok(false);
        }
        // Also check errcode in the response body
        if let Ok(body) = resp.text().await {
            if let Ok(v) = serde_json::from_str::<Value>(&body) {
                if v.get("errcode").and_then(Value::as_i64).unwrap_or(0) != 0 {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// Build common headers for all API requests. UIN is regenerated per-request
    /// (matches the reference's `randomWechatUin()` in `buildHeaders()`).
    async fn api_headers(&self) -> anyhow::Result<(String, String, String, String)> {
        let state = self.state.lock().await;
        let creds = state
            .credentials
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("weixin not logged in"))?;
        Ok((
            creds.base_url.clone(),
            creds.bot_token.clone(),
            random_uin(),
            creds.ilink_user_id.clone(),
        ))
    }

    /// Run the long-poll loop.
    async fn run_poll_loop(
        http: Client,
        base_url: String,
        state: Arc<Mutex<ChannelState>>,
        event_tx: mpsc::Sender<Event>,
    ) {
        let mut cursor = String::new();

        loop {
            // Re-login if credentials cleared (e.g. session timeout)
            {
                let s = state.lock().await;
                if s.credentials.is_none() {
                    drop(s);
                    match Self::login(&base_url, &http).await {
                        Ok(creds) => {
                            state.lock().await.credentials = Some(creds);
                            tracing::info!(target: "weixin", "re-login succeeded");
                        }
                        Err(e) => {
                            tracing::error!(target: "weixin", "re-login failed: {e}");
                            tokio::time::sleep(Duration::from_secs(30)).await;
                            continue;
                        }
                    }
                }
            }

            let (base_url, token, _ilink_user_id) = {
                let s = state.lock().await;
                let Some(ref creds) = s.credentials else {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                };
                (
                    creds.base_url.clone(),
                    creds.bot_token.clone(),
                    creds.ilink_user_id.clone(),
                )
            };

            let resp = match http
                .post(format!("{base_url}/ilink/bot/getupdates"))
                .header("Content-Type", "application/json")
                .header("AuthorizationType", "ilink_bot_token")
                .header("Authorization", format!("Bearer {token}"))
                .header("X-WECHAT-UIN", &random_uin())
                .header("iLink-App-Id", "bot")
                .header("iLink-App-ClientVersion", CLIENT_VERSION_HEADER)
                .json(&serde_json::json!({
                    "get_updates_buf": cursor,
                    "base_info": { "channel_version": "2.1.7" }
                }))
                .timeout(Duration::from_secs(40))
                .send()
                .await
            {
                Ok(r) => r,
                Err(_) => {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            };

            let body = match resp.text().await {
                Ok(text) => {
                    // Check for session timeout or re-login
                    if let Ok(v) = serde_json::from_str::<Value>(&text) {
                        if v.get("errcode").and_then(Value::as_i64).unwrap_or(0) == -14 {
                            tracing::warn!(target: "weixin", "poll: session timeout, forcing re-login");
                            state.lock().await.credentials = None;
                            tokio::time::sleep(Duration::from_secs(2)).await;
                            continue;
                        }
                    }
                    match serde_json::from_str::<GetUpdatesResponse>(&text) {
                        Ok(b) => b,
                        Err(e) => {
                            tracing::warn!(target: "weixin", "failed to parse getUpdates: {e}, body: {text}");
                            tokio::time::sleep(Duration::from_secs(2)).await;
                            continue;
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(target: "weixin", "failed to read getUpdates body: {e}");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            };

            // Only update cursor on successful parse
            cursor = body.get_updates_buf.clone();

            for msg in &body.msgs {
                if msg.message_type != 1 || msg.message_state != 2 {
                    continue;
                }
                let Some(from_id) = &msg.from_user_id else {
                    continue;
                };
                let ctx_token = msg.context_token.as_deref().unwrap_or("");
                tracing::debug!(target: "weixin", "incoming msg from={from_id} ctx={ctx_token}");

                for item in &msg.item_list {
                    if item.r#type == 1 {
                        if let Some(ref text_item) = item.text_item {
                            if let Err(e) = event_tx
                                .send(Event::IncomingMessage {
                                    chat: ChatInfo {
                                        id: crate::ChatId::from(format!("weixin:{from_id}")),
                                        name: crate::ChatName::from(from_id.clone()),
                                        reply_target: build_reply_target(from_id, &ctx_token),
                                        chat_type: crate::ChatType::Private,
                                    },
                                    sender: SenderInfo {
                                        id: crate::SenderId::from(from_id.clone()),
                                        name: crate::SenderName::from(from_id.clone()),
                                    },
                                    content: text_item.text.clone(),
                                })
                                .await
                            {
                                tracing::error!(target: "weixin", "failed to forward event: {e}");
                            }
                        }
                    }
                }
            }

            let sleep_ms = 35000 / 2;
            tokio::time::sleep(Duration::from_millis(sleep_ms as u64)).await;
        }
    }
}

#[async_trait::async_trait]
impl Channel for WeiXinChannel {
    async fn register(
        &self,
        event_tx: mpsc::Sender<Event>,
        app_db: Option<turso::Connection>,
    ) -> anyhow::Result<ChannelInfo> {
        debug!("registering WeiXin channel '{}'", self.name);
        // Persist credentials via entity table
        if let Some(ref db) = app_db {
            let _ = entity::create_table(db).await;
        }

        let mut creds: Option<WeiXinCredentials> = None;
        if let Some(ref db) = app_db {
            if let Ok(Some(json)) = entity::get(db, &self.name).await {
                creds = serde_json::from_str(&json).ok();
                if creds.is_some() {
                    tracing::info!(target: "weixin", "loaded cached credentials from DB");
                }
            }
        }
        debug!(
            "requiring lock to set credentials, current: {}",
            creds.is_some()
        );
        let mut state = self.state.lock().await;
        state.credentials = creds;
        let need_login = state.credentials.is_none();
        drop(state);

        if need_login {
            tracing::info!(target: "weixin", "weixin channel needs login, requesting QR code...");
            let creds = Self::login(&self.base_url, &self.http).await?;
            self.state.lock().await.credentials = Some(creds.clone());

            if let (Some(ref db), Ok(json)) = (app_db.as_ref(), serde_json::to_string(&creds)) {
                let _ = entity::upsert(db, &self.name, &json).await;
                tracing::info!(target: "weixin", "登录成功，凭证已持久化");
            }
        } else {
            let valid = match self.verify_credentials().await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(target: "weixin", "failed to verify credentials: {e}");
                    false
                }
            };
            if !valid {
                tracing::info!(target: "weixin", "cached credentials expired, re-logging in...");
                self.state.lock().await.credentials = None;
                let creds = Self::login(&self.base_url, &self.http).await?;
                self.state.lock().await.credentials = Some(creds.clone());

                if let (Some(ref db), Ok(json)) = (app_db.as_ref(), serde_json::to_string(&creds)) {
                    let _ = entity::upsert(db, &self.name, &json).await;
                }
                tracing::info!(target: "weixin", "重新登录成功");
            }
        }

        let state = self.state.lock().await;
        let expected_state = state.credentials.is_some();
        drop(state);

        if !expected_state {
            anyhow::bail!("weixin login failed");
        }

        tracing::info!(target: "weixin", "weixin connected");
        let http = self.http.clone();
        let poll_state = self.state.clone();
        let base_url_clone = self.base_url.clone();
        let name = self.name.clone();
        tokio::spawn(async move {
            Self::run_poll_loop(http, base_url_clone, poll_state, event_tx).await;
        });

        Ok(ChannelInfo {
            id: crate::ChannelId::from(name.as_str()),
            name: crate::ChannelName::from(name.as_str()),
        })
    }

    async fn send(&self, request: Request) -> anyhow::Result<()> {
        match request {
            Request::SendMessage { target, content } => {
                let (uid, ctx) = parse_weixin_target(&target)
                    .ok_or_else(|| anyhow::anyhow!("invalid weixin target: {target}"))?;
                let (base_url, token, uin, _) = self.api_headers().await?;

                let msg = {
                    let mut m = serde_json::json!({
                        "from_user_id": "",
                        "to_user_id": uid,
                        "client_id": next_client_id(),
                        "message_type": 2,
                        "message_state": 2,
                        "item_list": [{"type": 1, "text_item": {"text": content}}]
                    });
                    if !ctx.is_empty() {
                        m.as_object_mut()
                            .unwrap()
                            .insert("context_token".to_owned(), Value::String(ctx.to_owned()));
                    }
                    m
                };
                let body = serde_json::json!({
                    "msg": msg,
                    "base_info": {"channel_version": "1.0"}
                });

                let request_body = body.to_string();
                tracing::debug!(target: "weixin", "sendmessage to={uid} ctx={ctx} body={request_body}");

                let resp = self
                    .http
                    .post(format!("{base_url}/ilink/bot/sendmessage"))
                    .header("Content-Type", "application/json")
                    .header("AuthorizationType", "ilink_bot_token")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("X-WECHAT-UIN", &uin)
                    .header("iLink-App-Id", "bot")
            .header("iLink-App-ClientVersion", CLIENT_VERSION_HEADER)
                    .json(&body)
                    .send()
                    .await
                    .context("failed to send weixin message")?;

                let status = resp.status();
                let resp_body = resp.text().await.unwrap_or_default();
                tracing::debug!(target: "weixin", "sendmessage status={status} body={resp_body}");
                if !status.is_success() {
                    anyhow::bail!("weixin sendmessage failed (status={status}): {body}");
                }
                // Check for API-level error
                if let Ok(v) = serde_json::from_str::<Value>(&resp_body) {
                    let errcode = v.get("errcode").and_then(Value::as_i64).unwrap_or(0);
                    if errcode == -14 {
                        tracing::warn!(target: "weixin", "session timeout, forcing re-login");
                        self.state.lock().await.credentials = None;
                        anyhow::bail!("weixin session timeout");
                    } else if errcode != 0 {
                        let errmsg = v.get("errmsg").and_then(Value::as_str).unwrap_or("unknown");
                        anyhow::bail!("weixin API error (errcode={errcode}): {errmsg}");
                    }
                }
            }
            Request::StartTyping { target } => {
                let (uid, _) = parse_weixin_target(&target)
                    .ok_or_else(|| anyhow::anyhow!("invalid weixin target: {target}"))?;
                let (base_url, token, uin, _) = self.api_headers().await?;

                let _ = self
                    .http
                    .post(format!("{base_url}/ilink/bot/sendtyping"))
                    .header("Content-Type", "application/json")
                    .header("AuthorizationType", "ilink_bot_token")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("X-WECHAT-UIN", &uin)
                    .header("iLink-App-Id", "bot")
                    .header("iLink-App-ClientVersion", CLIENT_VERSION_HEADER)
                    .json(&serde_json::json!({
                        "ilink_user_id": uid,
                        "status": 1,
                        "base_info": {"channel_version": "1.0"}
                    }))
                    .send()
                    .await;
            }
            Request::StopTyping { target } => {
                let (uid, _) = parse_weixin_target(&target)
                    .ok_or_else(|| anyhow::anyhow!("invalid weixin target: {target}"))?;
                let (base_url, token, uin, _) = self.api_headers().await?;

                let _ = self
                    .http
                    .post(format!("{base_url}/ilink/bot/sendtyping"))
                    .header("Content-Type", "application/json")
                    .header("AuthorizationType", "ilink_bot_token")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("X-WECHAT-UIN", &uin)
                    .header("iLink-App-Id", "bot")
                    .header("iLink-App-ClientVersion", CLIENT_VERSION_HEADER)
                    .json(&serde_json::json!({
                        "ilink_user_id": uid,
                        "status": 2,
                        "base_info": {"channel_version": "1.0"}
                    }))
                    .send()
                    .await;
            }
        }
        Ok(())
    }
}

/// iLink-App-ClientVersion: uint32 encoded as 0x00MMNNPP (major<<16 | minor<<8 | patch).
/// Kept in sync with openclaw-weixin's version.
const CLIENT_VERSION_HEADER: &str = "131335"; // "2.1.7"

/// Random uint32 as base64, per API spec.
/// Per API spec: random uint32 → decimal string → base64.
fn random_uin() -> String {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let n: u32 = rand::random();
    STANDARD.encode(n.to_string().as_bytes())
}

/// Matches the reference `generateId("openclaw-weixin")` format:
/// `prefix:{timestamp}-{random_hex}`
fn next_client_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let rand_hex: u32 = rand::random();
    format!("openclaw-weixin:{ts}-{rand_hex:08x}")
}

// ── API response types ──

#[derive(Deserialize, Default)]
struct QrCodeStatus {
    status: String,
    #[serde(default)]
    bot_token: Option<String>,
    #[serde(default)]
    ilink_bot_id: Option<String>,
    #[serde(default)]
    ilink_user_id: Option<String>,
    #[serde(default)]
    redirect_host: Option<String>,
}

#[derive(Deserialize)]
struct QrCodeResponse {
    qrcode: String,
    qrcode_img_content: String,
}

#[derive(Deserialize, Default)]
struct GetUpdatesResponse {
    msgs: Vec<WeiXinMessage>,
    get_updates_buf: String,
    #[serde(alias = "sync_buf")]
    #[allow(dead_code)]
    sync_buf: String,
}

#[derive(Deserialize)]
struct WeiXinMessage {
    from_user_id: Option<String>,
    message_type: i32,
    message_state: i32,
    context_token: Option<String>,
    item_list: Vec<MessageItem>,
}

#[derive(Deserialize)]
struct MessageItem {
    r#type: i32,
    text_item: Option<TextItem>,
}

#[derive(Deserialize)]
struct TextItem {
    text: String,
}
