//! Bash tool — execute shell commands with timeout.

use std::time::Duration;

use serde_json::Value;

use nekobot_core::agent::tool::{ToolError, ToolResult};

use crate::MAX_OUTPUT_BYTES;

pub struct BashTool {
    pub timeout_secs: u64,
    pub workdir: Option<String>,
}

#[async_trait::async_trait]
impl nekobot_core::agent::tool::Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a shell command and return stdout, stderr, and exit code. \
         The command runs in a non-interactive shell with a timeout."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "workdir": {
                    "type": "string",
                    "description": "Working directory (optional)"
                }
            },
            "required": ["command"]
        })
    }

    async fn call(&self, args: Value) -> ToolResult<Value> {
        let command = args
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArguments("missing 'command'".to_owned()))?;

        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd.kill_on_drop(true);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        if let Some(wd) = args.get("workdir").and_then(Value::as_str).or(self.workdir.as_deref()) {
            cmd.current_dir(wd);
        }

        let child = cmd
            .spawn()
            .map_err(|e| ToolError::Execution(format!("failed to spawn: {e}")))?;

        let output = tokio::time::timeout(
            Duration::from_secs(self.timeout_secs),
            child.wait_with_output(),
        )
        .await
        .map_err(|_| ToolError::Execution("command timed out".to_owned()))?
        .map_err(|e| ToolError::Execution(format!("command failed: {e}")))?;

        let stdout = truncate(String::from_utf8_lossy(&output.stdout).into_owned());
        let stderr = truncate(String::from_utf8_lossy(&output.stderr).into_owned());

        Ok(serde_json::json!({
            "stdout": stdout,
            "stderr": stderr,
            "exit_code": output.status.code().unwrap_or(-1),
        }))
    }
}

fn truncate(s: String) -> String {
    if s.len() > MAX_OUTPUT_BYTES {
        let mut t = s[..MAX_OUTPUT_BYTES].to_owned();
        t.push_str("\n... (truncated)");
        t
    } else {
        s
    }
}
