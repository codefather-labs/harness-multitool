//! The system's file picker, for the Chats tab's Load (CON-173): a dialog program run as a child,
//! the first found of zenity and kdialog on Linux, osascript on macOS, PowerShell's
//! OpenFileDialog on Windows (journal#1296). It blocks, so the window runs it off its thread.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    Mac,
    Windows,
}

impl Os {
    pub fn this() -> Os {
        if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

/// What came of a pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picked {
    Path(PathBuf),
    Cancelled,
    /// No picker could be opened, and why.
    Unavailable(String),
}

const TITLE: &str = "Load a conversation";

/// The command line that opens a picker for a .zip starting in `start`, or None when `on_path`
/// finds none.
pub fn command_line(os: Os, on_path: impl Fn(&str) -> bool, start: &Path) -> Option<Vec<String>> {
    let dir = start.to_string_lossy();
    let line: Vec<String> = match os {
        Os::Linux if on_path("zenity") => vec!["zenity".into(), "--file-selection".into(), format!("--title={TITLE}"), "--file-filter=Chat archives | *.zip".into(), format!("--filename={}/", dir.trim_end_matches('/'))],
        Os::Linux if on_path("kdialog") => vec!["kdialog".into(), "--title".into(), TITLE.into(), "--getopenfilename".into(), dir.into_owned(), "*.zip|Chat archives".into()],
        Os::Linux => return None,
        Os::Mac => {
            let quoted = dir.replace('\\', "\\\\").replace('"', "\\\"");
            vec!["osascript".into(), "-e".into(), format!("POSIX path of (choose file with prompt \"{TITLE}\" of type {{\"zip\"}} default location (POSIX file \"{quoted}\"))")]
        }
        Os::Windows => {
            let quoted = dir.replace('\'', "''");
            let script = format!("Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.OpenFileDialog; $d.Title = '{TITLE}'; $d.Filter = 'Chat archives (*.zip)|*.zip'; $d.InitialDirectory = '{quoted}'; if ($d.ShowDialog() -eq 'OK') {{ $d.FileName }}");
            vec!["powershell".into(), "-NoProfile".into(), "-STA".into(), "-Command".into(), script]
        }
    };
    Some(line)
}

/// What a picker's run says: its exit code, what it printed and its errors.
pub fn outcome(success: bool, code: Option<i32>, stdout: &str, stderr: &str) -> Picked {
    let chosen = stdout.lines().map(str::trim).find(|l| !l.is_empty());
    match (success, chosen) {
        (true, Some(path)) => Picked::Path(PathBuf::from(path)),
        // A cancel: zenity and kdialog exit 1, osascript says error -128, PowerShell prints nothing.
        (true, None) => Picked::Cancelled,
        (false, _) if code == Some(1) || stderr.contains("-128") => Picked::Cancelled,
        (false, _) => Picked::Unavailable(format!("the file picker failed: {}", stderr.trim().lines().last().unwrap_or("no reason given"))),
    }
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|dirs| std::env::split_paths(&dirs).any(|dir| dir.join(program).is_file()))
}

/// Opens the picker and waits for it. `start` is where it opens.
pub fn pick(start: &Path) -> Picked {
    let os = Os::this();
    if os == Os::Linux && ["DISPLAY", "WAYLAND_DISPLAY"].iter().all(|v| std::env::var_os(v).is_none_or(|x| x.is_empty())) {
        return Picked::Unavailable("no graphical session here (no DISPLAY or WAYLAND_DISPLAY), so no file picker".to_string());
    }
    let Some(line) = command_line(os, on_path, start) else {
        return Picked::Unavailable("no file picker found: install zenity or kdialog".to_string());
    };
    match std::process::Command::new(&line[0]).args(&line[1..]).stdin(std::process::Stdio::null()).output() {
        Ok(out) => outcome(out.status.success(), out.status.code(), &String::from_utf8_lossy(&out.stdout), &String::from_utf8_lossy(&out.stderr)),
        Err(e) => Picked::Unavailable(format!("{}: {e}", line[0])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CON-173: the first picker found, with a .zip filter, opening where it is told.
    #[test]
    fn the_picker_is_the_first_found_with_its_form() {
        let start = Path::new("/p/multitool_dumps");
        assert_eq!(command_line(Os::Linux, |p| p == "zenity" || p == "kdialog", start).unwrap(), ["zenity", "--file-selection", "--title=Load a conversation", "--file-filter=Chat archives | *.zip", "--filename=/p/multitool_dumps/"]);
        assert_eq!(command_line(Os::Linux, |p| p == "kdialog", start).unwrap(), ["kdialog", "--title", "Load a conversation", "--getopenfilename", "/p/multitool_dumps", "*.zip|Chat archives"]);
        assert_eq!(command_line(Os::Linux, |_| false, start), None);
        let mac = command_line(Os::Mac, |_| false, Path::new("/p/a \"b\"")).unwrap();
        assert_eq!(mac[..2], ["osascript", "-e"]);
        assert!(mac[2].contains("of type {\"zip\"}") && mac[2].contains("POSIX file \"/p/a \\\"b\\\"\""), "{}", mac[2]);
        let win = command_line(Os::Windows, |_| false, Path::new("C:\\p\\it's")).unwrap();
        assert_eq!(win[..4], ["powershell", "-NoProfile", "-STA", "-Command"]);
        assert!(win[4].contains("InitialDirectory = 'C:\\p\\it''s'") && win[4].contains("*.zip"), "{}", win[4]);
    }

    /// A path chosen, a cancel of each kind, a failure.
    #[test]
    fn a_pick_is_a_path_a_cancel_or_a_failure() {
        assert_eq!(outcome(true, Some(0), "/p/a.zip\n", ""), Picked::Path(PathBuf::from("/p/a.zip")));
        assert_eq!(outcome(false, Some(1), "", ""), Picked::Cancelled, "zenity and kdialog");
        assert_eq!(outcome(false, Some(1), "", "execution error: User canceled. (-128)"), Picked::Cancelled, "osascript");
        assert_eq!(outcome(true, Some(0), "\n", ""), Picked::Cancelled, "PowerShell prints nothing");
        assert!(matches!(outcome(false, Some(255), "", "Gtk-WARNING: cannot open display\n"), Picked::Unavailable(why) if why.contains("cannot open display")));
    }
}
