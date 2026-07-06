use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Which config file an MCP server was loaded from.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum McpScope {
    /// Global, in the app data dir (`mcp.json`). Shared across projects.
    User,
    /// Per-project `.mcp.json` in the project root. Committed to source control.
    Project,
}

impl Default for McpScope {
    fn default() -> Self {
        McpScope::User
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub id: String,
    pub name: String,
    pub transport: McpTransport,
    pub enabled: bool,
    #[serde(default)]
    pub scope: McpScope,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum McpTransport {
    #[serde(rename = "stdio")]
    Stdio {
        command: String,
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
    },
    /// Remote transport. `"type": "http"` is accepted as an alias — both name
    /// the same connection strategy in client.rs: Streamable HTTP
    /// (2025-03-26 spec) probed first, legacy HTTP+SSE (2024-11-05 spec) as
    /// fallback. `headers` are sent on every request (auth tokens etc.).
    /// Serialization always emits `"sse"` for backward compatibility.
    #[serde(rename = "sse", alias = "http")]
    Sse {
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdio_parses() {
        let t: McpTransport =
            serde_json::from_str(r#"{"type":"stdio","command":"npx","args":["-y","server"]}"#)
                .unwrap();
        match t {
            McpTransport::Stdio { command, args, env } => {
                assert_eq!(command, "npx");
                assert_eq!(args, vec!["-y", "server"]);
                assert!(env.is_empty());
            }
            _ => panic!("expected stdio transport"),
        }
    }

    #[test]
    fn sse_parses_with_headers() {
        let t: McpTransport = serde_json::from_str(
            r#"{"type":"sse","url":"https://example.com/mcp","headers":{"Authorization":"Bearer x"}}"#,
        )
        .unwrap();
        match t {
            McpTransport::Sse { url, headers } => {
                assert_eq!(url, "https://example.com/mcp");
                assert_eq!(
                    headers.get("Authorization").map(String::as_str),
                    Some("Bearer x")
                );
            }
            _ => panic!("expected sse transport"),
        }
    }

    #[test]
    fn sse_headers_default_to_empty() {
        let t: McpTransport =
            serde_json::from_str(r#"{"type":"sse","url":"https://example.com/mcp"}"#).unwrap();
        match t {
            McpTransport::Sse { headers, .. } => assert!(headers.is_empty()),
            _ => panic!("expected sse transport"),
        }
    }

    #[test]
    fn http_type_is_alias_for_remote_transport() {
        let t: McpTransport =
            serde_json::from_str(r#"{"type":"http","url":"https://example.com/mcp"}"#).unwrap();
        assert!(matches!(t, McpTransport::Sse { .. }));
    }

    #[test]
    fn sse_serializes_back_as_sse() {
        let t = McpTransport::Sse {
            url: "https://example.com/mcp".into(),
            headers: HashMap::new(),
        };
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v.get("type").and_then(|x| x.as_str()), Some("sse"));
    }
}
