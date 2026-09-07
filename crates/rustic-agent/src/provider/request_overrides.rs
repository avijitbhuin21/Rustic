//! Applies per-model `RequestParamOverrides` to a provider request body right
//! before the HTTP send, mapping logical parameter names onto each provider's
//! wire keys. Adapters build their body as usual, then call
//! [`apply_request_overrides`] once.

use crate::config::{MaxTokensKey, ParamOverride, RequestParamOverrides};
use serde_json::{json, Value};

/// Wire dialect of the body being post-processed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyDialect {
    /// `/v1/chat/completions` (OpenAI, OpenRouter, Compatible gateways).
    OpenAiChat,
    /// `/v1/responses`.
    OpenAiResponses,
    /// Anthropic `/v1/messages`.
    Claude,
    /// Gemini `generateContent` (`generationConfig.*`).
    Gemini,
}

/// Mutates `body` according to `ov`; a no-op when the overrides are all `Auto`.
pub fn apply_request_overrides(body: &mut Value, ov: &RequestParamOverrides, dialect: BodyDialect) {
    if ov.is_empty() || !body.is_object() {
        return;
    }
    apply_max_tokens(body, ov, dialect);
    apply_scalar(body, &ov.temperature, dialect, "temperature");
    apply_scalar(body, &ov.top_p, dialect, "top_p");
    apply_reasoning_effort(body, &ov.reasoning_effort, dialect);
    apply_thinking_budget(body, &ov.thinking_budget, dialect);
    apply_scalar(body, &ov.parallel_tool_calls, dialect, "parallel_tool_calls");
    apply_tool_choice(body, &ov.tool_choice, dialect);
    apply_scalar(body, &ov.stop, dialect, "stop");
    for name in &ov.omit_params {
        remove_param(body, name, dialect);
    }
    if let Some(extra) = ov.extra_body.as_ref().filter(|v| v.is_object()) {
        deep_merge(body, extra);
    }
}

/// Wire path (dot-separated) for a logical parameter name, `None` when the dialect has no equivalent.
fn wire_path(name: &str, dialect: BodyDialect) -> Option<&'static str> {
    use BodyDialect::*;
    Some(match (name, dialect) {
        ("temperature", Gemini) => "generationConfig.temperature",
        ("temperature", _) => "temperature",
        ("top_p", Gemini) => "generationConfig.topP",
        ("top_p", _) => "top_p",
        ("stop", Claude) => "stop_sequences",
        ("stop", Gemini) => "generationConfig.stopSequences",
        ("stop", _) => "stop",
        ("parallel_tool_calls", OpenAiChat | OpenAiResponses) => "parallel_tool_calls",
        ("parallel_tool_calls", _) => return None,
        ("tool_choice", Gemini) => "toolConfig.functionCallingConfig.mode",
        ("tool_choice", _) => "tool_choice",
        ("max_tokens", Gemini) => "generationConfig.maxOutputTokens",
        ("max_tokens", OpenAiResponses) => "max_output_tokens",
        ("max_tokens", _) => "max_tokens",
        ("max_completion_tokens", OpenAiChat) => "max_completion_tokens",
        ("reasoning_effort", OpenAiChat) => "reasoning",
        ("reasoning_effort", OpenAiResponses) => "reasoning",
        ("reasoning_effort", _) => return None,
        ("thinking_budget", Claude) => "thinking",
        ("thinking_budget", Gemini) => "generationConfig.thinkingConfig",
        ("thinking_budget", _) => return None,
        ("stream", Claude | OpenAiChat | OpenAiResponses) => "stream",
        _ => return None,
    })
}

fn apply_scalar(body: &mut Value, ov: &ParamOverride, dialect: BodyDialect, name: &str) {
    let Some(path) = wire_path(name, dialect) else {
        return;
    };
    match ov {
        ParamOverride::Auto => {}
        ParamOverride::Send { value } => set_path(body, path, value.clone()),
        ParamOverride::Omit => remove_path(body, path),
    }
}

fn apply_max_tokens(body: &mut Value, ov: &RequestParamOverrides, dialect: BodyDialect) {
    let is_oai_chat = dialect == BodyDialect::OpenAiChat;
    let default_path = wire_path("max_tokens", dialect).unwrap_or("max_tokens");
    let mut target = default_path.to_string();
    if is_oai_chat {
        match ov.max_tokens_key {
            MaxTokensKey::Auto => {
                if body.get("max_completion_tokens").is_some() {
                    target = "max_completion_tokens".to_string();
                }
            }
            MaxTokensKey::MaxTokens => {
                if let Some(v) = body.as_object_mut().and_then(|o| o.remove("max_completion_tokens")) {
                    body["max_tokens"] = v;
                }
            }
            MaxTokensKey::MaxCompletionTokens => {
                if let Some(v) = body.as_object_mut().and_then(|o| o.remove("max_tokens")) {
                    body["max_completion_tokens"] = v;
                }
                target = "max_completion_tokens".to_string();
            }
            MaxTokensKey::Omit => {
                remove_path(body, "max_tokens");
                remove_path(body, "max_completion_tokens");
                return;
            }
        }
    }
    match &ov.max_tokens {
        ParamOverride::Auto => {}
        ParamOverride::Send { value } => set_path(body, &target, value.clone()),
        ParamOverride::Omit => {
            remove_path(body, &target);
            if is_oai_chat {
                remove_path(body, "max_tokens");
                remove_path(body, "max_completion_tokens");
            }
        }
    }
}

fn apply_reasoning_effort(body: &mut Value, ov: &ParamOverride, dialect: BodyDialect) {
    use BodyDialect::*;
    match (ov, dialect) {
        (ParamOverride::Auto, _) => {}
        (ParamOverride::Omit, OpenAiChat) => {
            remove_path(body, "reasoning");
            remove_path(body, "reasoning_effort");
        }
        (ParamOverride::Omit, OpenAiResponses) => remove_path(body, "reasoning"),
        (ParamOverride::Send { value }, OpenAiChat) => {
            // Direct openai.com wants the flat `reasoning_effort`; OpenRouter and
            // most gateways take `reasoning: { effort }`. Keep whichever shape
            // the adapter already chose, defaulting to the nested form.
            if body.get("reasoning_effort").is_some() {
                body["reasoning_effort"] = value.clone();
            } else {
                set_path(body, "reasoning.effort", value.clone());
            }
        }
        (ParamOverride::Send { value }, OpenAiResponses) => set_path(body, "reasoning.effort", value.clone()),
        // Claude / Gemini expose effort through thinking budgets, not an effort enum.
        (_, Claude | Gemini) => {}
    }
}

fn apply_thinking_budget(body: &mut Value, ov: &ParamOverride, dialect: BodyDialect) {
    use BodyDialect::*;
    match (ov, dialect) {
        (ParamOverride::Auto, _) => {}
        (ParamOverride::Omit, Claude) => remove_path(body, "thinking"),
        (ParamOverride::Omit, Gemini) => remove_path(body, "generationConfig.thinkingConfig"),
        (ParamOverride::Send { value }, Claude) => {
            let budget = value.as_u64().unwrap_or(0);
            if budget == 0 {
                remove_path(body, "thinking");
            } else {
                body["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
            }
        }
        (ParamOverride::Send { value }, Gemini) => {
            set_path(body, "generationConfig.thinkingConfig.thinkingBudget", value.clone());
        }
        (_, OpenAiChat | OpenAiResponses) => {}
    }
}

fn apply_tool_choice(body: &mut Value, ov: &ParamOverride, dialect: BodyDialect) {
    use BodyDialect::*;
    let Some(path) = wire_path("tool_choice", dialect) else {
        return;
    };
    match ov {
        ParamOverride::Auto => {}
        ParamOverride::Omit => remove_path(body, path),
        ParamOverride::Send { value } => {
            let mapped = match (value.as_str(), dialect) {
                (Some(s), Claude) => match s {
                    "auto" | "any" | "none" => json!({ "type": s }),
                    "required" => json!({ "type": "any" }),
                    other => json!({ "type": "tool", "name": other }),
                },
                (Some(s), Gemini) => json!(match s {
                    "required" | "any" => "ANY",
                    "none" => "NONE",
                    _ => "AUTO",
                }),
                _ => value.clone(),
            };
            set_path(body, path, mapped);
        }
    }
}

/// Removes a logical parameter (mapped per dialect) or, failing that, a literal dotted path.
fn remove_param(body: &mut Value, name: &str, dialect: BodyDialect) {
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    if let Some(path) = wire_path(name, dialect) {
        remove_path(body, path);
    }
    if name == "reasoning_effort" {
        remove_path(body, "reasoning_effort");
    }
    remove_path(body, name);
}

fn set_path(body: &mut Value, path: &str, value: Value) {
    let mut cur = body;
    let mut parts = path.split('.').peekable();
    while let Some(part) = parts.next() {
        if !cur.is_object() {
            *cur = json!({});
        }
        let obj = cur.as_object_mut().unwrap();
        if parts.peek().is_none() {
            obj.insert(part.to_string(), value);
            return;
        }
        cur = obj.entry(part.to_string()).or_insert_with(|| json!({}));
    }
}

fn remove_path(body: &mut Value, path: &str) {
    let parts: Vec<&str> = path.split('.').collect();
    let Some((last, parents)) = parts.split_last() else {
        return;
    };
    let mut cur = body;
    for p in parents {
        match cur.get_mut(*p) {
            Some(next) => cur = next,
            None => return,
        }
    }
    if let Some(obj) = cur.as_object_mut() {
        obj.remove(*last);
    }
}

fn deep_merge(base: &mut Value, extra: &Value) {
    match (base, extra) {
        (Value::Object(b), Value::Object(e)) => {
            for (k, v) in e {
                match b.get_mut(k) {
                    Some(existing) if existing.is_object() && v.is_object() => deep_merge(existing, v),
                    _ => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, e) => *b = e.clone(),
    }
}

/// Extracts the offending parameter name from a provider 400 body such as
/// `Unsupported parameter: 'max_tokens' is not supported with this model` or
/// `Unrecognized request argument supplied: foo`.
pub fn unsupported_param_from_error(err: &str) -> Option<String> {
    let lower = err.to_lowercase();
    let markers = [
        "unsupported parameter:",
        "unsupported parameter",
        "unrecognized request argument supplied:",
        "unrecognized request argument:",
        "unknown parameter:",
        "unknown field:",
        "extra inputs are not permitted",
    ];
    let start = markers.iter().find_map(|m| lower.find(m).map(|i| i + m.len()))?;
    let rest = &err[start..];
    let ident: String = rest
        .chars()
        .skip_while(|c| !(c.is_ascii_alphanumeric() || *c == '_'))
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
        .collect();
    let ident = ident.trim_end_matches('.').to_string();
    if ident.is_empty() || ident.eq_ignore_ascii_case("is") || ident.len() > 64 {
        return None;
    }
    Some(ident)
}

/// The replacement key a provider names in a "use X instead" 400, if any.
pub fn suggested_replacement_param(err: &str) -> Option<String> {
    let lower = err.to_lowercase();
    let i = lower.find("use '").or_else(|| lower.find("use `"))?;
    let rest = &err[i + 5..];
    let ident: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (!ident.is_empty()).then_some(ident)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ov() -> RequestParamOverrides {
        RequestParamOverrides::default()
    }

    #[test]
    fn auto_is_noop() {
        let mut body = json!({ "model": "x", "max_tokens": 10, "temperature": 0.7 });
        let before = body.clone();
        apply_request_overrides(&mut body, &ov(), BodyDialect::OpenAiChat);
        assert_eq!(body, before);
    }

    #[test]
    fn renames_max_tokens_to_max_completion_tokens() {
        let mut body = json!({ "max_tokens": 4096 });
        let mut o = ov();
        o.max_tokens_key = MaxTokensKey::MaxCompletionTokens;
        apply_request_overrides(&mut body, &o, BodyDialect::OpenAiChat);
        assert_eq!(body, json!({ "max_completion_tokens": 4096 }));
    }

    #[test]
    fn omits_and_sends_scalars_per_dialect() {
        let mut body = json!({ "temperature": 0.7, "generationConfig": { "temperature": 0.7 } });
        let mut o = ov();
        o.temperature = ParamOverride::Omit;
        o.top_p = ParamOverride::Send { value: json!(0.9) };
        apply_request_overrides(&mut body, &o, BodyDialect::Gemini);
        assert_eq!(body, json!({ "temperature": 0.7, "generationConfig": { "topP": 0.9 } }));
    }

    #[test]
    fn omit_params_and_extra_body() {
        let mut body = json!({ "max_tokens": 1, "temperature": 0.5, "stream": true });
        let mut o = ov();
        o.omit_params = vec!["temperature".into(), "stream_options".into()];
        o.extra_body = Some(json!({ "provider": { "order": ["a"] } }));
        apply_request_overrides(&mut body, &o, BodyDialect::OpenAiChat);
        assert_eq!(
            body,
            json!({ "max_tokens": 1, "stream": true, "provider": { "order": ["a"] } })
        );
    }

    #[test]
    fn tool_choice_maps_to_claude_shape() {
        let mut body = json!({});
        let mut o = ov();
        o.tool_choice = ParamOverride::Send { value: json!("required") };
        apply_request_overrides(&mut body, &o, BodyDialect::Claude);
        assert_eq!(body, json!({ "tool_choice": { "type": "any" } }));
    }

    #[test]
    fn parses_unsupported_parameter_errors() {
        let e = "OpenAI API error 400 Bad Request: { \"error\": { \"message\": \"Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead.\" } }";
        assert_eq!(unsupported_param_from_error(e).as_deref(), Some("max_tokens"));
        assert_eq!(suggested_replacement_param(e).as_deref(), Some("max_completion_tokens"));
        assert_eq!(
            unsupported_param_from_error("Unrecognized request argument supplied: top_k").as_deref(),
            Some("top_k")
        );
        assert!(unsupported_param_from_error("rate limit exceeded").is_none());
    }
}
