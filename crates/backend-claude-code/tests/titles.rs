//! journal#1254: conversations are named as Claude Code's /resume names them.

use harness_api::ConversationFormat;
use harness_backend_claude_code::conversations::ClaudeCodeConversations;
use serde_json::json;

fn line(v: serde_json::Value) -> String {
    format!("{v}\n")
}

fn user(text: &str) -> String {
    line(json!({"type": "user", "uuid": "u1", "parentUuid": null, "message": {"role": "user", "content": text}, "timestamp": "2026-11-16T10:00:00Z"}))
}

#[test]
fn a_title_written_early_in_a_big_file_names_it_and_an_empty_one_is_left_out() {
    let dir = tempfile::tempdir().unwrap();
    let root = "/home/u/proj";
    let folder = dir.path().join("projects").join("-home-u-proj");
    std::fs::create_dir_all(&folder).unwrap();
    // A title near the start of a file well over 2 MiB, none at its end.
    let mut big = user("<pasted_content id=\"7ae1\">\n[callback]: {}\n</pasted_content>");
    big.push_str(&line(json!({"type": "ai-title", "aiTitle": "refactor-crypto-classifier-prompts", "sessionId": "big"})));
    big.push_str(&line(json!({"type": "custom-title", "customTitle": "orchestrator-3", "sessionId": "big"})));
    let filler = line(json!({"type": "attachment", "content": "x".repeat(3000)}));
    while big.len() < 3 << 20 {
        big.push_str(&filler);
    }
    std::fs::write(folder.join("big.jsonl"), big).unwrap();
    // Only an ai-title: that one.
    std::fs::write(folder.join("ai.jsonl"), user("hello") + &line(json!({"type": "ai-title", "aiTitle": "Go-воркеры даунстрима"}))).unwrap();
    // No title: its first telling words.
    std::fs::write(folder.join("words.jsonl"), user("[Request interrupted by user]\nmake the feed faster")).unwrap();
    // No message at all: not listed.
    std::fs::write(folder.join("empty.jsonl"), line(json!({"type": "last-prompt", "lastPrompt": ""})) + &line(json!({"type": "permission-mode", "mode": "default"}))).unwrap();
    let source = ClaudeCodeConversations::new(dir.path());
    let listed = source.list(root).unwrap();
    let name = |id: &str| listed.iter().find(|c| c.id == id).map(|c| c.title.clone());
    assert_eq!(name("big").as_deref(), Some("orchestrator-3"), "a /rename name wins over the ai-title, found at the start");
    assert_eq!(name("ai").as_deref(), Some("Go-воркеры даунстрима"));
    assert_eq!(name("words").as_deref(), Some("make the feed faster"));
    assert_eq!(name("empty"), None, "an empty conversation is not listed, as /resume lists none");
}
