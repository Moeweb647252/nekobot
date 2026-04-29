//! DeepSeek API provider implementation.

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

const DEFAULT_DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com";
const RESERVED_BODY_KEYS: &[&str] = &[
    "model",
    "messages",
    "temperature",
    "top_p",
    "max_tokens",
    "max_output_tokens",
    "stream",
    "tools",
    "tool_choice",
];

/// DeepSeek API provider that communicates with the DeepSeek chat completions endpoint.
pub struct DeepSeekProvider {
    api_key: String,
    model: String,
    base_url: String,
    client: Client,
}

impl DeepSeekProvider {
    /// Creates a new DeepSeek provider with the given API key and model, using the default base URL.
    pub fn new(
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self, ProviderError> {
        Self::from_config(api_key, model, None::<String>)
    }

    /// Creates a new DeepSeek provider with a custom base URL, falling back to the default if none is provided.
    pub fn from_config(
        api_key: impl Into<String>,
        model: impl Into<String>,
        base_url: Option<impl Into<String>>,
    ) -> Result<Self, ProviderError> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(ProviderError::Authentication(
                "missing DeepSeek API key".to_owned(),
            ));
        }

        let model = model.into();
        if model.trim().is_empty() {
            return Err(ProviderError::InvalidRequest(
                "missing DeepSeek model".to_owned(),
            ));
        }

        let base_url = base_url
            .map(Into::into)
            .unwrap_or_else(|| DEFAULT_DEEPSEEK_BASE_URL.to_owned());
        if base_url.trim().is_empty() {
            return Err(ProviderError::InvalidRequest(
                "missing DeepSeek base URL".to_owned(),
            ));
        }

        Ok(Self {
            api_key,
            model,
            base_url: base_url.trim_end_matches('/').to_owned(),
            client: Client::new(),
        })
    }

    /// Returns the configured model name.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Returns the configured base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn chat_completions_url(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    fn request_builder(&self, stream: bool) -> reqwest::RequestBuilder {
        let mut builder = self
            .client
            .post(self.chat_completions_url())
            .bearer_auth(&self.api_key);

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
        body.insert("messages".to_owned(), chat_messages(&request.chat));

        if let Some(temperature) = request.options.temperature {
            body.insert("temperature".to_owned(), json!(temperature));
        }

        if let Some(top_p) = request.options.top_p {
            body.insert("top_p".to_owned(), json!(top_p));
        }

        if let Some(max_output_tokens) = request.options.max_output_tokens {
            body.insert("max_tokens".to_owned(), json!(max_output_tokens));
        }

        for (key, value) in &request.options.extra {
            if !RESERVED_BODY_KEYS.contains(&key.as_str()) {
                body.insert(key.clone(), value.clone());
            }
        }

        if stream {
            body.insert("stream".to_owned(), Value::Bool(true));
            body.entry("stream_options".to_owned()).or_insert_with(|| {
                json!({
                    "include_usage": true,
                })
            });
        }

        Ok(Value::Object(body))
    }
}

impl fmt::Debug for DeepSeekProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeepSeekProvider")
            .field("api_key", &"<redacted>")
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
impl Provider for DeepSeekProvider {
    /// Returns the provider identifier, `"deepseek"`.
    fn id(&self) -> &'static str {
        "deepseek"
    }

    /// Sends a non-streaming chat completion request and returns the full response.
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

        if response.get("error").is_some() {
            return Err(ProviderError::Remote(response_error_message(&response)));
        }

        Ok(parse_response(&response))
    }

    /// Sends a streaming chat completion request, emitting deltas via the given channel
    /// and returning the accumulated response once the stream finishes.
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

                match parse_sse_event(&event)? {
                    SseEvent::Empty => {}
                    SseEvent::Done => {
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
                    SseEvent::Data(event) => {
                        if event.get("error").is_some() {
                            return Err(ProviderError::Remote(response_error_message(&event)));
                        }

                        if let Some(event_usage) = parse_usage(event.get("usage")) {
                            usage = Some(event_usage);
                        }

                        let Some(delta) = event
                            .get("choices")
                            .and_then(Value::as_array)
                            .and_then(|choices| choices.first())
                            .and_then(|choice| choice.get("delta"))
                        else {
                            continue;
                        };

                        if let Some(delta) = delta.get("content").and_then(Value::as_str) {
                            content.push_str(delta);
                            let _ = events
                                .send(ProviderEvent::ContentDelta(delta.to_owned()))
                                .await;
                        }

                        if let Some(delta) = delta.get("reasoning_content").and_then(Value::as_str)
                        {
                            reasoning_content.push_str(delta);
                            let _ = events
                                .send(ProviderEvent::ReasoningDelta(delta.to_owned()))
                                .await;
                        }
                    }
                }
            }
        }

        if !buffer.is_empty() {
            return Err(ProviderError::Remote(
                "malformed server-sent event stream".to_owned(),
            ));
        }

        Err(ProviderError::Remote(
            "stream closed before [DONE]".to_owned(),
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

fn chat_messages(request: &nekobot_core::agent::types::ChatRequest) -> Value {
    let mut messages = Vec::new();

    if let Some(system_prompt) = &request.system_prompt {
        messages.push(json!({
            "role": "system",
            "content": system_prompt,
        }));
    }

    messages.extend(request.messages.iter().map(chat_message));
    Value::Array(messages)
}

fn chat_message(message: &ChatMessage) -> Value {
    let (role, content) = match &message.role {
        Role::User => ("user", message.content.content.clone()),
        Role::Assistant => ("assistant", message.content.content.clone()),
        Role::Custom(role) if role == "system" || role == "developer" => {
            ("system", message.content.content.clone())
        }
        Role::Custom(role) => ("user", format!("[{role}] {}", message.content.content)),
    };

    json!({
        "role": role,
        "content": content,
    })
}

fn parse_response(response: &Value) -> ChatResponse {
    let message = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"));

    let content = message
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();

    let reasoning_content = message
        .and_then(|message| message.get("reasoning_content"))
        .and_then(Value::as_str)
        .filter(|content| !content.is_empty())
        .map(ToOwned::to_owned);

    ChatResponse {
        content,
        reasoning_content,
        images: Vec::new(),
        usage: parse_usage(response.get("usage")),
    }
}

fn parse_usage(usage: Option<&Value>) -> Option<Usage> {
    let usage = usage?;

    Some(Usage {
        input_tokens: usage.get("prompt_tokens").and_then(Value::as_u64),
        output_tokens: usage.get("completion_tokens").and_then(Value::as_u64),
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
        StatusCode::UNAUTHORIZED => ProviderError::Authentication(message),
        StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimited {
            retry_after,
            message,
        },
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => {
            ProviderError::InvalidRequest(message)
        }
        StatusCode::INTERNAL_SERVER_ERROR | StatusCode::SERVICE_UNAVAILABLE => {
            ProviderError::Unavailable(message)
        }
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

enum SseEvent {
    Empty,
    Done,
    Data(Value),
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

fn parse_sse_event(bytes: &[u8]) -> Result<SseEvent, ProviderError> {
    let event = std::str::from_utf8(bytes)
        .map_err(|error| ProviderError::Remote(format!("invalid SSE event: {error}")))?;
    let mut data = Vec::new();

    for line in event.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with(':') {
            continue;
        }

        if let Some(value) = line.strip_prefix("data:") {
            data.push(value.trim_start().to_owned());
        }
    }

    if data.is_empty() {
        return Ok(SseEvent::Empty);
    }

    let data = data.join("\n");
    if data == "[DONE]" {
        return Ok(SseEvent::Done);
    }

    let data = serde_json::from_str::<Value>(&data)
        .map_err(|error| ProviderError::Remote(format!("invalid SSE JSON event: {error}")))?;

    Ok(SseEvent::Data(data))
}

fn response_error_message(event: &Value) -> String {
    event
        .get("error")
        .and_then(|error| error.get("message").or(Some(error)))
        .and_then(Value::as_str)
        .or_else(|| event.get("message").and_then(Value::as_str))
        .unwrap_or("DeepSeek response failed")
        .to_owned()
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
                402 => "Payment Required",
                422 => "Unprocessable Entity",
                429 => "Too Many Requests",
                500 => "Internal Server Error",
                503 => "Service Unavailable",
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

    fn provider(base_url: String) -> DeepSeekProvider {
        DeepSeekProvider::from_config("deepseek-key", "deepseek-v4-pro", Some(base_url)).unwrap()
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
        let (base_url, request_receiver) =
            mock_server(200, &[], r#"{"choices":[{"message":{"content":"ok"}}]}"#).await;
        let provider = provider(base_url);
        let mut request = request_with_message("hello");
        request.options.temperature = Some(0.7);
        request.options.top_p = Some(0.9);
        request.options.max_output_tokens = Some(123);
        request
            .options
            .extra
            .insert("thinking".to_owned(), json!({ "type": "enabled" }));

        let response = provider.complete(request).await.unwrap();
        let request = request_receiver.await.unwrap();

        assert_eq!(response.content, "ok");
        assert_eq!(request.path, "/chat/completions");
        assert_eq!(
            request.headers.get("authorization").map(String::as_str),
            Some("Bearer deepseek-key")
        );
        assert_eq!(request.body["model"], "deepseek-v4-pro");
        assert!((request.body["temperature"].as_f64().unwrap() - 0.7).abs() < 0.00001);
        assert!((request.body["top_p"].as_f64().unwrap() - 0.9).abs() < 0.00001);
        assert_eq!(request.body["max_tokens"], 123);
        assert_eq!(request.body["thinking"], json!({ "type": "enabled" }));
        assert_eq!(request.body["messages"][0]["role"], "system");
        assert_eq!(request.body["messages"][0]["content"], "be useful");
        assert_eq!(request.body["messages"][1]["role"], "user");
        assert_eq!(request.body["messages"][1]["content"], "hello");
        assert!(request.body.get("stream").is_none());
    }

    #[tokio::test]
    async fn model_options_can_override_config_model() {
        let (base_url, request_receiver) =
            mock_server(200, &[], r#"{"choices":[{"message":{"content":"ok"}}]}"#).await;
        let provider = provider(base_url);
        let mut request = request_with_message("hello");
        request.options.model = Some("deepseek-v4-flash".to_owned());

        provider.complete(request).await.unwrap();
        let request = request_receiver.await.unwrap();

        assert_eq!(request.body["model"], "deepseek-v4-flash");
    }

    #[test]
    fn constructor_requires_model() {
        let result = DeepSeekProvider::new("key", "");

        assert!(matches!(result, Err(ProviderError::InvalidRequest(_))));
    }

    #[test]
    fn debug_output_redacts_api_key() {
        let provider = DeepSeekProvider::new("secret-key", "deepseek-v4-pro").unwrap();
        let debug = format!("{provider:?}");

        assert!(!debug.contains("secret-key"));
        assert!(debug.contains("<redacted>"));
    }

    #[tokio::test]
    async fn custom_roles_are_mapped_for_chat_completions() {
        let (base_url, request_receiver) =
            mock_server(200, &[], r#"{"choices":[{"message":{"content":"ok"}}]}"#).await;
        let provider = provider(base_url);
        let mut request = request_with_message("hello");
        request.chat.system_prompt = None;
        request.chat.messages = vec![
            ChatMessage {
                role: Role::Custom("developer".to_owned()),
                content: ChatMessageContent {
                    content: "dev rules".to_owned(),
                    reasoning_content: None,
                    images: Vec::new(),
                },
            },
            ChatMessage {
                role: Role::Custom("internal".to_owned()),
                content: ChatMessageContent {
                    content: "internal note".to_owned(),
                    reasoning_content: None,
                    images: Vec::new(),
                },
            },
        ];

        provider.complete(request).await.unwrap();
        let request = request_receiver.await.unwrap();

        assert_eq!(request.body["messages"][0]["role"], "system");
        assert_eq!(request.body["messages"][0]["content"], "dev rules");
        assert_eq!(request.body["messages"][1]["role"], "user");
        assert_eq!(
            request.body["messages"][1]["content"],
            "[internal] internal note"
        );
    }

    #[tokio::test]
    async fn complete_parses_content_reasoning_and_usage() {
        let (base_url, _request_receiver) = mock_server(
            200,
            &[],
            r#"{
                "choices": [
                    {
                        "message": {
                            "content": "done",
                            "reasoning_content": "reasoning"
                        }
                    }
                ],
                "usage": {
                    "prompt_tokens": 3,
                    "completion_tokens": 4,
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
        assert_eq!(response.reasoning_content.as_deref(), Some("reasoning"));
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
    async fn stream_emits_deltas_and_returns_accumulated_response() {
        let sse = concat!(
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"why\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2,\"total_tokens\":3}}\n\n",
            "data: [DONE]\n\n",
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
            request.body["stream_options"],
            json!({ "include_usage": true })
        );
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
        assert_eq!(
            event_receiver.recv().await,
            Some(ProviderEvent::ReasoningDelta("why".to_owned()))
        );
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
            Some(ProviderEvent::Finished {
                usage: response.usage.clone(),
            })
        );
    }

    #[tokio::test]
    async fn caller_stream_options_are_preserved() {
        let sse = concat!("data: [DONE]\n\n",);
        let (base_url, request_receiver) =
            mock_server(200, &[("content-type", "text/event-stream")], sse).await;
        let provider = provider(base_url);
        let mut request = request_with_message("hello");
        request.options.extra.insert(
            "stream_options".to_owned(),
            json!({ "include_usage": false }),
        );
        let (event_sender, _event_receiver) = mpsc::channel(8);

        provider.stream(request, event_sender).await.unwrap();
        let request = request_receiver.await.unwrap();

        assert_eq!(
            request.body["stream_options"],
            json!({ "include_usage": false })
        );
    }

    #[tokio::test]
    async fn stream_errors_on_malformed_sse() {
        let (base_url, _request_receiver) =
            mock_server(200, &[("content-type", "text/event-stream")], "data: {\n\n").await;
        let provider = provider(base_url);
        let (event_sender, _event_receiver) = mpsc::channel(8);

        let result = provider
            .stream(request_with_message("hello"), event_sender)
            .await;

        assert!(
            matches!(result, Err(ProviderError::Remote(message)) if message.starts_with("invalid SSE JSON event"))
        );
    }

    #[tokio::test]
    async fn stream_errors_when_closed_before_done() {
        let sse = concat!("data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",);
        let (base_url, _request_receiver) =
            mock_server(200, &[("content-type", "text/event-stream")], sse).await;
        let provider = provider(base_url);
        let (event_sender, _event_receiver) = mpsc::channel(8);

        let result = provider
            .stream(request_with_message("hello"), event_sender)
            .await;

        assert!(
            matches!(result, Err(ProviderError::Remote(message)) if message == "stream closed before [DONE]")
        );
    }

    #[tokio::test]
    async fn http_errors_map_to_provider_errors() {
        let (base_url, _request_receiver) =
            mock_server(401, &[], r#"{"error":{"message":"bad key"}}"#).await;
        let result = provider(base_url)
            .complete(request_with_message("hello"))
            .await;
        assert!(
            matches!(result, Err(ProviderError::Authentication(message)) if message == "bad key")
        );

        for status in [400, 422] {
            let (base_url, _request_receiver) =
                mock_server(status, &[], r#"{"error":{"message":"bad request"}}"#).await;
            let result = provider(base_url)
                .complete(request_with_message("hello"))
                .await;
            assert!(
                matches!(result, Err(ProviderError::InvalidRequest(message)) if message == "bad request")
            );
        }

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

        for status in [500, 503] {
            let (base_url, _request_receiver) =
                mock_server(status, &[], r#"{"error":{"message":"upstream down"}}"#).await;
            let result = provider(base_url)
                .complete(request_with_message("hello"))
                .await;
            assert!(
                matches!(result, Err(ProviderError::Unavailable(message)) if message == "upstream down")
            );
        }

        let (base_url, _request_receiver) =
            mock_server(402, &[], r#"{"error":{"message":"payment required"}}"#).await;
        let result = provider(base_url)
            .complete(request_with_message("hello"))
            .await;
        assert!(
            matches!(result, Err(ProviderError::Remote(message)) if message == "payment required")
        );
    }

    #[tokio::test]
    async fn tools_are_not_supported_yet() {
        let provider = DeepSeekProvider::new("key", "deepseek-v4-pro").unwrap();
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
