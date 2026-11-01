//! The Responses API (IF-1), as a ChatGPT plan requires it (CON-55): `store` false, `stream` true,
//! the whole history in `input`, the system prompt in `instructions`, and none of the fields the
//! route refuses, such as `max_output_tokens`. Only `response.completed` counts as success.

use std::rc::Rc;

use harness_api::platform::{Net, NetRequest};
use harness_api::sse::SseParser;
use harness_api::*;
use serde_json::{Value, json};

pub struct Responses {
    name: String,
    auth: Rc<dyn Auth>,
    net: Rc<dyn Net>,
    base_url: String,
    context_window: u64,
    reasoning_summary: Option<String>,
    /// The tokens are included in a plan, so nothing is charged per token.
    on_plan: bool,
}

impl Responses {
    /// Settings: `base_url`, `context_window`, `reasoning_summary`: "auto" by default, which shows
    /// the model's reasoning; "off" asks for none, for a model that refuses the field.
    pub fn new(name: &str, auth: Rc<dyn Auth>, net: Rc<dyn Net>, settings: &Value) -> Self {
        Responses {
            name: name.to_string(),
            auth,
            net,
            base_url: settings["base_url"].as_str().unwrap_or("https://api.openai.com/v1").trim_end_matches('/').to_string(),
            context_window: settings["context_window"].as_u64().unwrap_or(128_000),
            reasoning_summary: Some(settings["reasoning_summary"].as_str().unwrap_or("auto")).filter(|s| *s != "off").map(str::to_string),
            on_plan: false,
        }
    }

    /// Its tokens come from a plan, not a per-token bill.
    pub fn on_plan(mut self, on_plan: bool) -> Self {
        self.on_plan = on_plan;
        self
    }

    fn body(&self, request: &ModelRequest) -> Value {
        let mut body = json!({"model": request.model, "input": input(request), "store": false, "stream": true});
        if let Some(system) = &request.system {
            body["instructions"] = json!(system);
        }
        if !request.tools.is_empty() {
            body["tools"] = request.tools.iter().map(|t| json!({"type": "function", "name": t.name, "description": t.description, "parameters": t.schema})).collect();
        }
        if let Some(summary) = &self.reasoning_summary {
            body["reasoning"] = json!({"summary": summary});
        }
        body
    }
}

/// The conversation as input items, in order.
pub fn input(request: &ModelRequest) -> Vec<Value> {
    let mut items = Vec::new();
    for message in &request.messages {
        for content in &message.content {
            match content {
                Content::Text { text } if message.role == Role::User => items.push(json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": text}]})),
                Content::Text { text } => items.push(json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text}]})),
                Content::ToolUse { id, name, input } => items.push(json!({"type": "function_call", "call_id": id, "name": name, "arguments": input.to_string()})),
                Content::ToolResult { id, output, .. } => items.push(json!({"type": "function_call_output", "call_id": id, "output": output})),
                Content::Opaque { .. } => {}
            }
        }
    }
    items
}

/// What a plan's error means for the user, from OpenAI's errors page. A code stays in the message
/// so an unknown one can still be looked up.
fn explain(code: &str, message: &str) -> String {
    let meaning = match code {
        "subscription_sharing_usage_limit_exceeded" => "the plan's usage limit is reached; new requests wait, see the usage settings in ChatGPT",
        "subscription_sharing_usage_unavailable" | "subscription_sharing_user_unavailable" => "usage could not be checked just now; try again later",
        "subscription_sharing_user_not_eligible" => "plan usage is not available for this user, workspace or policy",
        "subscription_sharing_unsupported_capability" => "this route does not support part of the request",
        "subscription_sharing_invalid_user" => "the credential was refused",
        _ => message,
    };
    format!("{code}: {meaning}")
}

fn error_of(value: &Value) -> String {
    let error = &value["error"];
    let code = error["code"].as_str().or(error["type"].as_str()).unwrap_or("error");
    explain(code, error["message"].as_str().unwrap_or("no message"))
}

/// Reads a turn's events.
#[derive(Default)]
pub struct Reader {
    usage: Usage,
    called: bool,
    /// How the turn ended, once it has.
    end: Option<Result<StopReason, String>>,
}

impl Reader {
    /// One event. False if the user has stopped the turn.
    pub fn event(&mut self, data: &str, sink: &mut dyn FnMut(ModelEvent) -> bool) -> bool {
        let Ok(event) = serde_json::from_str::<Value>(data) else { return true };
        let delta = || event["delta"].as_str().unwrap_or_default().to_string();
        match event["type"].as_str().unwrap_or_default() {
            "response.output_text.delta" => return sink(ModelEvent::Text(delta())),
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => return sink(ModelEvent::Thinking(delta())),
            "response.output_item.done" if event["item"]["type"] == "function_call" => {
                let item = &event["item"];
                self.called = true;
                let call = ToolCall {
                    id: item["call_id"].as_str().unwrap_or_default().to_string(),
                    name: item["name"].as_str().unwrap_or_default().to_string(),
                    input: serde_json::from_str(item["arguments"].as_str().unwrap_or("{}")).unwrap_or(Value::Null),
                };
                return sink(ModelEvent::ToolCall(call));
            }
            "response.completed" => {
                self.read_usage(&event["response"]["usage"]);
                self.end = Some(Ok(if self.called { StopReason::ToolUse } else { StopReason::EndTurn }));
            }
            "response.incomplete" => {
                self.read_usage(&event["response"]["usage"]);
                let reason = event["response"]["incomplete_details"]["reason"].as_str().unwrap_or("unknown");
                self.end = Some(Ok(if reason == "max_output_tokens" { StopReason::MaxTokens } else { StopReason::Other(format!("incomplete: {reason}")) }));
            }
            "response.failed" => self.end = Some(Err(error_of(&event["response"]))),
            "error" => self.end = Some(Err(error_of(&event))),
            _ => {}
        }
        true
    }

    fn read_usage(&mut self, usage: &Value) {
        let (all, cached) = (usage["input_tokens"].as_u64().unwrap_or(0), usage["input_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0));
        self.usage = Usage { input: all.saturating_sub(cached), output: usage["output_tokens"].as_u64().unwrap_or(0), cache_read: cached, cache_write: 0 };
    }

    /// A stream that stops with no terminal event did not succeed.
    pub fn finish(self) -> PluginResult<TurnEnd> {
        match self.end {
            Some(Ok(stop)) => Ok(TurnEnd { stop, usage: self.usage, cost_micro_usd: None }),
            Some(Err(message)) => Err(PluginError::new(message)),
            None => Err(PluginError::new("the stream ended before response.completed")),
        }
    }
}

impl Provider for Responses {
    fn name(&self) -> &str {
        &self.name
    }

    fn models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo { id: "configured".into(), context_window: self.context_window, supports_tools: true }]
    }

    fn turn<'a>(&'a self, request: &'a ModelRequest, sink: &'a mut dyn FnMut(ModelEvent) -> bool) -> BoxFuture<'a, PluginResult<TurnEnd>> {
        Box::pin(async move {
            let key = self.auth.credentials(&self.name).await?;
            let headers = vec![("content-type".to_string(), "application/json".to_string()), ("authorization".to_string(), format!("Bearer {}", key.expose()))];
            let url = format!("{}/responses", self.base_url);
            let mut response = self.net.send(NetRequest { method: "POST".into(), url, headers, body: Some(self.body(request).to_string().into_bytes()) }).await?;
            if response.status != 200 {
                let mut text = Vec::new();
                while let Ok(Some(chunk)) = response.body.next().await {
                    text.extend(chunk);
                    if text.len() > 64 * 1024 {
                        break;
                    }
                }
                let parsed: Value = serde_json::from_slice(&text).unwrap_or(Value::Null);
                let detail = if parsed["error"].is_object() { error_of(&parsed) } else { String::from_utf8_lossy(&text).chars().take(500).collect() };
                return Err(PluginError::new(format!("{} answered {}: {detail}", self.name, response.status)));
            }
            let (mut parser, mut reader) = (SseParser::default(), Reader::default());
            while let Some(chunk) = response.body.next().await? {
                for data in parser.feed(&chunk) {
                    if !reader.event(&data, sink) {
                        return Ok(TurnEnd { stop: StopReason::Stopped, usage: reader.usage, cost_micro_usd: None });
                    }
                }
            }
            let mut end = reader.finish()?;
            if !self.on_plan && self.base_url.contains("api.openai.com") {
                end.cost_micro_usd = crate::prices::cost_micro_usd(&request.model, &end.usage);
            }
            Ok(end)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> ModelRequest {
        ModelRequest {
            model: "m".into(),
            system: Some("be brief".into()),
            messages: vec![
                Message::user("hi"),
                Message { role: Role::Assistant, content: vec![Content::Text { text: "reading".into() }, Content::ToolUse { id: "c1".into(), name: "read".into(), input: json!({"path": "a"}) }] },
                Message { role: Role::User, content: vec![Content::ToolResult { id: "c1".into(), output: "text".into(), is_error: false }] },
            ],
            tools: vec![ToolSpec { name: "read".into(), description: "reads".into(), schema: json!({"type": "object"}) }],
            max_tokens: 100,
        }
    }

    struct NoAuth;
    impl Auth for NoAuth {
        fn credentials(&self, _: &str) -> BoxFuture<'_, PluginResult<Secret>> {
            Box::pin(std::future::ready(Ok(Secret::new("t"))))
        }
    }
    struct NoNet;
    impl Net for NoNet {
        fn send(&self, _: NetRequest) -> BoxFuture<'_, PluginResult<harness_api::platform::NetResponse>> {
            Box::pin(std::future::ready(Err(PluginError::new("no network here"))))
        }
    }

    #[test]
    fn the_request_is_what_the_plan_route_accepts() {
        let p = Responses::new("c", Rc::new(NoAuth), Rc::new(NoNet), &json!({}));
        let body = p.body(&request());
        assert_eq!((body["store"].clone(), body["stream"].clone()), (json!(false), json!(true)));
        assert_eq!(body["instructions"], "be brief");
        for refused in ["max_output_tokens", "temperature", "previous_response_id", "metadata", "top_p", "user"] {
            assert!(body.get(refused).is_none(), "{refused}");
        }
        assert!(!body["input"].to_string().contains("\"system\""), "system goes in instructions");
        assert_eq!(body["input"].as_array().unwrap().iter().map(|i| i["type"].as_str().unwrap()).collect::<Vec<_>>(), ["message", "message", "function_call", "function_call_output"]);
        assert_eq!(body["input"][1]["content"][0]["type"], "output_text");
        assert_eq!(body["input"][2]["arguments"], "{\"path\":\"a\"}");
        assert_eq!(body["input"][3], json!({"type": "function_call_output", "call_id": "c1", "output": "text"}));
        assert_eq!(body["tools"][0]["parameters"], json!({"type": "object"}));
        assert_eq!(body["reasoning"]["summary"], "auto", "reasoning is shown by default");
        let off = Responses::new("c", Rc::new(NoAuth), Rc::new(NoNet), &json!({"reasoning_summary": "off"}));
        assert!(off.body(&request()).get("reasoning").is_none());
    }

    fn read(events: &[Value]) -> (Vec<ModelEvent>, PluginResult<TurnEnd>) {
        let mut out = Vec::new();
        let mut reader = Reader::default();
        let mut sink = |m: ModelEvent| {
            out.push(m);
            true
        };
        for event in events {
            reader.event(&event.to_string(), &mut sink);
        }
        let end = reader.finish();
        (out, end)
    }

    #[test]
    fn text_a_call_and_usage_are_read() {
        let (out, end) = read(&[
            json!({"type": "response.reasoning_summary_text.delta", "delta": "Hm"}),
            json!({"type": "response.output_text.delta", "delta": "Hi"}),
            json!({"type": "response.output_item.done", "item": {"type": "message"}}),
            json!({"type": "response.output_item.done", "item": {"type": "function_call", "call_id": "c1", "name": "read", "arguments": "{\"path\":\"a\"}"}}),
            json!({"type": "response.completed", "response": {"usage": {"input_tokens": 10, "input_tokens_details": {"cached_tokens": 4}, "output_tokens": 3}}}),
        ]);
        assert_eq!(out, vec![ModelEvent::Thinking("Hm".into()), ModelEvent::Text("Hi".into()), ModelEvent::ToolCall(ToolCall { id: "c1".into(), name: "read".into(), input: json!({"path": "a"}) })]);
        let end = end.unwrap();
        assert_eq!((end.stop, end.usage.input, end.usage.cache_read, end.usage.output), (StopReason::ToolUse, 6, 4, 3));
    }

    #[test]
    fn only_completed_is_success() {
        assert!(read(&[json!({"type": "response.output_text.delta", "delta": "Hi"})]).1.unwrap_err().to_string().contains("before response.completed"));
        let limit = read(&[json!({"type": "response.failed", "response": {"error": {"code": "subscription_sharing_usage_limit_exceeded", "message": "x"}}})]).1.unwrap_err().to_string();
        assert!(limit.contains("usage limit"), "{limit}");
        let (_, end) = read(&[json!({"type": "response.incomplete", "response": {"incomplete_details": {"reason": "max_output_tokens"}}})]);
        assert_eq!(end.unwrap().stop, StopReason::MaxTokens);
        let (_, end) = read(&[json!({"type": "response.incomplete", "response": {"incomplete_details": {"reason": "content_filter"}}})]);
        assert_eq!(end.unwrap().stop, StopReason::Other("incomplete: content_filter".into()));
    }
}
