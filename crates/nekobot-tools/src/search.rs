//! Web search tool via SearXNG API.

use serde::Deserialize;
use serde_json::Value;

use nekobot_core::agent::tool::{ToolError, ToolResult};
use tracing::debug;

#[derive(Debug, Deserialize)]
struct SearxResponse {
    results: Vec<SearxResult>,
    #[serde(default)]
    number_of_results: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct SearxResult {
    title: String,
    url: String,
    #[serde(default)]
    content: Option<String>,
}

pub struct SearchTool {
    base_url: String,
    http: reqwest::Client,
}

impl SearchTool {
    pub fn new(base_url: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
        }
    }
}

#[async_trait::async_trait]
impl nekobot_core::agent::tool::Tool for SearchTool {
    fn name(&self) -> &str {
        "search"
    }

    fn description(&self) -> &str {
        "Search the web using SearXNG. Returns titles, URLs, and snippets of search results."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "language": {
                    "type": "string",
                    "description": "Language code, e.g. 'zh-CN', 'en-US' (optional)"
                },
                "time_range": {
                    "type": "string",
                    "description": "Time range filter (optional)",
                    "enum": ["day", "month", "year"]
                }
            },
            "required": ["query"]
        })
    }

    async fn call(&self, args: Value) -> ToolResult<Value> {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("missing 'query'".to_owned()))?;

        let mut query_params: Vec<(String, String)> = vec![
            ("q".to_owned(), query.to_owned()),
            ("format".to_owned(), "json".to_owned()),
        ];
        if let Some(lang) = args.get("language").and_then(Value::as_str) {
            query_params.push(("language".to_owned(), lang.to_owned()));
        }
        if let Some(tr) = args.get("time_range").and_then(Value::as_str) {
            query_params.push(("time_range".to_owned(), tr.to_owned()));
        }

        let resp = self
            .http
            .get(format!("{}/search", self.base_url))
            .query(&query_params)
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("search request failed: {e}")))?;

        debug!("search url: {}", resp.url());

        let status = resp.status();
        if !status.is_success() {
            return Err(ToolError::Execution(format!("search returned {status}")));
        }

        let body: SearxResponse = resp
            .json()
            .await
            .map_err(|e| ToolError::Execution(format!("parse failed: {e}")))?;

        let items: Vec<Value> = body
            .results
            .iter()
            .take(10)
            .map(|r| {
                serde_json::json!({
                    "title": r.title,
                    "url": r.url,
                    "snippet": r.content.as_deref().unwrap_or(""),
                })
            })
            .collect();

        Ok(serde_json::json!({
            "results": items,
            "total": body.number_of_results.unwrap_or(0),
        }))
    }
}
