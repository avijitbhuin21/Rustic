//! `sleep` — pause the agent loop for a fixed number of seconds.
//!
//! Exists because `run_command` parks anything longer than its inline grace
//! window in a background terminal; when the agent just needs to *wait*
//! (server warm-up, a polling interval, a rate-limit cooldown) it should not
//! have to spawn a shell for it. The executor wraps every tool future in
//! `await_or_cancel`, so Stop interrupts the sleep immediately.

use super::{ToolContext, ToolOutput};
use crate::provider::ToolDef;
use anyhow::Result;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(250);

pub fn definitions() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "sleep".into(),
        description: "Pause for a fixed number of seconds, then return. Use this when you \
             need to WAIT — for a dev server to warm up, a background terminal to \
             progress, a rate-limit cooldown, or a polling interval before re-checking \
             something. Unlike `run_command` with a shell sleep, this never gets moved \
             to a background terminal, no matter how long it is; it simply blocks your \
             turn for the requested time and then returns. Cancelled immediately if \
             the user stops the task. Prefer a few short sleeps with a check in between \
             over one very long sleep."
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "seconds": {
                    "type": "number",
                    "minimum": 0,
                    "description": "How long to wait, in seconds (fractions allowed)."
                },
                "reason": {
                    "type": "string",
                    "description": "Optional short note on what you are waiting for; echoed back in the result."
                }
            },
            "required": ["seconds"]
        }),
    }]
}

/// Parse the requested duration, accepting numbers or numeric strings.
fn parse_seconds(params: &Value) -> std::result::Result<f64, String> {
    let raw = params
        .get("seconds")
        .ok_or_else(|| "Missing required parameter `seconds`.".to_string())?;
    let secs = match raw {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .ok_or_else(|| format!("`seconds` must be a number, got {raw}"))?;
    if !secs.is_finite() || secs < 0.0 {
        return Err(format!("`seconds` must be a finite non-negative number, got {secs}"));
    }
    Ok(secs)
}

/// Format a duration as a compact human string ("1m 05s", "12.5s").
fn fmt_secs(secs: f64) -> String {
    if secs >= 60.0 {
        let whole = secs.round() as u64;
        format!("{}m {:02}s", whole / 60, whole % 60)
    } else if secs.fract() == 0.0 {
        format!("{}s", secs as u64)
    } else {
        format!("{secs:.1}s")
    }
}

pub async fn execute(params: Value, context: &ToolContext) -> Result<ToolOutput> {
    let secs = match parse_seconds(&params) {
        Ok(s) => s,
        Err(msg) => {
            return Ok(ToolOutput {
                content: msg,
                is_error: true,
                attachments: Vec::new(),
            })
        }
    };
    let reason = params
        .get("reason")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let total = Duration::from_secs_f64(secs);
    let start = Instant::now();
    loop {
        let elapsed = start.elapsed();
        if elapsed >= total {
            break;
        }
        if let Some(tok) = &context.cancel_token {
            if tok.load(Ordering::Relaxed) {
                return Ok(ToolOutput {
                    content: format!(
                        "Sleep cancelled after {} (requested {}).",
                        fmt_secs(elapsed.as_secs_f64()),
                        fmt_secs(secs)
                    ),
                    is_error: true,
                    attachments: Vec::new(),
                });
            }
        }
        tokio::time::sleep((total - elapsed).min(POLL_INTERVAL)).await;
    }

    let mut content = format!("Slept for {}.", fmt_secs(secs));
    if let Some(r) = reason {
        content.push_str(&format!(" Reason: {r}"));
    }
    Ok(ToolOutput {
        content,
        is_error: false,
        attachments: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_numbers_and_numeric_strings() {
        assert_eq!(parse_seconds(&json!({ "seconds": 2 })).unwrap(), 2.0);
        assert_eq!(parse_seconds(&json!({ "seconds": 1.5 })).unwrap(), 1.5);
        assert_eq!(parse_seconds(&json!({ "seconds": "3" })).unwrap(), 3.0);
    }

    #[test]
    fn parse_rejects_missing_negative_and_garbage() {
        assert!(parse_seconds(&json!({})).is_err());
        assert!(parse_seconds(&json!({ "seconds": -1 })).is_err());
        assert!(parse_seconds(&json!({ "seconds": "soon" })).is_err());
        assert!(parse_seconds(&json!({ "seconds": true })).is_err());
    }

    #[test]
    fn fmt_secs_is_compact() {
        assert_eq!(fmt_secs(5.0), "5s");
        assert_eq!(fmt_secs(2.5), "2.5s");
        assert_eq!(fmt_secs(65.0), "1m 05s");
    }
}
