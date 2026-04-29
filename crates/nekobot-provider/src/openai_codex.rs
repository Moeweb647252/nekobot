//! OpenAI Codex API provider implementation.
//!
//! This module provides [`OpenAiCodexProvider`], a [`Provider`] that communicates with
//! the OpenAI Codex completions API (chatgpt.com/backend-api/codex).

use std::{fmt, time::Duration};

use futures_util::StreamExt;
use nekobot_core::{
    agent::types::{ChatMessage, ChatResponse, Role, Usage},
    provider::{Provider, ProviderError, ProviderEvent, ProviderRequest},
};
use reqwest::{
    Client, StatusCode,
    header::{ACCEPT, HeaderMap, RETRY_AFTER},
};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc::Sender;

const DEFAULT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const RESERVED_BODY_KEYS: &[&str] = &[
    "model",
    "instructions",
    "input",
    "temperature",
    "top_p",
    "max_output_tokens",
    "stream",
    "tools",
    "tool_choice",
    "parallel_tool_calls",
    "store",
    "include",
];

/// OpenAI Codex API provider backed by the chatgpt.com/backend-api/codex endpoint.
pub struct OpenAiCodexProvider {
    access_token: String,
    account_id: Option<String>,
    model: String,
    base_url: String,
    client: Client,
}

impl OpenAiCodexProvider {
    /// Creates a new `OpenAiCodexProvider` with the given access token and model name.
    pub fn new(
        access_token: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self, ProviderError> {
        Self::from_config(access_token, None::<String>, model, None::<String>)
    }

    /// Creates a new `OpenAiCodexProvider` with full configuration including
    /// optional account ID and custom base URL.
    pub fn from_config(
        access_token: impl Into<String>,
        account_id: Option<impl Into<String>>,
        model: impl Into<String>,
        base_url: Option<impl Into<String>>,
    ) -> Result<Self, ProviderError> {
        let access_token = access_token.into();
        if access_token.trim().is_empty() {
            return Err(ProviderError::Authentication(
                "missing OpenAI Codex access token".to_owned(),
            ));
        }

        let model = model.into();
        if model.trim().is_empty() {
            return Err(ProviderError::InvalidRequest(
                "missing OpenAI Codex model".to_owned(),
            ));
        }

        let base_url = base_url
            .map(Into::into)
            .unwrap_or_else(|| DEFAULT_CODEX_BASE_URL.to_owned());
        if base_url.trim().is_empty() {
            return Err(ProviderError::InvalidRequest(
                "missing OpenAI Codex base URL".to_owned(),
            ));
        }

        let account_id = account_id.and_then(|value| {
            let value = value.into();
            (!value.trim().is_empty()).then_some(value)
        });

        Ok(Self {
            access_token,
            account_id,
            model,
            base_url: base_url.trim_end_matches('/').to_owned(),
            client: Client::new(),
        })
    }

    /// Returns the model name configured for this provider.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Returns the base URL of the Codex API endpoint.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn responses_url(&self) -> String {
        format!("{}/responses", self.base_url)
    }

    fn request_builder(&self, stream: bool) -> reqwest::RequestBuilder {
        let mut builder = self
            .client
            .post(self.responses_url())
            .bearer_auth(&self.access_token)
            .header("version", env!("CARGO_PKG_VERSION"));

        if let Some(account_id) = &self.account_id {
            builder = builder.header("ChatGPT-Account-ID", account_id);
        }

        if stream {
            builder = builder.header(ACCEPT, "text/event-stream");
        }

        builder
    }

    fn build_body(&self, request: &ProviderRequest, stream: bool) -> Result<Value, ProviderError> {
        validate_supported_request(request)?;

        let model = request
            .options
            .model
            .as_deref()
            .filter(|model| !model.trim().is_empty())
            .unwrap_or(&self.model);

        let mut body = Map::new();
        body.insert("model".to_owned(), Value::String(model.to_owned()));
        body.insert("input".to_owned(), chat_input(&request.chat.messages));
        body.insert("tools".to_owned(), Value::Array(Vec::new()));
        body.insert("tool_choice".to_owned(), Value::String("auto".to_owned()));
        body.insert("parallel_tool_calls".to_owned(), Value::Bool(false));
        body.insert("store".to_owned(), Value::Bool(false));
        body.insert("include".to_owned(), Value::Array(Vec::new()));

        if let Some(system_prompt) = &request.chat.system_prompt {
            body.insert(
                "instructions".to_owned(),
                Value::String(system_prompt.clone()),
            );
        }

        if let Some(temperature) = request.options.temperature {
            body.insert("temperature".to_owned(), json!(temperature));
        }

        if let Some(top_p) = request.options.top_p {
            body.insert("top_p".to_owned(), json!(top_p));
        }

        if let Some(max_output_tokens) = request.options.max_output_tokens {
            body.insert("max_output_tokens".to_owned(), json!(max_output_tokens));
        }

        for (key, value) in &request.options.extra {
            if !RESERVED_BODY_KEYS.contains(&key.as_str()) {
                body.insert(key.clone(), value.clone());
            }
        }

        if stream {
            body.insert("stream".to_owned(), Value::Bool(true));
        }

        Ok(Value::Object(body))
    }
}

impl fmt::Debug for OpenAiCodexProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiCodexProvider")
            .field("access_token", &"<redacted>")
            .field("account_id", &self.account_id)
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
impl Provider for OpenAiCodexProvider {
    /// Returns the provider identifier `"openai-codex"`.
    fn id(&self) -> &'static str {
        "openai-codex"
    }

    /// Sends a synchronous completion request to the Codex API and returns the full response.
    async fn complete(&self, request: ProviderRequest) -> Result<ChatResponse, ProviderError> {
        let body = self.build_body(&request, false)?;
        let response = self
            .request_builder(false)
            .json(&body)
            .send()
            .await
            .map_err(map_reqwest_error)?;

        if !response.status().is_success() {
            return Err(map_http_error(response).await);
        }

        let response = response
            .json::<Value>()
            .await
            .map_err(|error| ProviderError::Remote(error.to_string()))?;

        Ok(parse_response(&response))
    }

    /// Sends a streaming completion request to the Codex API, emitting
    /// [`ProviderEvent`]s as chunks arrive via the given channel.
    async fn stream(
        &self,
        request: ProviderRequest,
        events: Sender<ProviderEvent>,
    ) -> Result<ChatResponse, ProviderError> {
        let body = self.build_body(&request, true)?;
        let response = self
            .request_builder(true)
            .json(&body)
            .send()
            .await
            .map_err(map_reqwest_error)?;

        if !response.status().is_success() {
            return Err(map_http_error(response).await);
        }

        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        let mut content = String::new();
        let mut reasoning_content = String::new();
        let mut usage = None;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(map_reqwest_error)?;
            buffer.extend_from_slice(&chunk);

            while let Some((boundary, boundary_len)) = find_sse_boundary(&buffer) {
                let event = buffer[..boundary].to_vec();
                buffer.drain(..boundary + boundary_len);

                let Some(event) = parse_sse_event(&event)? else {
                    continue;
                };

                match event.kind.as_str() {
                    "response.created" => {
                        let _ = events.send(ProviderEvent::Started).await;
                    }
                    "response.output_text.delta" => {
                        if let Some(delta) = event.data.get("delta").and_then(Value::as_str) {
                            content.push_str(delta);
                            let _ = events
                                .send(ProviderEvent::ContentDelta(delta.to_owned()))
                                .await;
                        }
                    }
                    "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                        if let Some(delta) = event.data.get("delta").and_then(Value::as_str) {
                            reasoning_content.push_str(delta);
                            let _ = events
                                .send(ProviderEvent::ReasoningDelta(delta.to_owned()))
                                .await;
                        }
                    }
                    "response.output_item.done" => {
                        if let Some(item) = event.data.get("item") {
                            match item.get("type").and_then(Value::as_str) {
                                Some("message") => {
                                    if content.is_empty() {
                                        content = message_item_text(item);
                                    }
                                }
                                Some("reasoning") => {
                                    if reasoning_content.is_empty() {
                                        reasoning_content = reasoning_item_text(item);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    "response.completed" => {
                        if let Some(response) = event.data.get("response") {
                            if content.is_empty() {
                                content = response_text(response);
                            }
                            if reasoning_content.is_empty() {
                                reasoning_content = response_reasoning_text(response);
                            }
                            usage = parse_usage(response.get("usage"));
                        }
                        let _ = events
                            .send(ProviderEvent::Finished {
                                usage: usage.clone(),
                            })
                            .await;

                        return Ok(ChatResponse {
                            content,
                            reasoning_content: (!reasoning_content.is_empty())
                                .then_some(reasoning_content),
                            images: Vec::new(),
                            usage,
                        });
                    }
                    "response.failed" => {
                        return Err(ProviderError::Remote(response_error_message(&event.data)));
                    }
                    "response.incomplete" => {
                        return Err(ProviderError::Remote(response_incomplete_message(
                            &event.data,
                        )));
                    }
                    "error" => {
                        return Err(ProviderError::Remote(response_error_message(&event.data)));
                    }
                    _ => {}
                }
            }
        }

        if !buffer.is_empty() {
            return Err(ProviderError::Remote(
                "malformed server-sent event stream".to_owned(),
            ));
        }

        Err(ProviderError::Remote(
            "stream closed before response.completed".to_owned(),
        ))
    }
}

fn validate_supported_request(request: &ProviderRequest) -> Result<(), ProviderError> {
    if !request.chat.tools.is_empty() {
        return Err(ProviderError::UnsupportedFeature("tools".to_owned()));
    }

    if request
        .chat
        .messages
        .iter()
        .any(|message| !message.content.images.is_empty())
    {
        return Err(ProviderError::UnsupportedFeature("vision".to_owned()));
    }

    Ok(())
}

fn chat_input(messages: &[ChatMessage]) -> Value {
    Value::Array(messages.iter().map(chat_message_input).collect())
}

fn chat_message_input(message: &ChatMessage) -> Value {
    let (role, content) = match &message.role {
        Role::User => ("user", message.content.content.clone()),
        Role::Assistant => ("assistant", message.content.content.clone()),
        Role::Custom(role) if role == "system" || role == "developer" => {
            (role.as_str(), message.content.content.clone())
        }
        Role::Custom(role) => ("user", format!("[{role}] {}", message.content.content)),
    };
    let content_type = if role == "assistant" {
        "output_text"
    } else {
        "input_text"
    };

    json!({
        "type": "message",
        "role": role,
        "content": [
            {
                "type": content_type,
                "text": content,
            }
        ],
    })
}

fn parse_response(response: &Value) -> ChatResponse {
    let content = response
        .get("output_text")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| response_text(response));

    let reasoning_content = response_reasoning_text(response);

    ChatResponse {
        content,
        reasoning_content: (!reasoning_content.is_empty()).then_some(reasoning_content),
        images: Vec::new(),
        usage: parse_usage(response.get("usage")),
    }
}

fn response_text(response: &Value) -> String {
    response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
        .map(message_item_text)
        .collect()
}

fn response_reasoning_text(response: &Value) -> String {
    response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("reasoning"))
        .map(reasoning_item_text)
        .collect()
}

fn message_item_text(item: &Value) -> String {
    item.get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|content| {
            matches!(
                content.get("type").and_then(Value::as_str),
                None | Some("output_text")
            )
        })
        .filter_map(|content| content.get("text").and_then(Value::as_str))
        .collect()
}

fn reasoning_item_text(item: &Value) -> String {
    let summary_text = item
        .get("summary")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|summary| summary.get("text").and_then(Value::as_str));

    let reasoning_text = item
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|content| {
            matches!(
                content.get("type").and_then(Value::as_str),
                None | Some("reasoning_text")
            )
        })
        .filter_map(|content| content.get("text").and_then(Value::as_str));

    summary_text.chain(reasoning_text).collect()
}

fn parse_usage(usage: Option<&Value>) -> Option<Usage> {
    let usage = usage?;

    Some(Usage {
        input_tokens: usage.get("input_tokens").and_then(Value::as_u64),
        output_tokens: usage.get("output_tokens").and_then(Value::as_u64),
        total_tokens: usage.get("total_tokens").and_then(Value::as_u64),
    })
}

fn map_reqwest_error(error: reqwest::Error) -> ProviderError {
    if error.is_timeout() {
        ProviderError::Timeout(error.to_string())
    } else {
        ProviderError::Remote(error.to_string())
    }
}

async fn map_http_error(response: reqwest::Response) -> ProviderError {
    let status = response.status();
    let retry_after = retry_after(response.headers());
    let message = response
        .text()
        .await
        .ok()
        .and_then(|body| api_error_message(&body))
        .unwrap_or_else(|| {
            status
                .canonical_reason()
                .unwrap_or("request failed")
                .to_owned()
        });

    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => ProviderError::Authentication(message),
        StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimited {
            retry_after,
            message,
        },
        StatusCode::BAD_REQUEST => ProviderError::InvalidRequest(message),
        status if status.is_server_error() => ProviderError::Unavailable(message),
        _ => ProviderError::Remote(message),
    }
}

fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
}

fn api_error_message(body: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(body).ok()?;
    value
        .get("error")
        .and_then(|error| error.get("message").or(Some(error)))
        .and_then(Value::as_str)
        .or_else(|| value.get("message").and_then(Value::as_str))
        .map(ToOwned::to_owned)
}

struct SseEvent {
    kind: String,
    data: Value,
}

fn find_sse_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| (position, 4))
        .or_else(|| {
            buffer
                .windows(2)
                .position(|window| window == b"\n\n")
                .map(|position| (position, 2))
        })
}

fn parse_sse_event(bytes: &[u8]) -> Result<Option<SseEvent>, ProviderError> {
    let event = std::str::from_utf8(bytes)
        .map_err(|error| ProviderError::Remote(format!("invalid SSE event: {error}")))?;
    let mut kind = None;
    let mut data = Vec::new();

    for line in event.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with(':') {
            continue;
        }

        if let Some(value) = line.strip_prefix("event:") {
            kind = Some(value.trim().to_owned());
        } else if let Some(value) = line.strip_prefix("data:") {
            let value = value.trim_start();
            if value == "[DONE]" {
                return Ok(None);
            }
            data.push(value.to_owned());
        }
    }

    if data.is_empty() {
        return Ok(None);
    }

    let data = serde_json::from_str::<Value>(&data.join("\n"))
        .map_err(|error| ProviderError::Remote(format!("invalid SSE JSON event: {error}")))?;
    let kind = kind
        .or_else(|| {
            data.get("type")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "message".to_owned());

    Ok(Some(SseEvent { kind, data }))
}

fn response_error_message(event: &Value) -> String {
    event
        .get("response")
        .and_then(|response| response.get("error"))
        .and_then(|error| error.get("message").or(Some(error)))
        .and_then(Value::as_str)
        .or_else(|| {
            event
                .get("error")
                .and_then(|error| error.get("message").or(Some(error)))
                .and_then(Value::as_str)
        })
        .or_else(|| event.get("message").and_then(Value::as_str))
        .unwrap_or("OpenAI Codex response failed")
        .to_owned()
}

fn response_incomplete_message(event: &Value) -> String {
    event
        .get("response")
        .and_then(|response| response.get("incomplete_details"))
        .and_then(|details| details.get("reason"))
        .and_then(Value::as_str)
        .map(|reason| format!("OpenAI Codex response incomplete: {reason}"))
        .unwrap_or_else(|| "OpenAI Codex response incomplete".to_owned())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use nekobot_core::{
        agent::{
            tool::ToolSpec,
            types::{ChatMessageContent, ChatRequest},
        },
        provider::{ModelOptions, ProviderEvent},
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::{mpsc, oneshot},
    };

    use super::*;

    #[derive(Debug)]
    struct RecordedRequest {
        path: String,
        headers: HashMap<String, String>,
        body: Value,
    }

    async fn mock_server(
        status: u16,
        response_headers: &[(&str, &str)],
        response_body: &str,
    ) -> (String, oneshot::Receiver<RecordedRequest>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (request_sender, request_receiver) = oneshot::channel();
        let response_headers = response_headers
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect::<Vec<_>>();
        let response_body = response_body.to_owned();

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = Vec::new();
            let mut chunk = [0_u8; 1024];

            loop {
                let count = socket.read(&mut chunk).await.unwrap();
                if count == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..count]);
                if request_is_complete(&buffer) {
                    break;
                }
            }

            let request = parse_recorded_request(&buffer);
            let _ = request_sender.send(request);

            let reason = match status {
                200 => "OK",
                400 => "Bad Request",
                401 => "Unauthorized",
                429 => "Too Many Requests",
                500 => "Internal Server Error",
                _ => "Status",
            };
            let mut response = format!(
                "HTTP/1.1 {status} {reason}\r\ncontent-length: {}\r\nconnection: close\r\n",
                response_body.len()
            );
            for (key, value) in response_headers {
                response.push_str(&format!("{key}: {value}\r\n"));
            }
            response.push_str("\r\n");
            response.push_str(&response_body);
            socket.write_all(response.as_bytes()).await.unwrap();
        });

        (format!("http://{addr}"), request_receiver)
    }

    fn request_is_complete(buffer: &[u8]) -> bool {
        let Some(header_end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") else {
            return false;
        };
        let headers = String::from_utf8_lossy(&buffer[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case("content-length")
                    .then_some(value.trim())
            })
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        buffer.len() >= header_end + 4 + content_length
    }

    fn parse_recorded_request(buffer: &[u8]) -> RecordedRequest {
        let header_end = buffer
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap();
        let headers = String::from_utf8_lossy(&buffer[..header_end]);
        let mut lines = headers.lines();
        let request_line = lines.next().unwrap();
        let path = request_line.split_whitespace().nth(1).unwrap().to_owned();
        let headers = lines
            .filter_map(|line| {
                let (key, value) = line.split_once(':')?;
                Some((key.to_ascii_lowercase(), value.trim().to_owned()))
            })
            .collect();
        let body = serde_json::from_slice(&buffer[header_end + 4..]).unwrap();

        RecordedRequest {
            path,
            headers,
            body,
        }
    }

    fn provider(base_url: String) -> OpenAiCodexProvider {
        OpenAiCodexProvider::from_config(
            "access-token",
            Some("account-id"),
            "gpt-5.2-codex",
            Some(base_url),
        )
        .unwrap()
    }

    fn request_with_message(content: impl Into<String>) -> ProviderRequest {
        ProviderRequest {
            chat: ChatRequest {
                messages: vec![ChatMessage {
                    role: Role::User,
                    content: ChatMessageContent {
                        content: content.into(),
                        reasoning_content: None,
                        images: Vec::new(),
                    },
                }],
                system_prompt: Some("be useful".to_owned()),
                tools: Vec::new(),
            },
            options: ModelOptions::default(),
        }
    }

    #[tokio::test]
    async fn complete_sends_request_shape_and_auth_headers() {
        let (base_url, request_receiver) = mock_server(200, &[], r#"{"output_text":"ok"}"#).await;
        let provider = provider(base_url);

        let response = provider
            .complete(request_with_message("hello"))
            .await
            .unwrap();
        let request = request_receiver.await.unwrap();

        assert_eq!(response.content, "ok");
        assert_eq!(request.path, "/responses");
        assert_eq!(
            request.headers.get("authorization").map(String::as_str),
            Some("Bearer access-token")
        );
        assert_eq!(
            request
                .headers
                .get("chatgpt-account-id")
                .map(String::as_str),
            Some("account-id")
        );
        assert!(request.headers.contains_key("version"));
        assert_eq!(request.body["model"], "gpt-5.2-codex");
        assert_eq!(request.body["instructions"], "be useful");
        assert_eq!(request.body["tools"], json!([]));
        assert_eq!(request.body["tool_choice"], "auto");
        assert_eq!(request.body["parallel_tool_calls"], false);
        assert_eq!(request.body["store"], false);
        assert_eq!(request.body["include"], json!([]));
        assert_eq!(request.body["input"][0]["type"], "message");
        assert_eq!(request.body["input"][0]["role"], "user");
        assert_eq!(request.body["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(request.body["input"][0]["content"][0]["text"], "hello");
        assert!(request.body.get("stream").is_none());
    }

    #[tokio::test]
    async fn model_options_can_override_config_model() {
        let (base_url, request_receiver) = mock_server(200, &[], r#"{"output_text":"ok"}"#).await;
        let provider = provider(base_url);
        let mut request = request_with_message("hello");
        request.options.model = Some("gpt-5.1-codex".to_owned());

        provider.complete(request).await.unwrap();
        let request = request_receiver.await.unwrap();

        assert_eq!(request.body["model"], "gpt-5.1-codex");
    }

    #[test]
    fn constructor_requires_model() {
        let result = OpenAiCodexProvider::new("token", "");

        assert!(matches!(result, Err(ProviderError::InvalidRequest(_))));
    }

    #[test]
    fn debug_output_redacts_access_token() {
        let provider = OpenAiCodexProvider::new("secret-token", "gpt-5.2-codex").unwrap();
        let debug = format!("{provider:?}");

        assert!(!debug.contains("secret-token"));
        assert!(debug.contains("<redacted>"));
    }

    #[tokio::test]
    async fn complete_parses_output_text_and_usage() {
        let (base_url, _request_receiver) = mock_server(
            200,
            &[],
            r#"{
                "output_text": "done",
                "usage": {
                    "input_tokens": 3,
                    "output_tokens": 4,
                    "total_tokens": 7
                }
            }"#,
        )
        .await;
        let provider = provider(base_url);

        let response = provider
            .complete(request_with_message("hello"))
            .await
            .unwrap();

        assert_eq!(response.content, "done");
        assert_eq!(
            response.usage,
            Some(Usage {
                input_tokens: Some(3),
                output_tokens: Some(4),
                total_tokens: Some(7),
            })
        );
    }

    #[tokio::test]
    async fn complete_fallback_output_text_does_not_become_reasoning() {
        let (base_url, _request_receiver) = mock_server(
            200,
            &[],
            r#"{
                "output": [
                    {
                        "type": "message",
                        "content": [
                            { "type": "output_text", "text": "fallback text" }
                        ]
                    }
                ]
            }"#,
        )
        .await;
        let provider = provider(base_url);

        let response = provider
            .complete(request_with_message("hello"))
            .await
            .unwrap();

        assert_eq!(response.content, "fallback text");
        assert_eq!(response.reasoning_content, None);
    }

    #[tokio::test]
    async fn stream_emits_events_and_returns_accumulated_response() {
        let sse = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\"}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hel\"}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"lo\"}\n\n",
            "event: response.reasoning_summary_text.delta\n",
            "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"why\"}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n",
        );
        let (base_url, request_receiver) =
            mock_server(200, &[("content-type", "text/event-stream")], sse).await;
        let provider = provider(base_url);
        let (event_sender, mut event_receiver) = mpsc::channel(8);

        let response = provider
            .stream(request_with_message("hello"), event_sender)
            .await
            .unwrap();
        let request = request_receiver.await.unwrap();

        assert_eq!(request.body["stream"], true);
        assert_eq!(
            request.headers.get("accept").map(String::as_str),
            Some("text/event-stream")
        );
        assert_eq!(response.content, "hello");
        assert_eq!(response.reasoning_content.as_deref(), Some("why"));
        assert_eq!(
            response.usage,
            Some(Usage {
                input_tokens: Some(1),
                output_tokens: Some(2),
                total_tokens: Some(3),
            })
        );
        assert_eq!(event_receiver.recv().await, Some(ProviderEvent::Started));
        assert_eq!(
            event_receiver.recv().await,
            Some(ProviderEvent::ContentDelta("hel".to_owned()))
        );
        assert_eq!(
            event_receiver.recv().await,
            Some(ProviderEvent::ContentDelta("lo".to_owned()))
        );
        assert_eq!(
            event_receiver.recv().await,
            Some(ProviderEvent::ReasoningDelta("why".to_owned()))
        );
        assert_eq!(
            event_receiver.recv().await,
            Some(ProviderEvent::Finished {
                usage: response.usage.clone(),
            })
        );
    }

    #[tokio::test]
    async fn stream_uses_output_item_done_as_final_text_fallback() {
        let sse = concat!(
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"reasoning\",\"summary\":[{\"type\":\"summary_text\",\"text\":\"thinking\"}],\"content\":[{\"type\":\"reasoning_text\",\"text\":\" details\"}]}}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"final text\"}]}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n",
        );
        let (base_url, _request_receiver) =
            mock_server(200, &[("content-type", "text/event-stream")], sse).await;
        let provider = provider(base_url);
        let (event_sender, _event_receiver) = mpsc::channel(8);

        let response = provider
            .stream(request_with_message("hello"), event_sender)
            .await
            .unwrap();

        assert_eq!(response.content, "final text");
        assert_eq!(
            response.reasoning_content.as_deref(),
            Some("thinking details")
        );
    }

    #[tokio::test]
    async fn stream_errors_when_closed_before_completed() {
        let sse =
            concat!("data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",);
        let (base_url, _request_receiver) =
            mock_server(200, &[("content-type", "text/event-stream")], sse).await;
        let provider = provider(base_url);
        let (event_sender, _event_receiver) = mpsc::channel(8);

        let result = provider
            .stream(request_with_message("hello"), event_sender)
            .await;

        assert!(
            matches!(result, Err(ProviderError::Remote(message)) if message == "stream closed before response.completed")
        );
    }

    #[tokio::test]
    async fn http_errors_map_to_provider_errors() {
        let (base_url, _request_receiver) =
            mock_server(401, &[], r#"{"error":{"message":"bad token"}}"#).await;
        let result = provider(base_url)
            .complete(request_with_message("hello"))
            .await;
        assert!(
            matches!(result, Err(ProviderError::Authentication(message)) if message == "bad token")
        );

        let (base_url, _request_receiver) = mock_server(
            429,
            &[("retry-after", "5")],
            r#"{"error":{"message":"slow down"}}"#,
        )
        .await;
        let result = provider(base_url)
            .complete(request_with_message("hello"))
            .await;
        assert!(matches!(
            result,
            Err(ProviderError::RateLimited { retry_after: Some(duration), message })
                if duration == Duration::from_secs(5) && message == "slow down"
        ));

        let (base_url, _request_receiver) =
            mock_server(500, &[], r#"{"error":{"message":"upstream down"}}"#).await;
        let result = provider(base_url)
            .complete(request_with_message("hello"))
            .await;
        assert!(
            matches!(result, Err(ProviderError::Unavailable(message)) if message == "upstream down")
        );
    }

    #[tokio::test]
    async fn tools_are_not_supported_yet() {
        let provider = OpenAiCodexProvider::new("token", "gpt-5.2-codex").unwrap();
        let mut request = request_with_message("hello");
        request.chat.tools = vec![ToolSpec {
            name: "tool".to_owned(),
            description: "tool".to_owned(),
            parameters_schema: json!({ "type": "object" }),
        }];

        let result = provider.complete(request).await;

        assert!(matches!(
            result,
            Err(ProviderError::UnsupportedFeature(feature)) if feature == "tools"
        ));
    }
}
