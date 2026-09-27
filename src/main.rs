use std::process::ExitCode;

use anyhow::{Result, bail};
use herdr_ddev::app::App;
use herdr_ddev::ticker;

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

const USAGE: &str = concat!(
    "usage: herdr-ddev <ticker [--detach] | action <id> | worker <verb> <root> <name> | ",
    "picker | url | configure | unconfigure | --version>"
);

fn run(args: &[String]) -> Result<()> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    if let ["--version" | "-V"] = args.as_slice() {
        println!("herdr-ddev {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let app = App::from_env()?;
    let spawn = |spawn_args: &[String]| app.spawn(spawn_args);
    let ctx = app.ctx(&spawn);
    match args.as_slice() {
        ["ticker"] => ticker::run(&ctx, app.config_error.as_deref()),
        ["ticker", "--detach"] => ticker::ensure_running(&ctx),
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
