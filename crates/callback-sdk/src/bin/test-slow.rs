//! A handler for the SDK's integration test: answers past its time, so it is killed first.
fn main() -> std::process::ExitCode {
    harness_callback::run(|call| {
        std::thread::sleep(std::time::Duration::from_secs(20));
        call.message("too late")?;
        Ok(())
    })
}
