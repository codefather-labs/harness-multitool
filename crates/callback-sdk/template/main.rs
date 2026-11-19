//! The handler of the custom callback {{name}} (CON-183). The daemon runs it when a callback of
//! one of the types in config.toml reaches a conversation subscribed to it in Settings > Callbacks.
//! Build it with `multitool callback build {{name}}`; the contract is in docs/callbacks.md.

fn main() -> std::process::ExitCode {
    harness_callback::run(|call| {
        // call.kind: the type; call.input: the callback's JSON; call.session: the conversation;
        // call.args: config.toml's args. Run anything here, in any language, as a command.
        call.message(&format!("{} came: {}", call.kind, call.input))?;
        Ok(())
    })
}
