use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("herdr-ddev: {err:#}");
            ExitCode::FAILURE
        }
    }
}

use anyhow::{Result, bail};

const USAGE: &str = concat!(
    "usage: herdr-ddev <ticker [--detach] | action <id> | worker <verb> <root> <name> | ",
    "picker | url | configure | unconfigure | --version>"
);

fn run(args: &[String]) -> Result<()> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["--version" | "-V"] => {
            println!("herdr-ddev {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => bail!("{USAGE}"),
    }
}

#[cfg(test)]
mod tests {
    use super::run;

    #[test]
    fn version_flag_succeeds() {
        assert!(run(&["--version".to_string()]).is_ok());
    }

    #[test]
    fn unknown_command_fails_with_usage() {
        let err = run(&["nope".to_string()]).unwrap_err();
        assert!(err.to_string().starts_with("usage: herdr-ddev"));
    }
}
