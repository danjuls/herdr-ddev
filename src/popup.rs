//! Popups close as soon as their command exits, so errors must be shown before that.

use std::io::{BufRead, Write};

/// Show a popup's error and wait for Enter, so it is readable before the popup closes.
pub fn report_error(
    err: &anyhow::Error,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
) -> std::io::Result<()> {
    writeln!(output, "\nherdr-ddev: {err:#}")?;
    write!(output, "\nPress enter to close.")?;
    output.flush()?;
    input.read_line(&mut String::new())?;
    Ok(())
}

/// Run a popup's body; on failure show the error before the popup disappears.
pub fn run(body: impl FnOnce() -> anyhow::Result<()>) -> anyhow::Result<()> {
    let result = body();
    if let Err(err) = &result {
        let _ = report_error(err, &mut std::io::stdin().lock(), &mut std::io::stdout());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_is_shown_and_waits_for_enter() {
        let err = anyhow::anyhow!("config.toml is not valid TOML");
        let mut input = std::io::Cursor::new(b"\nleft over".to_vec());
        let mut output = Vec::new();
        report_error(&err, &mut input, &mut output).unwrap();
        let shown = String::from_utf8(output).unwrap();
        assert!(shown.contains("config.toml is not valid TOML"), "{shown}");
        assert!(shown.contains("Press enter to close."));
        assert_eq!(input.position(), 1, "must read exactly one line");
    }

    #[test]
    fn closed_input_does_not_hang() {
        let err = anyhow::anyhow!("boom");
        let mut input = std::io::Cursor::new(Vec::new());
        let mut output = Vec::new();
        report_error(&err, &mut input, &mut output).unwrap();
    }
}
