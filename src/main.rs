use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use herdr_ddev::actions::{self, Kind};
use herdr_ddev::app::App;
use herdr_ddev::busy::Verb;
use herdr_ddev::{configure, open, picker, ticker};

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
    match args.as_slice() {
        ["--version" | "-V"] => {
            println!("herdr-ddev {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        ["url"] => {
            return open::run_url_popup(&std::env::var("HERDR_DDEV_URL").unwrap_or_default());
        }
        _ => {}
    }
    let app = App::from_env()?;
    let spawn = |spawn_args: &[String]| app.spawn(spawn_args);
    let ctx = app.ctx(&spawn);
    match args.as_slice() {
        ["ticker"] => ticker::run(&ctx, app.config_error.as_deref()),
        ["ticker", "--detach"] => ticker::ensure_running(&ctx),
        ["action", kind] => {
            let kind = Kind::parse(kind).with_context(|| format!("unknown action: {kind}"))?;
            actions::run(kind, &ctx)
        }
        ["worker", verb, root, name] => {
            let verb = Verb::parse(verb).with_context(|| format!("unknown verb: {verb}"))?;
            actions::worker(&ctx, verb, Path::new(root), name)
        }
        ["picker"] => picker::run(&ctx),
        ["configure"] => configure::run_configure(&ctx),
        ["unconfigure"] => configure::run_unconfigure(&ctx),
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
