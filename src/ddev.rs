//! ddev commands: start/stop/restart, the site URL, and the project list for the picker.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::busy::Verb;
use crate::runner::{Output, Runner};

fn argv(ddev: &[String], args: &[&str]) -> Vec<String> {
    ddev.iter()
        .cloned()
        .chain(args.iter().map(|a| a.to_string()))
        .collect()
}

/// Run `ddev start|stop|restart` in the project root, with no time limit (a first start pulls
/// images). `Err` only when ddev could not run.
pub fn run_verb(runner: &dyn Runner, ddev: &[String], root: &Path, verb: Verb) -> Result<Output> {
    runner.run_long(&argv(ddev, &[verb.as_str()]), Some(root))
}

/// What to show when ddev fails: the first non-empty stderr line, else stdout's, at most
/// 200 characters.
pub fn first_error_line(output: &Output) -> String {
    output
        .stderr
        .lines()
        .chain(output.stdout.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("ddev failed without output")
        .chars()
        .take(200)
        .collect()
}

/// ddev `-j` output: the whole text, else the last line that parses (ddev can print warning
/// objects on their own lines first).
fn parse_json<T: DeserializeOwned>(text: &str) -> Option<T> {
    serde_json::from_str(text.trim()).ok().or_else(|| {
        text.lines()
            .rev()
            .find_map(|line| serde_json::from_str(line.trim()).ok())
    })
}

#[derive(Deserialize)]
struct Describe {
    raw: DescribeRaw,
}

#[derive(Deserialize)]
struct DescribeRaw {
    #[serde(default)]
    primary_url: String,
    #[serde(default)]
    httpsurl: String,
    #[serde(default)]
    httpurl: String,
}

pub fn parse_site_url(text: &str) -> Result<String> {
    let describe: Describe = parse_json(text).context("unexpected `ddev describe -j` output")?;
    let raw = describe.raw;
    [raw.primary_url, raw.httpsurl, raw.httpurl]
        .into_iter()
        .find(|url| !url.is_empty())
        .context("ddev describe returned no URL")
}

/// The project's primary URL, from `ddev describe -j` in its root.
pub fn site_url(runner: &dyn Runner, ddev: &[String], root: &Path) -> Result<String> {
    let out = runner.run(&argv(ddev, &["describe", "-j"]), Some(root))?;
    if !out.success {
        bail!("{}", first_error_line(&out));
    }
    parse_site_url(&out.stdout)
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Listed {
    pub name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub approot: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub primary_url: String,
}

#[derive(Deserialize)]
struct ListOutput {
    raw: Vec<Listed>,
}

pub fn parse_list(text: &str) -> Result<Vec<Listed>> {
    let list: ListOutput = parse_json(text).context("unexpected `ddev list -j` output")?;
    Ok(list.raw)
}

/// Every project ddev knows about, with type and URL.
pub fn list(runner: &dyn Runner, ddev: &[String]) -> Result<Vec<Listed>> {
    let out = runner.run(&argv(ddev, &["list", "-j"]), None)?;
    if !out.success {
        bail!("{}", first_error_line(&out));
    }
    parse_list(&out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::FakeRunner;

    const DESCRIBE: &str = concat!(
        r#"{"level":"info","msg":"shop","raw":{"name":"shop","status":"running","#,
        r#""primary_url":"https://shop.ddev.site","httpsurl":"https://shop.ddev.site","#,
        r#""httpurl":"http://shop.ddev.site","approot":"/home/dev/sites/shop","#,
        r#""type":"drupal11"},"time":"2026-09-27T10:00:00+02:00"}"#
    );

    const LIST: &str = concat!(
        r#"{"level":"info","msg":"","raw":["#,
        r#"{"name":"shop","status":"running","approot":"/home/dev/sites/shop","#,
        r#""type":"drupal11","primary_url":"https://shop.ddev.site"},"#,
        r#"{"name":"blog","status":"stopped","approot":"/home/dev/blog","type":"wordpress"}"#,
        r#"]}"#
    );

    fn ddev() -> Vec<String> {
        vec!["ddev".to_string()]
    }

    #[test]
    fn site_url_prefers_primary_url() {
        assert_eq!(parse_site_url(DESCRIBE).unwrap(), "https://shop.ddev.site");
    }

    #[test]
    fn site_url_falls_back_to_http_urls() {
        let json = r#"{"raw":{"httpurl":"http://x.ddev.site"}}"#;
        assert_eq!(parse_site_url(json).unwrap(), "http://x.ddev.site");
    }

    #[test]
    fn warning_lines_before_the_result_are_skipped() {
        let text = format!(
            "{}\n{DESCRIBE}\n",
            r#"{"level":"warning","msg":"Docker is old"}"#
        );
        assert_eq!(parse_site_url(&text).unwrap(), "https://shop.ddev.site");
    }

    #[test]
    fn missing_url_is_an_error() {
        assert!(parse_site_url(r#"{"raw":{"name":"x"}}"#).is_err());
    }

    #[test]
    fn parses_the_project_list() {
        let projects = parse_list(LIST).unwrap();
        assert_eq!(projects.len(), 2);
        assert_eq!(projects[0].kind, "drupal11");
        assert_eq!(projects[1].status, "stopped");
        assert_eq!(projects[1].primary_url, "");
    }

    #[test]
    fn run_verb_runs_in_the_project_root() {
        let fake = FakeRunner::new();
        fake.on(&["ddev", "start"], Output::ok(""));
        let out = run_verb(&fake, &ddev(), Path::new("/w/shop"), Verb::Start).unwrap();
        assert!(out.success);
        let call = &fake.calls()[0];
        assert!(
            call.long,
            "ddev start can take minutes; it must not have a time limit"
        );
        assert_eq!(call.argv, ["ddev", "start"]);
        assert_eq!(call.cwd.as_deref(), Some(Path::new("/w/shop")));
    }

    #[test]
    fn wrapped_ddev_keeps_the_wrapper() {
        let fake = FakeRunner::new();
        fake.on(&["distrobox"], Output::ok(""));
        let wrapper: Vec<String> = ["distrobox", "enter", "--", "ddev"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        run_verb(&fake, &wrapper, Path::new("/w/shop"), Verb::Stop).unwrap();
        assert_eq!(
            fake.calls()[0].argv,
            ["distrobox", "enter", "--", "ddev", "stop"]
        );
    }

    #[test]
    fn first_error_line_prefers_stderr_and_caps_length() {
        let out = Output {
            success: false,
            stdout: "out\n".into(),
            stderr: "\n  Failed to start shop: port 443 in use\nmore".into(),
        };
        assert_eq!(
            first_error_line(&out),
            "Failed to start shop: port 443 in use"
        );
        assert_eq!(first_error_line(&Output::fail(&"x".repeat(500))).len(), 200);
        assert_eq!(
            first_error_line(&Output::fail("")),
            "ddev failed without output"
        );
    }

    #[test]
    fn site_url_reports_ddev_failure() {
        let fake = FakeRunner::new();
        fake.on(&["ddev", "describe"], Output::fail("not a ddev project"));
        let err = site_url(&fake, &ddev(), Path::new("/w")).unwrap_err();
        assert!(err.to_string().contains("not a ddev project"));
    }
}
