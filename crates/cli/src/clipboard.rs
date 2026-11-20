//! Copying the window's selection to the system's clipboard (CON-188): the clipboard program of
//! the session (wl-copy, xclip, xsel, pbcopy, clip.exe), and OSC 52 for the terminals that take
//! it, over SSH too.

use std::io::Write;
use std::process::{Command, Stdio};

/// The program that puts text on this session's clipboard, by the environment and the OS:
/// (program, arguments). None when none applies or none is installed.
pub fn program(env: &dyn Fn(&str) -> Option<String>, os: &str, on_path: &dyn Fn(&str) -> bool) -> Option<(&'static str, Vec<&'static str>)> {
    let set = |name: &str| env(name).is_some_and(|v| !v.is_empty());
    let candidates: Vec<(&'static str, Vec<&'static str>)> = match os {
        "macos" => vec![("pbcopy", vec![])],
        "windows" => vec![("clip.exe", vec![]), ("clip", vec![])],
        _ => {
            let mut c = Vec::new();
            if set("WAYLAND_DISPLAY") {
                c.push(("wl-copy", vec![]));
            }
            if set("DISPLAY") {
                c.push(("xclip", vec!["-selection", "clipboard"]));
                c.push(("xsel", vec!["--clipboard", "--input"]));
            }
            c
        }
    };
    candidates.into_iter().find(|(p, _)| on_path(p))
}

/// OSC 52: the terminal puts the text on the clipboard, `ESC ] 52 ; c ; <base64> BEL`.
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

fn os() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "unix"
    }
}

/// Copies the text: to the session's clipboard program when there is one, and as OSC 52 in
/// every case. Answers what the toast says.
pub fn copy(text: &str) -> String {
    let chars = text.chars().count();
    // OSC 52 goes straight to the terminal; a terminal that does not take it ignores it.
    let mut out = std::io::stdout();
    let _ = out.write_all(osc52(text).as_bytes());
    let _ = out.flush();
    let Some((program, args)) = program(&|k| std::env::var(k).ok(), os(), &on_path) else {
        return format!("Copied {chars} characters through the terminal (OSC 52): no clipboard program was found.");
    };
    let spawned = Command::new(program).args(&args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    match spawned {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            // wl-copy and xclip stay to serve the clipboard: reaped off the window's thread.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            format!("Copied {chars} characters.")
        }
        Err(e) => format!("Copied {chars} characters through the terminal (OSC 52); {program} failed: {e}."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn the_program_follows_the_session() {
        let all = |_: &str| true;
        assert_eq!(program(&env_of(&[("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0")]), "unix", &all).unwrap().0, "wl-copy");
        assert_eq!(program(&env_of(&[("DISPLAY", ":0")]), "unix", &all).unwrap(), ("xclip", vec!["-selection", "clipboard"]));
        assert_eq!(program(&env_of(&[("DISPLAY", ":0")]), "unix", &|p: &str| p == "xsel").unwrap(), ("xsel", vec!["--clipboard", "--input"]), "xsel when xclip is missing");
        assert_eq!(program(&env_of(&[]), "macos", &all).unwrap().0, "pbcopy");
        assert_eq!(program(&env_of(&[]), "windows", &all).unwrap().0, "clip.exe");
        assert!(program(&env_of(&[]), "unix", &all).is_none(), "no display: none");
        assert!(program(&env_of(&[("WAYLAND_DISPLAY", "wayland-0")]), "unix", &|_: &str| false).is_none(), "not installed: none");
    }

    #[test]
    fn osc52_carries_the_text_in_base64() {
        assert_eq!(osc52("hello"), "\x1b]52;c;aGVsbG8=\x07");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
        assert_eq!(base64("привет".as_bytes()), "0L/RgNC40LLQtdGC");
    }
}
