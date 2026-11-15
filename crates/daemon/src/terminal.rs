//! A new terminal window running multitool, opened by the daemon for Telegram's /open and /new
//! (CON-167). The window runs in a systemd scope of its own, so the daemon's restarts leave it be.
//!
//! The terminal is the one the user names (`[telegram] terminal`, a command the multitool command
//! line is put after), else the first found of the usual ones. Linux only for now.

/// The terminals tried, in order, each with what goes before the command it runs.
const KNOWN: [(&str, &[&str]); 4] = [("xdg-terminal-exec", &[]), ("ptyxis", &["--new-window", "--"]), ("gnome-terminal", &["--"]), ("x-terminal-emulator", &["-e"])];

/// The command line that opens a window running `command`: the named terminal, or the first of
/// [`KNOWN`] that `on_path` finds.
pub fn command_line(configured: Option<&str>, on_path: impl Fn(&str) -> bool, command: &[String]) -> Result<Vec<String>, String> {
    let mut line: Vec<String> = match configured.map(str::trim).filter(|c| !c.is_empty()) {
        Some(named) => named.split_whitespace().map(str::to_string).collect(),
        None => {
            let (program, before) = KNOWN.iter().find(|(program, _)| on_path(program)).ok_or("no terminal found: set [telegram] terminal in the settings to the command that opens one")?;
            std::iter::once(program.to_string()).chain(before.iter().map(|a| a.to_string())).collect()
        }
    };
    line.extend(command.iter().cloned());
    Ok(line)
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|dirs| std::env::split_paths(&dirs).any(|dir| dir.join(program).is_file()))
}

/// Opens a terminal window running multitool with `args` in `root`. Answers which terminal.
pub fn open(configured: Option<&str>, root: &str, args: &[&str]) -> Result<String, String> {
    if !cfg!(target_os = "linux") {
        return Err("opening a terminal from Telegram works on Linux only for now".to_string());
    }
    if !std::path::Path::new(root).is_dir() {
        return Err(format!("{root} is not there any more"));
    }
    let program = crate::own_program().map_err(|e| e.to_string())?;
    let mut command = vec![program.to_string_lossy().into_owned(), "--dir".to_string(), root.to_string()];
    command.extend(args.iter().map(|a| a.to_string()));
    let line = command_line(configured, on_path, &command)?;
    let mut spawn = harness_platform_native::scope::window_command(&line[0]);
    spawn.args(&line[1..]).current_dir(root).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let mut child = spawn.spawn().map_err(|e| format!("{}: {e}", line[0]))?;
    // Reaped when it ends: a terminal's launcher returns at once, the window lives on elsewhere.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(line[0].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd() -> Vec<String> {
        ["/bin/multitool", "--dir", "/p", "--resume", "s1"].map(String::from).to_vec()
    }

    /// The named terminal wins; else the first found, each with its own form.
    #[test]
    fn the_terminal_is_the_named_one_or_the_first_found() {
        assert_eq!(command_line(Some("kitty --hold"), |_| true, &cmd()).unwrap(), ["kitty", "--hold", "/bin/multitool", "--dir", "/p", "--resume", "s1"]);
        assert_eq!(command_line(None, |p| p == "xdg-terminal-exec", &cmd()).unwrap()[..2], ["xdg-terminal-exec", "/bin/multitool"]);
        assert_eq!(command_line(None, |p| p == "ptyxis", &cmd()).unwrap()[..4], ["ptyxis", "--new-window", "--", "/bin/multitool"]);
        assert_eq!(command_line(None, |p| p == "gnome-terminal", &cmd()).unwrap()[..3], ["gnome-terminal", "--", "/bin/multitool"]);
        assert_eq!(command_line(None, |p| p == "x-terminal-emulator", &cmd()).unwrap()[..3], ["x-terminal-emulator", "-e", "/bin/multitool"]);
        assert!(command_line(None, |_| false, &cmd()).unwrap_err().contains("[telegram] terminal"));
        assert_eq!(command_line(Some("  "), |p| p == "ptyxis", &cmd()).unwrap()[0], "ptyxis", "a blank setting is no setting");
    }
}
