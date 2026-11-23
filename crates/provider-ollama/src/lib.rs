//! Ollama on its own API (IF-1, CON-190): POST /api/chat, streamed as NDJSON.
//!
//! Not its OpenAI-compatible /v1: that cannot set the context, and Ollama 0.33.3 loads a model
//! there with 32768 tokens while the harness counted on more (journal#1532). Here every request
//! sends `options.num_ctx` equal to the window the harness counts with, never more than the
//! model's own. What a model can do, tools and thinking, comes from /api/show, read once.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use harness_api::platform::{Net, NetRequest};
use harness_api::*;
use serde_json::{Value, json};

/// Ollama's usual address.
pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:11434";

/// The window a model is run with when its row names none (CON-190): enough for the harness's own
/// request and a conversation, and within what a small GPU holds.
pub const DEFAULT_CONTEXT: u64 = 65_536;

/// What /api/show says of a model.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Facts {
    pub tools: bool,
    pub thinking: bool,
    /// Its own context length, when known.
    pub context: Option<u64>,
}

impl Facts {
    /// From /api/show's answer.
    pub fn from_show(show: &Value) -> Facts {
        let has = |c: &str| show["capabilities"].as_array().is_some_and(|caps| caps.iter().any(|x| x == c));
        let context = show["model_info"].as_object().and_then(|info| info.iter().find(|(k, _)| k.ends_with(".context_length")).and_then(|(_, v)| v.as_u64()));
        Facts { tools: has("tools"), thinking: has("thinking"), context }
    }
}

pub struct Ollama {
    name: String,
    net: Rc<dyn Net>,
    base_url: String,
    context_window: u64,
    /// How long Ollama keeps the model after a request: -1, until it stops, unless the row says
    /// otherwise (CON-190 point 11). Ollama's own default would drop it after 5 idle minutes.
    keep_alive: Value,
    facts: RefCell<HashMap<String, Facts>>,
}

impl Ollama {
    /// Settings: `base_url` (Ollama's usual one), `context_window` (65536), `keep_alive` (-1).
    pub fn new(name: &str, net: Rc<dyn Net>, settings: &Value) -> Self {
        let base = settings["base_url"].as_str().unwrap_or(DEFAULT_BASE_URL).trim_end_matches('/');
        // A row written for /v1 still reaches the same server.
        let base = base.strip_suffix("/v1").unwrap_or(base).to_string();
        Ollama {
            name: name.to_string(),
            net,
            base_url: base,
            context_window: settings["context_window"].as_u64().unwrap_or(DEFAULT_CONTEXT),
            keep_alive: settings.get("keep_alive").filter(|v| !v.is_null()).cloned().unwrap_or(json!(-1)),
            facts: RefCell::new(HashMap::new()),
        }
    }

    fn not_running(&self, why: &str) -> PluginError {
        PluginError::new(format!("Ollama is not running at {}: start it with `ollama serve`, or `systemctl start ollama` ({why})", self.base_url))
    }

    fn refused(&self, model: &str, status: u16, body: &str) -> PluginError {
        let said = serde_json::from_str::<Value>(body).ok().and_then(|v| v["error"].as_str().map(str::to_string)).unwrap_or_else(|| body.trim().to_string());
        if said.contains("not found") {
            return PluginError::new(format!("the model {model} is not pulled here: `ollama pull {model}`"));
        }
        PluginError::new(format!("{} answered {status}: {said}", self.name))
    }

    async fn post(&self, path: &str, body: &Value) -> PluginResult<harness_api::platform::NetResponse> {
        let url = format!("{}{path}", self.base_url);
        let request = NetRequest { method: "POST".into(), url, headers: vec![("content-type".to_string(), "application/json".to_string())], body: Some(body.to_string().into_bytes()) };
        self.net.send(request).await.map_err(|e| if e.0 == TOOL_STOPPED { e } else { self.not_running(&e.0) })
    }

    async fn read_all(response: &mut harness_api::platform::NetResponse) -> String {
        let mut text = Vec::new();
        while let Ok(Some(chunk)) = response.body.next().await {
            text.extend(chunk);
            if text.len() > 4 * 1024 * 1024 {
                break;
            }
        }
        String::from_utf8_lossy(&text).into_owned()
    }

    /// What `model` can do, from /api/show, asked once.
    async fn facts(&self, model: &str) -> PluginResult<Facts> {
        if let Some(known) = self.facts.borrow().get(model) {
            return Ok(known.clone());
        }
        let mut response = self.post("/api/show", &json!({"model": model})).await?;
        let body = Self::read_all(&mut response).await;
        if response.status != 200 {
            return Err(self.refused(model, response.status, &body));
        }
        let facts = Facts::from_show(&serde_json::from_str(&body).unwrap_or(Value::Null));
        self.facts.borrow_mut().insert(model.to_string(), facts.clone());
        Ok(facts)
    }

    /// The window a turn runs with: the row's, never more than the model's own.
    pub fn num_ctx(&self, facts: &Facts) -> u64 {
        facts.context.map_or(self.context_window, |own| own.min(self.context_window))
    }

    /// The /api/chat request for a turn.
    pub fn body(&self, request: &ModelRequest, facts: &Facts) -> Value {
        let mut body = json!({
            "model": request.model,
            "stream": true,
            "messages": messages(request),
            "options": {"num_ctx": self.num_ctx(facts), "num_predict": request.max_tokens},
        });
        // Tools only to a model that can use them; any other talks only (CON-190).
        if facts.tools && !request.tools.is_empty() {
            body["tools"] = request.tools.iter().map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.schema}})).collect();
        }
        if facts.thinking {
            body["think"] = json!(true);
        }
        body["keep_alive"] = self.keep_alive.clone();
        body
    }
}

/// Our history as Ollama's chat messages: a user message of tool results becomes one `tool`
/// message each, with the call's id and its tool's name; a call's arguments go as an object.
fn messages(request: &ModelRequest) -> Vec<Value> {
    let mut names: HashMap<&str, &str> = HashMap::new();
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
                        Content::Text { text: t } => {
                            if !text.is_empty() {
                                text.push_str("\n\n");
                            }
                            text.push_str(t);
                        }
                        Content::ToolResult { id, output, .. } => {
                            out.push(json!({"role": "tool", "tool_call_id": id, "tool_name": names.get(id.as_str()).copied().unwrap_or_default(), "content": output}));
                        }
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
                            names.insert(id, name);
                            calls.push(json!({"id": id, "function": {"name": name, "arguments": input}}));
                        }
                        _ => {}
                    }
                }
                let mut message = json!({"role": "assistant", "content": text});
                if !calls.is_empty() {
                    message["tool_calls"] = json!(calls);
                }
                out.push(message);
            }
        }
    }
    out
}

/// Reads the NDJSON stream of a turn.
#[derive(Default)]
struct Reader {
    pending: Vec<u8>,
    usage: Usage,
    done: Option<String>,
    calls: usize,
    error: Option<String>,
}

impl Reader {
    /// One chunk of bytes; false when the user stopped.
    fn feed(&mut self, chunk: &[u8], sink: &mut dyn FnMut(ModelEvent) -> bool) -> bool {
        self.pending.extend_from_slice(chunk);
        while let Some(at) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=at).collect();
            if !self.line(&String::from_utf8_lossy(&line), sink) {
                return false;
            }
        }
        true
    }

    fn line(&mut self, line: &str, sink: &mut dyn FnMut(ModelEvent) -> bool) -> bool {
        let Ok(e) = serde_json::from_str::<Value>(line.trim()) else { return true };
        if let Some(error) = e["error"].as_str() {
            self.error = Some(error.to_string());
            return true;
        }
        let message = &e["message"];
        if let Some(thinking) = message["thinking"].as_str().filter(|t| !t.is_empty())
            && !sink(ModelEvent::Thinking(thinking.to_string()))
        {
            return false;
        }
        if let Some(text) = message["content"].as_str().filter(|t| !t.is_empty())
            && !sink(ModelEvent::Text(text.to_string()))
        {
            return false;
        }
        for call in message["tool_calls"].as_array().into_iter().flatten() {
            self.calls += 1;
            let function = &call["function"];
            // Arguments come as an object; a string of JSON is taken too.
            let input = match &function["arguments"] {
                Value::String(s) => serde_json::from_str(s).unwrap_or_else(|_| json!({"_invalid_json": s})),
                Value::Null => json!({}),
                other => other.clone(),
            };
            let id = call["id"].as_str().filter(|i| !i.is_empty()).map_or_else(|| format!("ollama_call_{}", self.calls), str::to_string);
            if !sink(ModelEvent::ToolCall(ToolCall { id, name: function["name"].as_str().unwrap_or_default().to_string(), input })) {
                return false;
            }
        }
        if e["done"] == true {
            let count = |k: &str| e[k].as_u64().unwrap_or(0);
            self.usage.cache_read = count("prompt_eval_cached_count");
            self.usage.input = count("prompt_eval_count").saturating_sub(self.usage.cache_read);
            self.usage.output = count("eval_count");
            self.done = Some(e["done_reason"].as_str().unwrap_or("stop").to_string());
        }
        true
    }

    fn finish(mut self, sink: &mut dyn FnMut(ModelEvent) -> bool) -> Result<TurnEnd, String> {
        if !self.pending.is_empty() {
            let rest = String::from_utf8_lossy(&std::mem::take(&mut self.pending)).into_owned();
            self.line(&rest, sink);
        }
        if let Some(error) = self.error {
            return Err(error);
        }
        let stop = match self.done.as_deref() {
            None => return Err("the stream ended before the turn did".to_string()),
            Some(_) if self.calls > 0 => StopReason::ToolUse,
            Some("length") => StopReason::MaxTokens,
            Some("stop") => StopReason::EndTurn,
            Some(other) => StopReason::Other(other.to_string()),
        };
        Ok(TurnEnd { stop, usage: self.usage, cost_micro_usd: Some(0) })
    }
}

impl Provider for Ollama {
    fn name(&self) -> &str {
        &self.name
    }

    fn models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo { id: "configured".into(), context_window: self.context_window, supports_tools: true }]
    }

    fn turn<'a>(&'a self, request: &'a ModelRequest, sink: &'a mut dyn FnMut(ModelEvent) -> bool) -> BoxFuture<'a, PluginResult<TurnEnd>> {
        Box::pin(async move {
            let facts = self.facts(&request.model).await?;
            let mut response = self.post("/api/chat", &self.body(request, &facts)).await?;
            if response.status != 200 {
                let body = Self::read_all(&mut response).await;
                return Err(self.refused(&request.model, response.status, &body));
            }
            let mut reader = Reader::default();
            while let Some(chunk) = response.body.next().await? {
                if !reader.feed(&chunk, sink) {
                    // Dropping the response closes the stream, and Ollama stops generating.
                    return Ok(TurnEnd { stop: StopReason::Stopped, usage: reader.usage, cost_micro_usd: Some(0) });
                }
            }
            reader.finish(sink).map_err(|e| if e.contains("not found") { self.refused(&request.model, 404, &json!({"error": e}).to_string()) } else { PluginError::new(format!("{}: {e}", self.name)) })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_api::platform::{NetBody, NetResponse};
    use std::cell::Cell;

    /// The lines Ollama 0.33.3 streamed for gemma4:e2b with think and a tool (journal#1532).
    const RECORDED: &[&str] = &[
        r#"{"model":"gemma4:e2b","created_at":"t","message":{"role":"assistant","content":"","thinking":"The user wants"},"done":false}"#,
        r#"{"model":"gemma4:e2b","created_at":"t","message":{"role":"assistant","content":"","thinking":" the weather."},"done":false}"#,
        r#"{"model":"gemma4:e2b","created_at":"t","message":{"role":"assistant","content":"","tool_calls":[{"id":"call_51v5wxmk","function":{"index":0,"name":"get_weather","arguments":{"city":"Paris"}}}]},"done":false}"#,
        r#"{"model":"gemma4:e2b","created_at":"t","done":true,"done_reason":"stop","total_duration":12268620976,"load_duration":8883143834,"prompt_eval_count":68,"prompt_eval_cached_count":8,"prompt_eval_duration":218553000,"eval_count":163,"eval_duration":3159381000}"#,
    ];

    struct Chunks(Vec<Vec<u8>>);
    impl NetBody for Chunks {
        fn next(&mut self) -> BoxFuture<'_, PluginResult<Option<Vec<u8>>>> {
            let next = if self.0.is_empty() { None } else { Some(self.0.remove(0)) };
            Box::pin(std::future::ready(Ok(next)))
        }
    }

    /// A Net that answers /api/show and /api/chat as told, and keeps the bodies it was sent.
    struct Fake {
        show: (u16, String),
        chat: (u16, Vec<Vec<u8>>),
        refuse: bool,
        sent: RefCell<Vec<(String, Value)>>,
        shows: Cell<u32>,
    }
    impl Fake {
        fn new(show: Value, chat: Vec<Vec<u8>>) -> Rc<Fake> {
            Rc::new(Fake { show: (200, show.to_string()), chat: (200, chat), refuse: false, sent: RefCell::new(Vec::new()), shows: Cell::new(0) })
        }
    }
    impl Net for Fake {
        fn send(&self, request: NetRequest) -> BoxFuture<'_, PluginResult<NetResponse>> {
            if self.refuse {
                return Box::pin(std::future::ready(Err(PluginError::new("request to 127.0.0.1:11434: io: Connection refused"))));
            }
            let body: Value = serde_json::from_slice(request.body.as_deref().unwrap_or_default()).unwrap_or(Value::Null);
            self.sent.borrow_mut().push((request.url.clone(), body));
            let (status, chunks) = if request.url.ends_with("/api/show") {
                self.shows.set(self.shows.get() + 1);
                (self.show.0, vec![self.show.1.clone().into_bytes()])
            } else {
                (self.chat.0, self.chat.1.clone())
            };
            Box::pin(std::future::ready(Ok(NetResponse { status, body: Box::new(Chunks(chunks)) })))
        }
    }

    fn show(caps: &[&str], context: u64) -> Value {
        json!({"capabilities": caps, "model_info": {"general.architecture": "gemma4", "gemma4.context_length": context}})
    }

    fn request() -> ModelRequest {
        ModelRequest {
            model: "gemma4:e2b".into(),
            system: Some("be brief".into()),
            messages: vec![
                Message::user("weather in Paris?"),
                Message { role: Role::Assistant, content: vec![Content::Text { text: "Asking.".into() }, Content::ToolUse { id: "c1".into(), name: "get_weather".into(), input: json!({"city": "Paris"}) }] },
                Message { role: Role::User, content: vec![Content::ToolResult { id: "c1".into(), output: "18 C".into(), is_error: false }] },
            ],
            tools: vec![ToolSpec { name: "get_weather".into(), description: "weather".into(), schema: json!({"type": "object"}) }],
            max_tokens: 1000,
        }
    }

    fn lines(ls: &[&str]) -> Vec<Vec<u8>> {
        // Split across chunks the way a socket may: mid-line.
        let all: String = ls.iter().map(|l| format!("{l}\n")).collect();
        let bytes = all.into_bytes();
        bytes.chunks(37).map(<[u8]>::to_vec).collect()
    }

    fn run(p: &Ollama, r: &ModelRequest, stop_after: Option<usize>) -> (Vec<ModelEvent>, PluginResult<TurnEnd>) {
        let mut out = Vec::new();
        let mut sink = |m: ModelEvent| {
            out.push(m);
            stop_after.is_none_or(|n| out.len() < n)
        };
        let end = pollster::block_on(p.turn(r, &mut sink));
        (out, end)
    }

    #[test]
    fn a_recorded_turn_is_read_into_thinking_a_call_and_usage() {
        let net = Fake::new(show(&["completion", "tools", "thinking"], 131_072), lines(RECORDED));
        let p = Ollama::new("ollama", net.clone(), &json!({}));
        let (out, end) = run(&p, &request(), None);
        assert_eq!(out, vec![
            ModelEvent::Thinking("The user wants".into()),
            ModelEvent::Thinking(" the weather.".into()),
            ModelEvent::ToolCall(ToolCall { id: "call_51v5wxmk".into(), name: "get_weather".into(), input: json!({"city": "Paris"}) }),
        ]);
        let end = end.unwrap();
        assert_eq!(end.stop, StopReason::ToolUse);
        assert_eq!((end.usage.input, end.usage.cache_read, end.usage.output), (60, 8, 163), "the cached part is taken out of the input");
        assert_eq!(end.cost_micro_usd, Some(0));
    }

    /// CON-190 point 11 (journal#1679): with no keep_alive of the row's own, the model stays loaded
    /// (-1), instead of Ollama's 5 idle minutes; a row's own value goes as it is.
    #[test]
    fn the_model_stays_loaded_unless_the_row_says_otherwise() {
        let sent_keep_alive = |settings: Value| {
            let net = Fake::new(show(&["completion"], 131_072), lines(RECORDED));
            run(&Ollama::new("ollama", net.clone(), &settings), &request(), None).1.unwrap();
            let sent = net.sent.borrow();
            sent.iter().find(|(u, _)| u.ends_with("/api/chat")).unwrap().1["keep_alive"].clone()
        };
        assert_eq!(sent_keep_alive(json!({})), json!(-1), "kept until Ollama stops");
        assert_eq!(sent_keep_alive(json!({"keep_alive": null})), json!(-1));
        assert_eq!(sent_keep_alive(json!({"keep_alive": "30m"})), json!("30m"));
        assert_eq!(sent_keep_alive(json!({"keep_alive": 0})), json!(0), "0 unloads at once");
    }

    #[test]
    fn the_request_holds_the_window_tools_think_and_tool_results() {
        let net = Fake::new(show(&["completion", "tools", "thinking"], 131_072), lines(RECORDED));
        let p = Ollama::new("ollama", net.clone(), &json!({"context_window": 65_536, "keep_alive": "10m"}));
        run(&p, &request(), None).1.unwrap();
        let sent = net.sent.borrow();
        let (url, body) = sent.iter().find(|(u, _)| u.ends_with("/api/chat")).unwrap();
        assert_eq!(url, "http://127.0.0.1:11434/api/chat");
        assert_eq!(body["options"]["num_ctx"], 65_536);
        assert_eq!(body["options"]["num_predict"], 1000);
        assert_eq!(body["think"], true);
        assert_eq!(body["keep_alive"], "10m");
        assert_eq!(body["stream"], true);
        assert_eq!(body["tools"][0]["function"]["name"], "get_weather");
        let m = body["messages"].as_array().unwrap();
        assert_eq!(m[0], json!({"role": "system", "content": "be brief"}));
        assert_eq!(m[2]["tool_calls"][0], json!({"id": "c1", "function": {"name": "get_weather", "arguments": {"city": "Paris"}}}), "arguments go as an object");
        assert_eq!(m[3], json!({"role": "tool", "tool_call_id": "c1", "tool_name": "get_weather", "content": "18 C"}));
        // /api/show is asked once per model.
        drop(sent);
        run(&p, &request(), None).1.unwrap();
        assert_eq!(net.shows.get(), 1);
    }

    #[test]
    fn the_window_is_capped_by_the_model_and_its_capabilities_decide() {
        // A model of 8k with no tools and no thinking: talk only, at 8k.
        let net = Fake::new(show(&["completion"], 8192), lines(&[r#"{"message":{"content":"hi"},"done":false}"#, r#"{"done":true,"done_reason":"stop","prompt_eval_count":5,"eval_count":1}"#]));
        let p = Ollama::new("ollama", net.clone(), &json!({"base_url": "http://127.0.0.1:11434/v1/"}));
        let (out, end) = run(&p, &request(), None);
        assert_eq!(out, vec![ModelEvent::Text("hi".into())]);
        assert_eq!(end.unwrap().stop, StopReason::EndTurn);
        let sent = net.sent.borrow();
        let (url, body) = sent.iter().find(|(u, _)| u.ends_with("/api/chat")).unwrap();
        assert_eq!(url, "http://127.0.0.1:11434/api/chat", "a /v1 address reaches the same server");
        assert_eq!(body["options"]["num_ctx"], 8192, "never more than the model's own");
        assert!(body.get("tools").is_none() && body.get("think").is_none(), "{body}");
    }

    #[test]
    fn a_call_with_no_id_gets_one_and_string_arguments_are_read() {
        let net = Fake::new(show(&["tools"], 4096), lines(&[r#"{"message":{"tool_calls":[{"function":{"name":"read","arguments":"{\"path\":\"a\"}"}}]},"done":false}"#, r#"{"done":true,"done_reason":"stop"}"#]));
        let p = Ollama::new("ollama", net, &json!({}));
        let (out, _) = run(&p, &request(), None);
        assert_eq!(out, vec![ModelEvent::ToolCall(ToolCall { id: "ollama_call_1".into(), name: "read".into(), input: json!({"path": "a"}) })]);
    }

    #[test]
    fn a_stop_ends_the_turn_at_once() {
        let net = Fake::new(show(&["thinking"], 4096), lines(RECORDED));
        let p = Ollama::new("ollama", net, &json!({}));
        let (out, end) = run(&p, &request(), Some(1));
        assert_eq!(out.len(), 1);
        assert_eq!(end.unwrap().stop, StopReason::Stopped);
    }

    #[test]
    fn failures_say_what_to_do() {
        // Nothing listening.
        let net = Rc::new(Fake { show: (200, String::new()), chat: (200, vec![]), refuse: true, sent: RefCell::new(Vec::new()), shows: Cell::new(0) });
        let p = Ollama::new("ollama", net, &json!({}));
        let e = run(&p, &request(), None).1.unwrap_err().0;
        assert!(e.starts_with("Ollama is not running at http://127.0.0.1:11434: start it with `ollama serve`"), "{e}");
        // A model not pulled: /api/show says 404 not found.
        let net = Rc::new(Fake { show: (404, r#"{"error":"model 'gemma9:1b' not found"}"#.into()), chat: (200, vec![]), refuse: false, sent: RefCell::new(Vec::new()), shows: Cell::new(0) });
        let p = Ollama::new("ollama", net, &json!({}));
        let mut r = request();
        r.model = "gemma9:1b".into();
        let e = run(&p, &r, None).1.unwrap_err().0;
        assert_eq!(e, "the model gemma9:1b is not pulled here: `ollama pull gemma9:1b`");
        // An error in the stream.
        let net = Fake::new(show(&[], 4096), lines(&[r#"{"error":"out of memory"}"#]));
        let p = Ollama::new("ollama", net, &json!({}));
        let e = run(&p, &request(), None).1.unwrap_err().0;
        assert_eq!(e, "ollama: out of memory");
        // A stream cut short.
        let net = Fake::new(show(&[], 4096), lines(&[r#"{"message":{"content":"par"},"done":false}"#]));
        let p = Ollama::new("ollama", net, &json!({}));
        assert!(run(&p, &request(), None).1.unwrap_err().0.contains("ended before the turn did"));
    }
}
