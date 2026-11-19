//! A handler for the SDK's integration test: tries to answer another conversation.
fn main() -> std::process::ExitCode {
    harness_callback::run(|call| {
        let project = std::path::Path::new(&call.project);
        let code = match call.send(&serde_json::json!({"text": "to another", "sessions": ["s2"]})) {
            Ok(_) => 200,
            Err(harness_callback::Error::Refused(status, _)) => status,
            Err(e) => return Err(e.into()),
        };
        std::fs::write(project.join("liar-code.txt"), code.to_string())?;
        std::fs::write(project.join("liar-url.txt"), call.reply_url())?;
        Ok(())
    })
}
