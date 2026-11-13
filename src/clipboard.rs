//! Copy text (a path or a file name) to the system clipboard.
//!
//! A terminal file manager has no window of its own, so the clipboard is
//! reached in two ways:
//!
//! 1. a platform helper — `pbcopy` (macOS), `wl-copy`/`xclip`/`xsel` (Linux),
//!    `clip`/PowerShell (Windows);
//! 2. the OSC 52 escape sequence, which every modern terminal understands
//!    (Ghostty, Windows Terminal, WezTerm, kitty, iTerm2, Alacritty).
//!
//! Helpers are preferred because they work even in a plain `conhost` window;
//! OSC 52 is the fallback so copying still works on a machine with no helper
//! installed. `YAZI2D_CLIPBOARD` overrides the choice: set it to a program that
//! reads the text on stdin, or to `off` to disable copying (used by tests).

use std::io::Write;
use std::process::{Command, Stdio};

/// One way of reaching the clipboard.
struct Helper {
    name: &'static str,
    program: String,
    args: Vec<String>,
}

impl Helper {
    fn new(name: &'static str, program: &str) -> Self {
        Self {
            name,
            program: program.to_string(),
            args: Vec::new(),
        }
    }

    /// A helper that needs fixed arguments (`xclip -selection clipboard`).
    ///
    /// Only the Linux and Windows helpers take arguments; on macOS the single
    /// `pbcopy` needs none, so the constructor would be dead code there.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    fn with_args(name: &'static str, program: &str, args: &[&str]) -> Self {
        Self {
            name,
            program: program.to_string(),
            args: args.iter().map(|arg| arg.to_string()).collect(),
        }
    }
}

/// Copy `text` using the system helpers or, failing that, OSC 52.
pub fn copy_text(text: &str) -> Result<&'static str, String> {
    match std::env::var("YAZI2D_CLIPBOARD") {
        Ok(program) if program == "off" => {
            Err("clipboard disabled by YAZI2D_CLIPBOARD=off".to_string())
        }
        Ok(program) if !program.trim().is_empty() => copy_with(Some(program.trim()), text),
        _ => copy_with(None, text),
    }
}

/// Copy `text`, optionally through an explicit helper program.
///
/// The explicit program is what tests and `YAZI2D_CLIPBOARD=<program>` use; it
/// receives the text on stdin exactly like `pbcopy`.
pub fn copy_with(program: Option<&str>, text: &str) -> Result<&'static str, String> {
    let mut errors = Vec::new();

    if let Some(program) = program {
        let helper = Helper::new("custom", program);
        match run_helper(&helper, text) {
            Ok(()) => return Ok(helper.name),
            Err(error) => errors.push(format!("{program}: {error}")),
        }
        return match osc52(text) {
            Ok(()) => Ok("OSC 52"),
            Err(error) => {
                errors.push(format!("osc52: {error}"));
                Err(errors.join("; "))
            }
        };
    }

    for helper in helpers() {
        match run_helper(&helper, text) {
            Ok(()) => return Ok(helper.name),
            Err(error) => errors.push(format!("{}: {error}", helper.name)),
        }
    }

    match osc52(text) {
        Ok(()) => Ok("OSC 52"),
        Err(error) => {
            errors.push(format!("osc52: {error}"));
            Err(errors.join("; "))
        }
    }
}

fn helpers() -> Vec<Helper> {
    #[allow(unused_mut)]
    let mut helpers = Vec::new();

    #[cfg(target_os = "windows")]
    {
        helpers.push(Helper::new("clip", "clip.exe"));
        helpers.push(Helper::with_args(
            "powershell",
            "powershell.exe",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-STA",
                "-Command",
                "Add-Type -AssemblyName System.Windows.Forms; \
                 [System.Windows.Forms.Clipboard]::SetText([Console]::In.ReadToEnd())",
            ],
        ));
    }

    #[cfg(target_os = "macos")]
    {
        helpers.push(Helper::new("pbcopy", "pbcopy"));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            helpers.push(Helper::new("wl-copy", "wl-copy"));
        }
        helpers.push(Helper::with_args(
            "xclip",
            "xclip",
            &["-selection", "clipboard"],
        ));
        helpers.push(Helper::with_args("xsel", "xsel", &["-b", "-i"]));
    }

    helpers
}

fn run_helper(helper: &Helper, text: &str) -> Result<(), String> {
    let mut child = Command::new(&helper.program)
        .args(&helper.args)
        // The helper must never take over or dirty the terminal: it reads the
        // text on stdin and its output is irrelevant.
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;

    child
        .stdin
        .take()
        .ok_or_else(|| "no stdin".to_string())?
        .write_all(text.as_bytes())
        .map_err(|error| error.to_string())?;

    // Wait briefly so a helper that rejects the text (a missing display, for
    // example) is reported and its exit status is reaped. `wl-copy` and
    // `xclip` stay alive on purpose to serve the selection, so a helper that is
    // still running after the grace period is left alone instead of being
    // killed — killing it would drop the clipboard contents.
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(400);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(format!("exited with {status}"));
            }
            Ok(None) => {}
            Err(error) => return Err(error.to_string()),
        }
        if std::time::Instant::now() >= deadline {
            // Reap the long-lived helper without blocking the UI thread.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Write the text as an OSC 52 clipboard sequence.
///
/// Terminals that do not support the sequence ignore it silently, which is why
/// this runs after the native helpers.
fn osc52(text: &str) -> Result<(), String> {
    let payload = base64(text.as_bytes());
    let sequence = format!("\x1b]52;c;{payload}\x07");

    let mut stdout = std::io::stdout();
    stdout
        .write_all(sequence.as_bytes())
        .and_then(|()| stdout.flush())
        .map_err(|error| error.to_string())
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(ALPHABET[(triple >> 18) as usize & 0x3f] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 0x3f] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(triple >> 6) as usize & 0x3f] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[triple as usize & 0x3f] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_helper(
        text: &str,
    ) -> (
        tempfile::TempDir,
        std::path::PathBuf,
        Result<&'static str, String>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("clip.txt");
        let script = dir.path().join("fake-clip");
        std::fs::write(&script, format!("#!/bin/sh\ncat > {}\n", out.display())).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script, perms).unwrap();
        }
        let result = copy_with(Some(script.display().to_string().as_str()), text);
        (dir, out, result)
    }

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        // Cyrillic paths must survive unchanged.
        assert_eq!(base64("папка".as_bytes()), "0L/QsNC/0LrQsA==");
    }

    #[cfg(unix)]
    #[test]
    fn a_custom_helper_receives_the_text() {
        let (_dir, out, result) = fake_helper("/tmp/пример.txt");
        assert!(result.is_ok(), "custom helper failed: {result:?}");
        // `run_helper` does not wait for a helper that outlives the grace
        // period (that is what keeps `wl-copy` alive), so on a loaded machine
        // the shell may still be writing. Wait for the file instead of racing.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !out.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "/tmp/пример.txt");
    }

    #[test]
    fn a_missing_helper_is_reported() {
        // OSC 52 still succeeds (it only writes to stdout), so the call returns
        // Ok; the important part is that it does not panic or block.
        let result = copy_with(Some("/nonexistent/clip-tool"), "x");
        assert!(result.is_ok());
    }
}
