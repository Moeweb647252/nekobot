//! Current time tool — returns the current date/time.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use nekobot_core::agent::tool::ToolResult;

pub struct CurrentTimeTool;

#[async_trait::async_trait]
impl nekobot_core::agent::tool::Tool for CurrentTimeTool {
    fn name(&self) -> &str {
        "current_time"
    }

    fn description(&self) -> &str {
        "Get the current date and time. Returns RFC 3339 format by default, or Unix timestamp."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "format": {
                    "type": "string",
                    "description": "Output format",
                    "enum": ["rfc3339", "unix"]
                }
            }
        })
    }

    async fn call(&self, args: Value) -> ToolResult<Value> {
        let format = args
            .get("format")
            .and_then(Value::as_str)
            .unwrap_or("rfc3339");

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let secs = now.as_secs();

        match format {
            "unix" => Ok(Value::Number(secs.into())),
            _ => {
                let datetime = unix_to_rfc3339(secs);
                Ok(Value::String(datetime))
            }
        }
    }
}

/// Convert a Unix timestamp to RFC 3339 string (UTC).
fn unix_to_rfc3339(secs: u64) -> String {
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Calculate year/month/day from days since epoch
    let mut days = days_since_epoch as i64;
    let mut year = 1970i64;
    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }

    let month_lengths = [
        31,
        if is_leap(year) { 29 } else { 28 },
        31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
    ];
    let mut month = 1;
    for &len in &month_lengths {
        if days < len {
            break;
        }
        days -= len;
        month += 1;
    }
    let day = days + 1;

    format!(
        "{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z"
    )
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_format() {
        // 2024-01-15T10:00:00Z
        let result = unix_to_rfc3339(1705312800);
        assert_eq!(result, "2024-01-15T10:00:00Z");
    }

    #[test]
    fn rfc3339_epoch() {
        let result = unix_to_rfc3339(0);
        assert_eq!(result, "1970-01-01T00:00:00Z");
    }

    #[test]
    fn rfc3339_leap_year() {
        // 2024-02-29T00:00:00Z = 1709164800
        let result = unix_to_rfc3339(1709164800);
        assert_eq!(result, "2024-02-29T00:00:00Z");
    }
}
