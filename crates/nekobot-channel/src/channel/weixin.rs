//! WeiXin channel adapter — connects via openclaw-weixin gateway over HTTP.
//!
//! Uses QR code login, long-polling for events, and Bearer token authentication.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use reqwest::Client;
use serde::Deserialize;
use tokio::sync::{Mutex, mpsc};

use crate::{Channel, ChannelInfo, ChatInfo, Event, ReplyTarget, Request, SenderInfo, entity};

/// Encode a WeiXin ReplyTarget: `weixin:{user_id}|{context_token}`
fn build_reply_target(user_id: &str, context_token: &str) -> ReplyTarget {
    ReplyTarget::from(format!("weixin:{user_id}|{context_token}"))
}

/// Decode a WeiXin ReplyTarget into (user_id, context_token).
fn parse_weixin_target(target: &ReplyTarget) -> Option<(&str, &str)> {
    target.as_str().strip_prefix("weixin:").map(|s| {
        s.split_once('|').unwrap_or((s, ""))
    })
}

/// Shared mutable state.
struct ChannelState {
    credentials: Option<WeiXinCredentials>,
    uin_header: String,           // X-WECHAT-UIN base64 value
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
        let uin = random_uin();
        Self {
            name: name.into(),
            http: Client::new(),
            base_url: base_url.into(),
            state: Arc::new(Mutex::new(ChannelState {
                credentials: None,
                uin_header: uin,
            })),
        }
    }

    /// Perform QR code login and return credentials.
    async fn login(base_url: &str, http: &Client) -> anyhow::Result<WeiXinCredentials> {
        // 1. Get QR code
        let qr_url = format!("{base_url}/ilink/bot/get_bot_qrcode?bot_type=3");
        let qr_resp: QrCodeResponse = http
            .get(&qr_url)
            .send().await.context("failed to get QR code")?
            .json().await.context("failed to parse QR response")?;

        tracing::info!(target: "weixin", "请用微信扫描二维码: {}", qr_resp.qrcode_img_content);

        // 2. Poll for scan
        let status_url = format!("{base_url}/ilink/bot/get_qrcode_status?qrcode={}", qr_resp.qrcode);
        let deadline = std::time::Instant::now() + Duration::from_secs(480);
        let mut current_base_url = base_url.to_owned();
        let mut scanned = false;

        loop {
            if std::time::Instant::now() > deadline {
                anyhow::bail!("登录超时（8分钟），请重启");
            }

            let resp = http
                .get(&status_url.replace(base_url, &current_base_url))
                .timeout(Duration::from_secs(35))
                .send()
                .await;

            let status: QrCodeStatus = match resp {
                Ok(r) => r.json().await.unwrap_or(QrCodeStatus { status: "wait".into(), ..Default::default() }),
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
                    let bot_token = status.bot_token
                        .ok_or_else(|| anyhow::anyhow!("missing bot_token"))?;
                    let ilink_bot_id = status.ilink_bot_id
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
        let Some(ref creds) = state.credentials else { return Ok(false) };

        let resp = self.http
            .post(format!("{}/ilink/bot/getconfig", creds.base_url))
            .header("Content-Type", "application/json")
            .header("AuthorizationType", "ilink_bot_token")
            .header("Authorization", format!("Bearer {}", creds.bot_token))
            .header("X-WECHAT-UIN", &state.uin_header)
            .json(&serde_json::json!({"ilink_user_id": creds.ilink_user_id}))
            .send()
            .await;

        Ok(resp.is_ok() && resp.unwrap().status().is_success())
    }

    /// Build common headers for all API requests.
    async fn api_headers(&self) -> anyhow::Result<(String, String, String, String)> {
        let state = self.state.lock().await;
        let creds = state.credentials.as_ref()
            .ok_or_else(|| anyhow::anyhow!("weixin not logged in"))?;
        Ok((creds.base_url.clone(), creds.bot_token.clone(), state.uin_header.clone(), creds.ilink_user_id.clone()))
    }

    /// Run the long-poll loop.
    async fn run_poll_loop(
        http: Client,
        state: Arc<Mutex<ChannelState>>,
        event_tx: mpsc::Sender<Event>,
    ) {
        let mut cursor = String::new();

        loop {
            let (base_url, token, uin, _ilink_user_id) = {
                let s = state.lock().await;
                let Some(ref creds) = s.credentials else {
                    tracing::error!(target: "weixin", "poll loop: not logged in");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                };
                (creds.base_url.clone(), creds.bot_token.clone(), s.uin_header.clone(), creds.ilink_user_id.clone())
            };

            let resp = match http
                .post(format!("{base_url}/ilink/bot/getupdates"))
                .header("Content-Type", "application/json")
                .header("AuthorizationType", "ilink_bot_token")
                .header("Authorization", format!("Bearer {token}"))
                .header("X-WECHAT-UIN", &uin)
                .json(&serde_json::json!({
                    "get_updates_buf": cursor,
                    "base_info": { "channel_version": "1.0" }
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

            let body: GetUpdatesResponse = match resp.json().await {
                Ok(b) => b,
                Err(e) => {
                    tracing::error!(target: "weixin", "failed to parse getUpdates: {e}");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            };

            if body.errcode == Some(-14) {
                tracing::warn!(target: "weixin", "session expired, clearing credentials");
                state.lock().await.credentials = None;
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }

            if body.ret != 0 {
                cursor = body.get_updates_buf.unwrap_or_default();
                continue;
            }

            for msg in &body.msgs.unwrap_or_default() {
                if msg.message_type != Some(1) || msg.message_state != Some(2) {
                    continue;
                }
                let Some(ref items) = msg.item_list else { continue };
                let Some(from_id) = &msg.from_user_id else { continue };
                let ctx_token = msg.context_token.clone().unwrap_or_default();

                for item in items {
                    if item.r#type == Some(1) {
                        if let Some(ref text_item) = item.text_item {
                            let _ = event_tx.send(Event::IncomingMessage {
                                chat: ChatInfo {
                                    id: crate::ChatId::from(format!("weixin:{from_id}")),
                                    name: crate::ChatName::from(from_id.clone()),
                                    reply_target: build_reply_target(from_id, &ctx_token),
                                },
                                sender: SenderInfo {
                                    id: crate::SenderId::from(from_id.clone()),
                                    name: crate::SenderName::from(from_id.clone()),
                                },
                                content: text_item.text.clone(),
                            }).await;
                        }
                    }
                }
            }

            cursor = body.get_updates_buf.unwrap_or_default();
            let sleep_ms = body.longpolling_timeout_ms.unwrap_or(35000) / 2;
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
        } else if !self.verify_credentials().await.unwrap_or(false) {
            tracing::info!(target: "weixin", "cached credentials expired, re-logging in...");
            self.state.lock().await.credentials = None;
            let creds = Self::login(&self.base_url, &self.http).await?;
            self.state.lock().await.credentials = Some(creds.clone());

            if let (Some(ref db), Ok(json)) = (app_db.as_ref(), serde_json::to_string(&creds)) {
                let _ = entity::upsert(db, &self.name, &json).await;
            }
            tracing::info!(target: "weixin", "重新登录成功");
        }

        let state = self.state.lock().await;
        let name = self.name.clone();
        let uin = state.uin_header.clone();
        let expected_state = state.credentials.is_some();
        drop(state);

        if !expected_state {
            anyhow::bail!("weixin login failed");
        }

        tracing::info!(target: "weixin", "weixin connected (X-WECHAT-UIN: {uin})");
        let http = self.http.clone();
        let poll_state = self.state.clone();
        tokio::spawn(async move {
            Self::run_poll_loop(http, poll_state, event_tx).await;
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

                self.http
                    .post(format!("{base_url}/ilink/bot/sendmessage"))
                    .header("Content-Type", "application/json")
                    .header("AuthorizationType", "ilink_bot_token")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("X-WECHAT-UIN", &uin)
                    .json(&serde_json::json!({
                        "msg": {
                            "to_user_id": uid,
                            "context_token": ctx,
                            "item_list": [{"type": 1, "text_item": {"text": content}}]
                        },
                        "base_info": {"channel_version": "1.0"}
                    }))
                    .send().await.context("failed to send weixin message")?;
            }
            Request::StartTyping { target } => {
                let (_uid, _) = parse_weixin_target(&target)
                    .ok_or_else(|| anyhow::anyhow!("invalid weixin target: {target}"))?;
                let (base_url, token, uin, ilink_user_id) = self.api_headers().await?;

                let _ = self.http
                    .post(format!("{base_url}/ilink/bot/sendtyping"))
                    .header("Content-Type", "application/json")
                    .header("AuthorizationType", "ilink_bot_token")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("X-WECHAT-UIN", &uin)
                    .json(&serde_json::json!({
                        "ilink_user_id": ilink_user_id,
                        "status": 1,
                        "base_info": {"channel_version": "1.0"}
                    }))
                    .send().await;
            }
            Request::StopTyping { target } => {
                let (_uid, _) = parse_weixin_target(&target)
                    .ok_or_else(|| anyhow::anyhow!("invalid weixin target: {target}"))?;
                let (base_url, token, uin, ilink_user_id) = self.api_headers().await?;

                let _ = self.http
                    .post(format!("{base_url}/ilink/bot/sendtyping"))
                    .header("Content-Type", "application/json")
                    .header("AuthorizationType", "ilink_bot_token")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("X-WECHAT-UIN", &uin)
                    .json(&serde_json::json!({
                        "ilink_user_id": ilink_user_id,
                        "status": 2,
                        "base_info": {"channel_version": "1.0"}
                    }))
                    .send().await;
            }
        }
        Ok(())
    }
}

fn random_uin() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u32;
    // Simple base64-like encode for a random u32 header value
    let bytes = n.to_le_bytes();
    let chars: Vec<u8> = (0..=63).map(|i| {
        if i < 26 { b'A' + i }
        else if i < 52 { b'a' + i - 26 }
        else if i < 62 { b'0' + i - 52 }
        else if i == 62 { b'+' }
        else { b'/' }
    }).collect();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(chars[(n >> 18) as usize] as char);
        out.push(chars[((n >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 { out.push(chars[((n >> 6) & 0x3f) as usize] as char); }
        if chunk.len() > 2 { out.push(chars[(n & 0x3f) as usize] as char); }
    }
    out
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
    ret: i32,
    #[serde(default)]
    errcode: Option<i32>,
    #[serde(default)]
    msgs: Option<Vec<WeiXinMessage>>,
    #[serde(default)]
    get_updates_buf: Option<String>,
    #[serde(default)]
    longpolling_timeout_ms: Option<i32>,
}

#[derive(Deserialize)]
struct WeiXinMessage {
    #[serde(default)]
    from_user_id: Option<String>,
    #[serde(default)]
    message_type: Option<i32>,
    #[serde(default)]
    message_state: Option<i32>,
    #[serde(default)]
    context_token: Option<String>,
    #[serde(default)]
    item_list: Option<Vec<MessageItem>>,
}

#[derive(Deserialize)]
struct MessageItem {
    #[serde(default)]
    r#type: Option<i32>,
    #[serde(default)]
    text_item: Option<TextItem>,
}

#[derive(Deserialize)]
struct TextItem {
    text: String,
}
