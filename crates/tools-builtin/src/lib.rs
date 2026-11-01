//! The built-in tools (IF-4): plugins of the same interface as anyone's. Each one declares what
//! a call needs and refuses to touch anything it was not granted (INV-3).

use harness_api::platform::NetRequest;
use harness_api::*;
use serde_json::{Value, json};

const MAX_OUTPUT: usize = 30_000;

fn resolve(root: &str, path: &str) -> String {
    if path.starts_with('/') || path.get(1..3) == Some(":\\") { path.to_string() } else { format!("{}/{path}", root.trim_end_matches('/')) }
}

fn text_arg<'a>(input: &'a Value, name: &str) -> Result<&'a str, String> {
    input[name].as_str().ok_or_else(|| format!("{name} must be a string"))
}

/// The capability check every tool makes before it acts.
fn granted(cx: &ToolContext, need: &Capability) -> Result<(), ToolOutput> {
    if cx.granted.contains(need) { Ok(()) } else { Err(ToolOutput::error(format!("not granted: {need:?}"))) }
}

fn clip(mut text: String) -> String {
    if text.len() > MAX_OUTPUT {
        let mut cut = MAX_OUTPUT;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        let total = text.len();
        text.truncate(cut);
        text.push_str(&format!("\n… cut off: {total} bytes in all"));
    }
    text
}

pub struct Read;

impl Tool for Read {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read".into(),
            description: "Read a text file. Paths are relative to the project.".into(),
            schema: json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
        }
    }

    fn capabilities(&self, input: &Value) -> Result<Vec<Capability>, String> {
        Ok(vec![Capability::ReadPath(text_arg(input, "path")?.to_string())])
    }

    fn execute<'a>(&'a self, input: &'a Value, cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let Ok(path) = text_arg(input, "path") else { return ToolOutput::error("path must be a string") };
            if let Err(e) = granted(cx, &Capability::ReadPath(path.to_string())) {
                return e;
            }
            match cx.platform.fs.read(&resolve(&cx.root, path)).await {
                Ok(bytes) => ToolOutput::ok(clip(String::from_utf8_lossy(&bytes).into_owned())),
                Err(e) => ToolOutput::error(e.to_string()),
            }
        })
    }
}

pub struct Edit;

impl Tool for Edit {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit".into(),
            description: "Replace old_text with new_text in a file; old_text must occur exactly once. With old_text empty, writes new_text as the whole file.".into(),
            schema: json!({"type": "object", "properties": {"path": {"type": "string"}, "old_text": {"type": "string"}, "new_text": {"type": "string"}}, "required": ["path", "old_text", "new_text"]}),
        }
    }

    fn capabilities(&self, input: &Value) -> Result<Vec<Capability>, String> {
        text_arg(input, "old_text")?;
        text_arg(input, "new_text")?;
        Ok(vec![Capability::WritePath(text_arg(input, "path")?.to_string())])
    }

    fn execute<'a>(&'a self, input: &'a Value, cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let (Ok(path), Ok(old), Ok(new)) = (text_arg(input, "path"), text_arg(input, "old_text"), text_arg(input, "new_text")) else {
                return ToolOutput::error("path, old_text and new_text must be strings");
            };
            if let Err(e) = granted(cx, &Capability::WritePath(path.to_string())) {
                return e;
            }
            let full = resolve(&cx.root, path);
            let updated = if old.is_empty() {
                new.to_string()
            } else {
                let current = match cx.platform.fs.read(&full).await {
                    Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                    Err(e) => return ToolOutput::error(e.to_string()),
                };
                match current.matches(old).count() {
                    1 => current.replacen(old, new, 1),
                    0 => return ToolOutput::error("old_text does not occur in the file"),
                    n => return ToolOutput::error(format!("old_text occurs {n} times; it must be unique")),
                }
            };
            match cx.platform.fs.write(&full, updated.as_bytes()).await {
                Ok(()) => ToolOutput::ok(format!("wrote {path}")),
                Err(e) => ToolOutput::error(e.to_string()),
            }
        })
    }
}

pub struct Shell;

impl Tool for Shell {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "shell".into(),
            description: "Run a shell command in the project's directory.".into(),
            schema: json!({"type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"]}),
        }
    }

    fn capabilities(&self, input: &Value) -> Result<Vec<Capability>, String> {
        Ok(vec![Capability::Command(text_arg(input, "command")?.to_string())])
    }

    fn execute<'a>(&'a self, input: &'a Value, cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let Ok(command) = text_arg(input, "command") else { return ToolOutput::error("command must be a string") };
            if let Err(e) = granted(cx, &Capability::Command(command.to_string())) {
                return e;
            }
            let Some(process) = &cx.platform.process else { return ToolOutput::error("this platform has no processes") };
            match process.run(command, Some(&cx.root), &[]).await {
                Ok(out) => {
                    let mut text = out.stdout;
                    text.push_str(&out.stderr);
                    let ok = out.code == Some(0);
                    if !ok {
                        text.push_str(&format!("\nexit {}", out.code.map_or("none".to_string(), |c| c.to_string())));
                    }
                    ToolOutput { text: clip(text), is_error: !ok }
                }
                Err(e) => ToolOutput::error(e.to_string()),
            }
        })
    }
}

pub struct Search;

impl Search {
    /// Files under `dir`, breadth first, skipping hidden folders and build output.
    async fn walk(cx: &ToolContext, dir: String, limit: usize) -> Vec<String> {
        let mut out = Vec::new();
        let mut queue = vec![dir];
        while let Some(dir) = queue.pop() {
            let Ok(names) = cx.platform.fs.list(&dir).await else { continue };
            for name in names {
                let path = format!("{}/{name}", dir.trim_end_matches('/'));
                match name.strip_suffix('/') {
                    Some(folder) => {
                        if !folder.starts_with('.') && folder != "target" && folder != "node_modules" {
                            queue.push(path.trim_end_matches('/').to_string());
                        }
                    }
                    None => out.push(path),
                }
                if out.len() >= limit {
                    return out;
                }
            }
        }
        out
    }
}

impl Tool for Search {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "search".into(),
            description: "Find lines containing a text in the project's files. Prints path:line: text.".into(),
            schema: json!({"type": "object", "properties": {"text": {"type": "string"}, "path": {"type": "string", "description": "A folder to search in; the project by default."}}, "required": ["text"]}),
        }
    }

    fn capabilities(&self, input: &Value) -> Result<Vec<Capability>, String> {
        text_arg(input, "text")?;
        Ok(vec![Capability::ReadPath(input["path"].as_str().unwrap_or(".").to_string())])
    }

    fn execute<'a>(&'a self, input: &'a Value, cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let Ok(needle) = text_arg(input, "text") else { return ToolOutput::error("text must be a string") };
            let folder = input["path"].as_str().unwrap_or(".");
            if let Err(e) = granted(cx, &Capability::ReadPath(folder.to_string())) {
                return e;
            }
            let root = resolve(&cx.root, folder);
            let mut hits = Vec::new();
            // A file is searched as itself; a path that is neither is said, not "no matches".
            let files = if cx.platform.fs.list(&root).await.is_ok() {
                Self::walk(cx, root.clone(), 20_000).await
            } else if cx.platform.fs.read(&root).await.is_ok() {
                vec![root.clone()]
            } else {
                return ToolOutput::error(format!("no such file or folder: {folder}"));
            };
            for file in files {
                let Ok(bytes) = cx.platform.fs.read(&file).await else { continue };
                if bytes.contains(&0) {
                    continue;
                }
                let shown = file.strip_prefix(&format!("{}/", cx.root.trim_end_matches('/'))).unwrap_or(&file).to_string();
                for (i, line) in String::from_utf8_lossy(&bytes).lines().enumerate() {
                    if line.contains(needle) {
                        hits.push(format!("{shown}:{}: {}", i + 1, line.trim()));
                    }
                }
                if hits.len() > 500 {
                    break;
                }
            }
            if hits.is_empty() { ToolOutput::ok("no matches") } else { ToolOutput::ok(clip(hits.join("\n"))) }
        })
    }
}

pub struct Fetch;

fn host_of(url: &str) -> Result<&str, String> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).ok_or("url must start with http:// or https://")?;
    Ok(rest.split(['/', '?', '#']).next().unwrap_or(rest))
}

impl Tool for Fetch {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "fetch".into(),
            description: "Fetch a web page or API over HTTP(S) and return its body as text.".into(),
            schema: json!({"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]}),
        }
    }

    fn capabilities(&self, input: &Value) -> Result<Vec<Capability>, String> {
        Ok(vec![Capability::Host(host_of(text_arg(input, "url")?)?.to_string())])
    }

    fn execute<'a>(&'a self, input: &'a Value, cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            let Ok(url) = text_arg(input, "url") else { return ToolOutput::error("url must be a string") };
            let Ok(host) = host_of(url) else { return ToolOutput::error("url must start with http:// or https://") };
            if let Err(e) = granted(cx, &Capability::Host(host.to_string())) {
                return e;
            }
            let mut response = match cx.platform.net.send(NetRequest { method: "GET".into(), url: url.to_string(), headers: vec![], body: None }).await {
                Ok(r) => r,
                Err(e) => return ToolOutput::error(e.to_string()),
            };
            let mut bytes = Vec::new();
            loop {
                match response.body.next().await {
                    Ok(Some(chunk)) => bytes.extend(chunk),
                    Ok(None) => break,
                    Err(e) => return ToolOutput::error(e.to_string()),
                }
                if bytes.len() > MAX_OUTPUT * 2 {
                    break;
                }
            }
            ToolOutput { text: clip(String::from_utf8_lossy(&bytes).into_owned()), is_error: response.status >= 400 }
        })
    }
}
