//! Opening a site: a browser when there is a local desktop, the clipboard otherwise.

use std::io::Write;

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal;

use crate::config::OpenMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opener {
    /// The program that opens a URL in the default browser.
    Browser(&'static str),
    Clipboard,
}

/// `auto` picks the browser only with a local desktop (macOS, or Linux with a display) and no
/// SSH session. Empty variables count as unset.
pub fn decide(mode: OpenMode, os: &str, env: &dyn Fn(&str) -> Option<String>) -> Opener {
    let browser = if os == "macos" { "open" } else { "xdg-open" };
    let set = |key: &str| env(key).is_some_and(|value| !value.is_empty());
    match mode {
        OpenMode::Browser => Opener::Browser(browser),
        OpenMode::Clipboard => Opener::Clipboard,
        OpenMode::Auto => {
            let desktop = os == "macos" || set("DISPLAY") || set("WAYLAND_DISPLAY");
            if desktop && !set("SSH_CONNECTION") {
                Opener::Browser(browser)
            } else {
                Opener::Clipboard
            }
        }
    }
}

pub fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The OSC 52 escape that asks the outer terminal to put `text` on the clipboard.
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

/// The `url` popup: copy the URL with OSC 52, show it so it can be drag-selected if the copy
/// did not reach the terminal, and close on any key.
pub fn run_url_popup(url: &str) -> Result<()> {
    let mut stdout = std::io::stdout();
    write!(stdout, "{}", osc52(url))?;
    writeln!(
        stdout,
        "\n\n  {url}\n\n  Copied - or drag to select it. Press any key to close."
    )?;
    stdout.flush()?;
    terminal::enable_raw_mode()?;
    let result = loop {
        match event::read() {
            Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => break Ok(()),
            Ok(_) => continue,
            Err(err) => break Err(err.into()),
        }
    };
    terminal::disable_raw_mode()?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn explicit_modes_win() {
        let none = env_with(&[]);
        assert_eq!(
            decide(OpenMode::Browser, "linux", &none),
            Opener::Browser("xdg-open")
        );
        assert_eq!(
            decide(OpenMode::Clipboard, "macos", &none),
            Opener::Clipboard
        );
    }

    #[test]
    fn auto_uses_the_browser_on_a_local_mac() {
        assert_eq!(
            decide(OpenMode::Auto, "macos", &env_with(&[])),
            Opener::Browser("open")
        );
    }

    #[test]
    fn auto_uses_the_clipboard_over_ssh() {
        let ssh = env_with(&[("SSH_CONNECTION", "10.0.0.2 51000 10.0.0.1 22")]);
        assert_eq!(decide(OpenMode::Auto, "macos", &ssh), Opener::Clipboard);
    }

    #[test]
    fn auto_on_linux_needs_a_display() {
        let x11 = env_with(&[("DISPLAY", ":0")]);
        let wayland = env_with(&[("WAYLAND_DISPLAY", "wayland-0")]);
        assert_eq!(
            decide(OpenMode::Auto, "linux", &x11),
            Opener::Browser("xdg-open")
        );
        assert_eq!(
            decide(OpenMode::Auto, "linux", &wayland),
            Opener::Browser("xdg-open")
        );
        assert_eq!(
            decide(OpenMode::Auto, "linux", &env_with(&[])),
            Opener::Clipboard
        );
    }

    #[test]
    fn empty_variables_count_as_unset() {
        let empty_ssh = env_with(&[("SSH_CONNECTION", "")]);
        assert_eq!(
            decide(OpenMode::Auto, "macos", &empty_ssh),
            Opener::Browser("open")
        );
    }

    #[test]
    fn base64_matches_rfc4648_vectors() {
        let cases = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foobar", "Zm9vYmFy"),
        ];
        for (input, expected) in cases {
            assert_eq!(base64(input.as_bytes()), expected);
        }
    }

    #[test]
    fn osc52_wraps_base64_in_the_clipboard_escape() {
        assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07");
    }
}
