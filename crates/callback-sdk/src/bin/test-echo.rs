//! A handler for the SDK's integration test: notes what it got in the project, then answers its
//! run with a message.
fn main() -> std::process::ExitCode {
    harness_callback::run(|call| {
        let project = std::path::Path::new(&call.project);
        std::fs::write(project.join("echo-stdin.json"), call.input.to_string())?;
        std::fs::write(project.join("echo-sessions.txt"), &call.session)?;
        std::fs::write(project.join("echo-args.txt"), call.args.join(" "))?;
        let reply = call.message(&format!("echo {} for {}", call.kind, call.session))?;
        std::fs::write(project.join("echo-reply.txt"), reply.result)?;
        Ok(())
    })
}
