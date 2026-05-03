//! Sleep tool — pauses execution for a specified duration.

use nekobot_core::agent::tool::ToolResult;
use serde_json::Value;

pub struct SleepTool;

#[async_trait::async_trait]
impl nekobot_core::agent::tool::Tool for SleepTool {
    fn name(&self) -> &str {
        "sleep"
    }
    fn description(&self) -> &str {
        "Pause execution for a given number of seconds."
    }
    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "seconds": {
                    "type": "number",
                    "description": "Number of seconds to sleep (max 300)"
                }
            },
            "required": ["seconds"]
        })
    }
    async fn call(&self, args: Value) -> ToolResult<Value> {
        let secs = args.get("seconds").and_then(Value::as_f64).unwrap_or(0.0);
        let ms = (secs.min(300.0).max(0.0) * 1000.0) as u64;
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        Ok(Value::String(format!(
            "Slept for {:.1}s",
            ms as f64 / 1000.0
        )))
    }
}
