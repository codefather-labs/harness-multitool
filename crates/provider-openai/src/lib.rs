//! OpenAI-compatible chat completions (IF-1): OpenAI, OpenRouter, and local servers. The
//! provider's name selects its key (IF-2) and its `base_url` comes from the settings.

use std::collections::BTreeMap;
use std::rc::Rc;

use harness_api::platform::{Net, NetRequest};
use harness_api::sse::SseParser;
use harness_api::*;
use serde_json::{Value, json};

pub mod prices;
pub mod responses;

pub struct OpenAiCompatible {
    name: String,
    auth: Rc<dyn Auth>,
    net: Rc<dyn Net>,
    base_url: String,
    context_window: u64,
}

impl OpenAiCompatible {
    /// Settings: `base_url` (default OpenAI's), `context_window`.
    pub fn new(name: &str, auth: Rc<dyn Auth>, net: Rc<dyn Net>, settings: &Value) -> Self {
        OpenAiCompatible {
            name: name.to_string(),
            auth,
            net,
            base_url: settings["base_url"].as_str().unwrap_or("https://api.openai.com/v1").trim_end_matches('/').to_string(),
            context_window: settings["context_window"].as_u64().unwrap_or(128_000),
        }
    }
}

/// Our history as chat messages: a user message of tool results becomes one `tool` message each.
fn messages(request: &ModelRequest) -> Vec<Value> {
    let mut out = Vec::new();
    if let Some(system) = &request.system {
        out.push(json!({"role": "system", "content": system}));
    }
    for m in &request.messages {
        match m.role {
            Role::User => {
                let mut text = String::new();
                for c in &m.content {
                    match c {
                        Content::Text { text: t } => text.push_str(t),
                        Content::ToolResult { id, output, .. } => out.push(json!({"role": "tool", "tool_call_id": id, "content": output})),
                        _ => {}
                    }
                }
                if !text.is_empty() {
                    out.push(json!({"role": "user", "content": text}));
                }
            }
            Role::Assistant => {
                let mut text = String::new();
                let mut calls = Vec::new();
                for c in &m.content {
                    match c {
                        Content::Text { text: t } => text.push_str(t),
                        Content::ToolUse { id, name, input } => {
                            calls.push(json!({"id": id, "type": "function", "function": {"name": name, "arguments": input.to_string()}}));
                        }
                        _ => {}
                    }
                }
                let mut message = json!({"role": "assistant", "content": if text.is_empty() { Value::Null } else { json!(text) }});
                if !calls.is_empty() {
                    message["tool_calls"] = json!(calls);
                }
                out.push(message);
            }
        }
    }
    out
}

#[derive(Default)]
struct OpenCall {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Default)]
struct TurnReader {
    calls: BTreeMap<u64, OpenCall>,
    usage: Usage,
    finish: Option<String>,
}

impl TurnReader {
    fn event(&mut self, data: &str, sink: &mut dyn FnMut(ModelEvent) -> bool) -> bool {
        if data.trim() == "[DONE]" {
            return true;
        }
        let Ok(e) = serde_json::from_str::<Value>(data) else { return true };
        if let Some(u) = e.get("usage").filter(|u| u.is_object()) {
            self.usage.input = u["prompt_tokens"].as_u64().unwrap_or(0);
            self.usage.output = u["completion_tokens"].as_u64().unwrap_or(0);
        }
        let choice = &e["choices"][0];
        if let Some(reason) = choice["finish_reason"].as_str() {
            self.finish = Some(reason.to_string());
        }
        let delta = &choice["delta"];
        // Ollama and others stream the reasoning apart from the answer, under one of these names.
        if let Some(thinking) = delta["reasoning"].as_str().or(delta["reasoning_content"].as_str()).filter(|t| !t.is_empty())
            && !sink(ModelEvent::Thinking(thinking.to_string()))
        {
            return false;
        }
        if let Some(text) = delta["content"].as_str().filter(|t| !t.is_empty())
            && !sink(ModelEvent::Text(text.to_string()))
        {
            return false;
        }
        for call in delta["tool_calls"].as_array().into_iter().flatten() {
            let open = self.calls.entry(call["index"].as_u64().unwrap_or(0)).or_default();
            if let Some(id) = call["id"].as_str() {
                open.id = id.to_string();
            }
            if let Some(name) = call["function"]["name"].as_str() {
                open.name.push_str(name);
            }
            if let Some(args) = call["function"]["arguments"].as_str() {
                open.arguments.push_str(args);
            }
        }
        true
    }

    fn finish(self, sink: &mut dyn FnMut(ModelEvent) -> bool) -> PluginResult<TurnEnd> {
        let had_calls = !self.calls.is_empty();
        for (_, call) in self.calls {
            let input = if call.arguments.trim().is_empty() {
                json!({})
            } else {
                serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({"_invalid_json": call.arguments}))
            };
            sink(ModelEvent::ToolCall(ToolCall { id: call.id, name: call.name, input }));
        }
        let stop = match self.finish.as_deref() {
            Some("length") => StopReason::MaxTokens,
            Some("content_filter") => StopReason::Other("content_filter".into()),
            Some(_) | None if had_calls => StopReason::ToolUse,
            Some(_) => StopReason::EndTurn,
            None => return Err(PluginError::new("the stream ended before the turn did")),
        };
        Ok(TurnEnd { stop, usage: self.usage, cost_micro_usd: None })
    }
}

impl Provider for OpenAiCompatible {
    fn name(&self) -> &str {
        &self.name
    }

    fn models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo { id: "configured".into(), context_window: self.context_window, supports_tools: true }]
    }

    fn turn<'a>(&'a self, request: &'a ModelRequest, sink: &'a mut dyn FnMut(ModelEvent) -> bool) -> BoxFuture<'a, PluginResult<TurnEnd>> {
        Box::pin(async move {
            let key = self.auth.credentials(&self.name).await?;
            let mut body = json!({
                "model": request.model,
                "stream": true,
                "stream_options": {"include_usage": true},
                "max_tokens": request.max_tokens,
                "messages": messages(request),
            });
            if !request.tools.is_empty() {
                body["tools"] = request.tools.iter().map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.schema}})).collect();
            }
            let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
            if !key.expose().is_empty() {
                headers.push(("authorization".to_string(), format!("Bearer {}", key.expose())));
            }
            let url = format!("{}/chat/completions", self.base_url);
            let mut response = self.net.send(NetRequest { method: "POST".into(), url, headers, body: Some(body.to_string().into_bytes()) }).await?;
            if response.status != 200 {
                let mut text = Vec::new();
                while let Ok(Some(chunk)) = response.body.next().await {
                    text.extend(chunk);
                    if text.len() > 64 * 1024 {
                        break;
                    }
                }
                return Err(PluginError::new(format!("{} answered {}: {}", self.name, response.status, String::from_utf8_lossy(&text))));
            }
            let (mut parser, mut reader) = (SseParser::default(), TurnReader::default());
            while let Some(chunk) = response.body.next().await? {
                for data in parser.feed(&chunk) {
                    if !reader.event(&data, sink) {
                        return Ok(TurnEnd { stop: StopReason::Stopped, usage: reader.usage, cost_micro_usd: None });
                    }
                }
            }
            reader.finish(sink)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_with_split_tool_arguments_is_put_together() {
        let mut out = Vec::new();
        let mut sink = |m: ModelEvent| {
            out.push(m);
            true
        };
        let mut r = TurnReader::default();
        for e in [
            r#"{"choices":[{"delta":{"content":"","reasoning":"Hm"}}]}"#,
            r#"{"choices":[{"delta":{"content":"Hi"}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"shell","arguments":"{\"com"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"mand\":\"ls\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":3}}"#,
            "[DONE]",
        ] {
            r.event(e, &mut sink);
        }
        let end = r.finish(&mut sink).unwrap();
        assert_eq!((end.stop, end.usage.input, end.usage.output), (StopReason::ToolUse, 7, 3));
        assert_eq!(out, vec![
            ModelEvent::Thinking("Hm".into()),
            ModelEvent::Text("Hi".into()),
            ModelEvent::ToolCall(ToolCall { id: "c1".into(), name: "shell".into(), input: json!({"command":"ls"}) }),
        ]);
    }

    #[test]
    fn tool_results_become_tool_messages() {
        let request = ModelRequest {
            model: "m".into(),
            system: Some("s".into()),
            messages: vec![
                Message::user("hi"),
                Message { role: Role::Assistant, content: vec![Content::ToolUse { id: "c".into(), name: "n".into(), input: json!({}) }] },
                Message { role: Role::User, content: vec![Content::ToolResult { id: "c".into(), output: "out".into(), is_error: false }] },
            ],
            tools: vec![],
            max_tokens: 10,
        };
        let m = messages(&request);
        assert_eq!(m[0]["role"], "system");
        assert_eq!(m[2]["tool_calls"][0]["function"]["arguments"], "{}");
        assert_eq!(m[3], json!({"role":"tool","tool_call_id":"c","content":"out"}));
    }
}
