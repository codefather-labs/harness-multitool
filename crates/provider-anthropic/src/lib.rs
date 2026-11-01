//! The Anthropic API (IF-1), raw HTTP over the `Net` interface: Rust has no official SDK.
//! Contract: POST /v1/messages with `stream: true`, API version 2026-10-21. Blocks the harness
//! does not read (thinking, fallback markers) are replayed unchanged.

use std::collections::BTreeMap;
use std::rc::Rc;

use harness_api::platform::{Clock, Net, NetRequest};
use harness_api::sse::SseParser;
use harness_api::*;
use serde_json::{Value, json};

pub const API_VERSION: &str = "2026-10-21";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const RETRIES: u32 = 2;

pub struct Anthropic {
    auth: Rc<dyn Auth>,
    net: Rc<dyn Net>,
    clock: Rc<dyn Clock>,
    base_url: String,
    effort: String,
    fallbacks: bool,
}

impl Anthropic {
    /// Settings: `base_url`, `effort`, `fallbacks` (all optional).
    pub fn new(auth: Rc<dyn Auth>, net: Rc<dyn Net>, clock: Rc<dyn Clock>, settings: &Value) -> Self {
        Anthropic {
            auth,
            net,
            clock,
            base_url: settings["base_url"].as_str().unwrap_or("https://api.anthropic.com").trim_end_matches('/').to_string(),
            effort: settings["effort"].as_str().unwrap_or("high").to_string(),
            fallbacks: settings["fallbacks"].as_bool().unwrap_or(true),
        }
    }

    fn supports_fallbacks(model: &str) -> bool {
        ["claude-fable-5-1", "claude-opus-5-5", "claude-opus-5", "claude-sonnet-5-5"].contains(&model)
    }

    fn body(&self, request: &ModelRequest) -> Value {
        let mut body = json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "stream": true,
            "messages": request.messages.iter().map(message).collect::<Vec<_>>(),
            "cache_control": {"type": "ephemeral"},
            "output_config": {"effort": self.effort},
            // Show a summary of the reasoning: without it the default leaves the thinking empty.
            "thinking": {"type": "adaptive", "display": "summarized"},
        });
        if let Some(system) = &request.system {
            body["system"] = json!(system);
        }
        if !request.tools.is_empty() {
            body["tools"] = request
                .tools
                .iter()
                .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.schema, "eager_input_streaming": true}))
                .collect();
        }
        if self.fallbacks && Self::supports_fallbacks(&request.model) {
            body["fallbacks"] = json!("default");
        }
        body
    }
}

fn message(m: &Message) -> Value {
    let content: Vec<Value> = m
        .content
        .iter()
        .map(|c| match c {
            Content::Text { text } => json!({"type": "text", "text": text}),
            Content::ToolUse { id, name, input } => json!({"type": "tool_use", "id": id, "name": name, "input": input}),
            Content::ToolResult { id, output, is_error } => json!({"type": "tool_result", "tool_use_id": id, "content": output, "is_error": is_error}),
            Content::Opaque { block } => block.clone(),
        })
        .collect();
    json!({"role": match m.role { Role::User => "user", Role::Assistant => "assistant" }, "content": content})
}

/// Micro-dollars per token, i.e. dollars per million tokens: input, output, cache read, cache
/// write. From the price table of 2026-10-06; cache writes are assumed at 1.25x input.
/// A model's (input, output) price per 1M tokens, for showing.
pub fn per_million(model: &str) -> Option<(f64, f64)> {
    prices(model).map(|(input, output, ..)| (input, output))
}

fn prices(model: &str) -> Option<(f64, f64, f64, f64)> {
    let (input, output, cache_read) = match model {
        "claude-opus-5-5" => (4.0, 20.0, 0.20),
        "claude-sonnet-5-5" => (2.0, 10.0, 0.20),
        "claude-haiku-5-5" => (0.10, 0.50, 0.01),
        "claude-fable-5-1" => (10.0, 50.0, 0.25),
        _ => return None,
    };
    Some((input, output, cache_read, input * 1.25))
}

fn cost(model: &str, u: &Usage) -> Option<u64> {
    let (i, o, r, w) = prices(model)?;
    Some((u.input as f64 * i + u.output as f64 * o + u.cache_read as f64 * r + u.cache_write as f64 * w).round() as u64)
}

/// One content block being streamed.
enum Open {
    Text,
    Tool { id: String, name: String, json: String },
    Other { block: Value },
}

/// Turns the stream's events into model events.
#[derive(Default)]
struct TurnReader {
    open: BTreeMap<u64, Open>,
    usage: Usage,
    stop: Option<StopReason>,
    error: Option<String>,
}

impl TurnReader {
    fn event(&mut self, data: &str, sink: &mut dyn FnMut(ModelEvent) -> bool) -> bool {
        let Ok(e) = serde_json::from_str::<Value>(data) else { return true };
        let index = e["index"].as_u64().unwrap_or(0);
        match e["type"].as_str().unwrap_or_default() {
            "message_start" => self.read_usage(&e["message"]["usage"]),
            "content_block_start" => {
                let block = &e["content_block"];
                let open = match block["type"].as_str().unwrap_or_default() {
                    "text" => Open::Text,
                    "tool_use" => Open::Tool {
                        id: block["id"].as_str().unwrap_or_default().to_string(),
                        name: block["name"].as_str().unwrap_or_default().to_string(),
                        json: String::new(),
                    },
                    _ => Open::Other { block: block.clone() },
                };
                self.open.insert(index, open);
            }
            "content_block_delta" => {
                let delta = &e["delta"];
                match (self.open.get_mut(&index), delta["type"].as_str().unwrap_or_default()) {
                    (Some(Open::Text), "text_delta") => {
                        if !sink(ModelEvent::Text(delta["text"].as_str().unwrap_or_default().to_string())) {
                            return false;
                        }
                    }
                    (Some(Open::Tool { json, .. }), "input_json_delta") => json.push_str(delta["partial_json"].as_str().unwrap_or_default()),
                    // Thinking and signature deltas: grow the field they name, so the block
                    // replays whole.
                    (Some(Open::Other { block }), kind) => {
                        if let Some(field) = kind.strip_suffix("_delta") {
                            let piece = delta[field].as_str().unwrap_or_default();
                            if field == "thinking" && !piece.is_empty() && !sink(ModelEvent::Thinking(piece.to_string())) {
                                return false;
                            }
                            let grown = format!("{}{piece}", block[field].as_str().unwrap_or_default());
                            block[field] = json!(grown);
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => match self.open.remove(&index) {
                Some(Open::Tool { id, name, json }) => {
                    // Eager streaming leaves validation to the client: a broken input reaches
                    // the tool as such and comes back to the model as an error result.
                    let input = if json.trim().is_empty() {
                        json!({})
                    } else {
                        serde_json::from_str(&json).unwrap_or_else(|_| json!({"_invalid_json": json}))
                    };
                    sink(ModelEvent::ToolCall(ToolCall { id, name, input }));
                }
                Some(Open::Other { block }) => {
                    sink(ModelEvent::Opaque(block));
                }
                _ => {}
            },
            "message_delta" => {
                self.read_usage(&e["usage"]);
                self.stop = e["delta"]["stop_reason"].as_str().map(|s| match s {
                    "end_turn" | "stop_sequence" => StopReason::EndTurn,
                    "tool_use" => StopReason::ToolUse,
                    "max_tokens" => StopReason::MaxTokens,
                    other => StopReason::Other(other.to_string()),
                });
            }
            "error" => self.error = Some(e["error"]["message"].as_str().unwrap_or("the API reported an error").to_string()),
            _ => {} // ping, message_stop, and whatever comes later
        }
        true
    }

    fn read_usage(&mut self, u: &Value) {
        let mut take = |field: &str, slot: fn(&mut Usage) -> &mut u64| {
            if let Some(n) = u[field].as_u64() {
                *slot(&mut self.usage) = n;
            }
        };
        take("input_tokens", |x| &mut x.input);
        take("output_tokens", |x| &mut x.output);
        take("cache_read_input_tokens", |x| &mut x.cache_read);
        take("cache_creation_input_tokens", |x| &mut x.cache_write);
    }
}

impl Provider for Anthropic {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn models(&self) -> Vec<ModelInfo> {
        ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5", "claude-fable-5-1"]
            .iter()
            .map(|id| ModelInfo { id: id.to_string(), context_window: 1_000_000, supports_tools: true })
            .collect()
    }

    fn turn<'a>(&'a self, request: &'a ModelRequest, sink: &'a mut dyn FnMut(ModelEvent) -> bool) -> BoxFuture<'a, PluginResult<TurnEnd>> {
        Box::pin(async move {
            let key = self.auth.credentials("anthropic").await?;
            let body = self.body(request).to_string().into_bytes();
            let mut headers = vec![
                ("content-type".to_string(), "application/json".to_string()),
                ("anthropic-version".to_string(), API_VERSION.to_string()),
                ("x-api-key".to_string(), key.expose().to_string()),
            ];
            if body_has_fallbacks(&body) {
                headers.push(("anthropic-beta".to_string(), FALLBACK_BETA.to_string()));
            }
            let url = format!("{}/v1/messages", self.base_url);
            let mut attempt = 0;
            let mut response = loop {
                let sent = self.net.send(NetRequest { method: "POST".into(), url: url.clone(), headers: headers.clone(), body: Some(body.clone()) }).await;
                match sent {
                    Ok(r) if r.status == 200 => break r,
                    Ok(mut r) if !(r.status == 429 || r.status >= 500) || attempt >= RETRIES => {
                        return Err(PluginError::new(format!("the Anthropic API answered {}: {}", r.status, error_text(&mut r.body).await)));
                    }
                    Err(e) if attempt >= RETRIES => return Err(e),
                    _ => {}
                }
                attempt += 1;
                self.clock.sleep(1000 * u64::from(attempt)).await;
            };
            let (mut parser, mut reader) = (SseParser::default(), TurnReader::default());
            while let Some(chunk) = response.body.next().await? {
                for data in parser.feed(&chunk) {
                    if !reader.event(&data, sink) {
                        return Ok(TurnEnd { stop: StopReason::Stopped, usage: reader.usage, cost_micro_usd: cost(&request.model, &reader.usage) });
                    }
                }
            }
            if let Some(message) = reader.error {
                return Err(PluginError(message));
            }
            let stop = reader.stop.ok_or_else(|| PluginError::new("the stream ended before the turn did"))?;
            Ok(TurnEnd { stop, usage: reader.usage, cost_micro_usd: cost(&request.model, &reader.usage) })
        })
    }
}

fn body_has_fallbacks(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body).is_ok_and(|b| b.get("fallbacks").is_some())
}

async fn error_text(body: &mut Box<dyn harness_api::platform::NetBody>) -> String {
    let mut bytes = Vec::new();
    while let Ok(Some(chunk)) = body.next().await {
        bytes.extend(chunk);
        if bytes.len() > 64 * 1024 {
            break;
        }
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"]["message"].as_str().map(str::to_string)).unwrap_or(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(events: &[&str]) -> (TurnReader, Vec<ModelEvent>) {
        let mut out = Vec::new();
        let mut reader = TurnReader::default();
        for e in events {
            reader.event(e, &mut |m| {
                out.push(m);
                true
            });
        }
        (reader, out)
    }

    #[test]
    fn a_stream_becomes_text_tools_blocks_usage_and_a_stop() {
        let (r, out) = read(&[
            r#"{"type":"message_start","message":{"usage":{"input_tokens":10,"cache_read_input_tokens":5,"cache_creation_input_tokens":2,"output_tokens":1}}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hm"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hello"}}"#,
            r#"{"type":"content_block_stop","index":1}"#,
            r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"t1","name":"shell","input":{}}}"#,
            r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"command\":"}}"#,
            r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"\"ls\"}"}}"#,
            r#"{"type":"content_block_stop","index":2}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":12}}"#,
            r#"{"type":"message_stop"}"#,
        ]);
        assert_eq!(out, vec![
            ModelEvent::Thinking("hm".into()),
            ModelEvent::Opaque(json!({"type":"thinking","thinking":"hm","signature":"sig"})),
            ModelEvent::Text("Hello".into()),
            ModelEvent::ToolCall(ToolCall { id: "t1".into(), name: "shell".into(), input: json!({"command":"ls"}) }),
        ]);
        assert_eq!(r.stop, Some(StopReason::ToolUse));
        assert_eq!(r.usage, Usage { input: 10, output: 12, cache_read: 5, cache_write: 2 });
    }

    #[test]
    fn a_broken_tool_input_is_marked_not_dropped() {
        let (_, out) = read(&[
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"x"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"cut"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
        ]);
        let ModelEvent::ToolCall(call) = &out[0] else { panic!() };
        assert_eq!(call.input["_invalid_json"], json!("{\"cut"));
    }

    #[test]
    fn a_refusal_is_reported_as_it_is() {
        let (r, _) = read(&[r#"{"type":"message_delta","delta":{"stop_reason":"refusal"},"usage":{"output_tokens":0}}"#]);
        assert_eq!(r.stop, Some(StopReason::Other("refusal".into())));
    }

    #[test]
    fn history_goes_out_in_the_apis_shape_with_opaque_blocks_unchanged() {
        let m = Message { role: Role::Assistant, content: vec![
            Content::Opaque { block: json!({"type":"thinking","thinking":"","signature":"s"}) },
            Content::ToolUse { id: "t".into(), name: "n".into(), input: json!({"a":1}) },
        ]};
        assert_eq!(message(&m), json!({"role":"assistant","content":[
            {"type":"thinking","thinking":"","signature":"s"},
            {"type":"tool_use","id":"t","name":"n","input":{"a":1}},
        ]}));
        let r = Message { role: Role::User, content: vec![Content::ToolResult { id: "t".into(), output: "ok".into(), is_error: false }] };
        assert_eq!(message(&r)["content"][0], json!({"type":"tool_result","tool_use_id":"t","content":"ok","is_error":false}));
    }

    #[test]
    fn cost_follows_the_price_table() {
        let u = Usage { input: 1_000_000, output: 1_000_000, cache_read: 0, cache_write: 0 };
        assert_eq!(cost("claude-opus-5-5", &u), Some(24_000_000));
        assert_eq!(cost("some-other-model", &u), None);
    }
}
