//! Typed wrappers over the herdr CLI calls this plugin makes.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::runner::Runner;

pub const PLUGIN_ID: &str = "danjuls.ddev";
pub const SOURCE: &str = "danjuls.ddev";
pub const TOKEN: &str = "ddev";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Pane {
    pub pane_id: String,
    pub workspace_id: String,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub foreground_cwd: Option<String>,
}

impl Pane {
    /// The folder that best describes the pane: the foreground process's, else the pane's.
    pub fn dir(&self) -> Option<&str> {
        [&self.foreground_cwd, &self.cwd]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .find(|dir| !dir.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Workspace {
    pub workspace_id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub focused: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    None,
    Done,
    Request,
}

impl Sound {
    fn as_arg(self) -> &'static str {
        match self {
            Sound::None => "none",
            Sound::Done => "done",
            Sound::Request => "request",
        }
    }
}

#[derive(Deserialize)]
struct Envelope<T> {
    result: T,
}

#[derive(Deserialize)]
struct PaneList {
    panes: Vec<Pane>,
}

#[derive(Deserialize)]
struct PaneInfo {
    pane: Pane,
}

#[derive(Deserialize)]
struct WorkspaceList {
    workspaces: Vec<Workspace>,
}

#[derive(Deserialize)]
struct PluginList {
    plugins: Vec<PluginEntry>,
}

#[derive(Deserialize)]
struct PluginEntry {
    plugin_id: String,
    #[serde(default)]
    enabled: bool,
}

/// Whether `herdr plugin list --json` shows this plugin installed and enabled.
pub fn parse_plugin_enabled(json: &str) -> Result<bool> {
    let envelope: Envelope<PluginList> =
        serde_json::from_str(json).context("unexpected `herdr plugin list` output")?;
    Ok(envelope
        .result
        .plugins
        .iter()
        .any(|p| p.plugin_id == PLUGIN_ID && p.enabled))
}

pub fn parse_pane_list(json: &str) -> Result<Vec<Pane>> {
    let envelope: Envelope<PaneList> =
        serde_json::from_str(json).context("unexpected `herdr pane list` output")?;
    Ok(envelope.result.panes)
}

pub fn parse_pane(json: &str) -> Result<Pane> {
    let envelope: Envelope<PaneInfo> =
        serde_json::from_str(json).context("unexpected `herdr pane` output")?;
    Ok(envelope.result.pane)
}

pub fn parse_workspace_list(json: &str) -> Result<Vec<Workspace>> {
    let envelope: Envelope<WorkspaceList> =
        serde_json::from_str(json).context("unexpected `herdr workspace list` output")?;
    Ok(envelope.result.workspaces)
}

pub struct Herdr<'a> {
    runner: &'a dyn Runner,
    bin: String,
}

impl<'a> Herdr<'a> {
    pub fn new(runner: &'a dyn Runner, bin: &str) -> Herdr<'a> {
        Herdr {
            runner,
            bin: bin.to_string(),
        }
    }

    fn argv(&self, args: &[&str]) -> Vec<String> {
        std::iter::once(self.bin.clone())
            .chain(args.iter().map(|a| a.to_string()))
            .collect()
    }

    fn call(&self, args: &[&str]) -> Result<String> {
        let out = self.runner.run(&self.argv(args), None)?;
        if !out.success {
            bail!("herdr {} failed: {}", args.join(" "), out.stderr.trim());
        }
        Ok(out.stdout)
    }

    /// Lists every plugin: a filtered list would fail outright once this one is uninstalled.
    pub fn plugin_enabled(&self) -> Result<bool> {
        parse_plugin_enabled(&self.call(&["plugin", "list", "--json"])?)
    }

    pub fn panes(&self) -> Result<Vec<Pane>> {
        parse_pane_list(&self.call(&["pane", "list"])?)
    }

    pub fn pane(&self, pane_id: &str) -> Result<Pane> {
        parse_pane(&self.call(&["pane", "get", pane_id])?)
    }

    pub fn current_pane(&self) -> Result<Pane> {
        parse_pane(&self.call(&["pane", "current"])?)
    }

    pub fn workspaces(&self) -> Result<Vec<Workspace>> {
        parse_workspace_list(&self.call(&["workspace", "list"])?)
    }

    pub fn set_badge(&self, workspace_id: &str, value: &str, seq: u64, ttl_ms: u64) -> Result<()> {
        let token = format!("{TOKEN}={value}");
        let (seq, ttl) = (seq.to_string(), ttl_ms.to_string());
        let args = [
            "workspace",
            "report-metadata",
            workspace_id,
            "--source",
            SOURCE,
            "--token",
            &token,
            "--seq",
            &seq,
            "--ttl-ms",
            &ttl,
        ];
        self.call(&args).map(drop)
    }

    pub fn clear_badge(&self, workspace_id: &str, seq: u64) -> Result<()> {
        let seq = seq.to_string();
        let args = [
            "workspace",
            "report-metadata",
            workspace_id,
            "--source",
            SOURCE,
            "--clear-token",
            TOKEN,
            "--seq",
            &seq,
        ];
        self.call(&args).map(drop)
    }

    pub fn notify(&self, body: &str, sound: Sound) -> Result<()> {
        let args = [
            "notification",
            "show",
            "ddev",
            "--body",
            body,
            "--sound",
            sound.as_arg(),
        ];
        self.call(&args).map(drop)
    }

    pub fn focus_workspace(&self, workspace_id: &str) -> Result<()> {
        self.call(&["workspace", "focus", workspace_id]).map(drop)
    }

    pub fn create_workspace(&self, cwd: &Path, label: &str) -> Result<()> {
        let cwd = cwd.to_string_lossy();
        self.call(&[
            "workspace",
            "create",
            "--cwd",
            &cwd,
            "--label",
            label,
            "--focus",
        ])
        .map(drop)
    }

    pub fn open_pane(&self, entrypoint: &str, env: &[(&str, &str)]) -> Result<()> {
        let pairs: Vec<String> = env
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        let mut args = vec![
            "plugin",
            "pane",
            "open",
            "--plugin",
            PLUGIN_ID,
            "--entrypoint",
        ];
        args.push(entrypoint);
        for pair in &pairs {
            args.push("--env");
            args.push(pair);
        }
        self.call(&args).map(drop)
    }

    /// Whether `herdr config check` accepts the current config file.
    pub fn config_check(&self) -> Result<bool> {
        Ok(self
            .runner
            .run(&self.argv(&["config", "check"]), None)?
            .success)
    }

    pub fn reload_config(&self) -> Result<()> {
        self.call(&["server", "reload-config"]).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::Output;
    use crate::runner::fake::FakeRunner;

    const PANE_LIST: &str = include_str!("../tests/fixtures/pane-list.json");
    const PANE_GET: &str = include_str!("../tests/fixtures/pane-get.json");
    const WORKSPACE_LIST: &str = include_str!("../tests/fixtures/workspace-list.json");

    fn herdr_ok() -> FakeRunner {
        let fake = FakeRunner::new();
        fake.on(&["herdr"], Output::ok("{}"));
        fake
    }

    #[test]
    fn parses_pane_list_ignoring_extra_fields() {
        let panes = parse_pane_list(PANE_LIST).unwrap();
        assert_eq!(panes.len(), 3);
        assert_eq!(panes[0].workspace_id, "w1");
        assert!(panes[0].focused);
        assert_eq!(panes[0].dir(), Some("/home/dev/sites/shop/web"));
        assert_eq!(panes[1].dir(), Some("/home/dev/sites/shop"));
    }

    #[test]
    fn parses_pane_get() {
        assert_eq!(parse_pane(PANE_GET).unwrap().pane_id, "w1:p1");
    }

    #[test]
    fn parses_workspace_list() {
        let workspaces = parse_workspace_list(WORKSPACE_LIST).unwrap();
        assert_eq!(workspaces[1].label, "notes");
        assert!(workspaces[0].focused);
    }

    #[test]
    fn empty_dirs_are_ignored() {
        let pane = Pane {
            pane_id: "p".into(),
            workspace_id: "w".into(),
            focused: false,
            cwd: Some(String::new()),
            foreground_cwd: None,
        };
        assert_eq!(pane.dir(), None);
    }

    #[test]
    fn garbage_output_is_an_error() {
        assert!(parse_pane_list("not json").is_err());
    }

    #[test]
    fn set_badge_sends_token_seq_and_ttl() {
        let fake = herdr_ok();
        Herdr::new(&fake, "herdr")
            .set_badge("w1", "● ddev", 1_700_000_000_000, 20_000)
            .unwrap();
        assert_eq!(
            fake.calls()[0].argv,
            [
                "herdr",
                "workspace",
                "report-metadata",
                "w1",
                "--source",
                "danjuls.ddev",
                "--token",
                "ddev=● ddev",
                "--seq",
                "1700000000000",
                "--ttl-ms",
                "20000",
            ]
        );
    }

    #[test]
    fn clear_badge_uses_clear_token() {
        let fake = herdr_ok();
        Herdr::new(&fake, "herdr").clear_badge("w2", 5).unwrap();
        assert_eq!(
            fake.calls()[0].argv,
            [
                "herdr",
                "workspace",
                "report-metadata",
                "w2",
                "--source",
                "danjuls.ddev",
                "--clear-token",
                "ddev",
                "--seq",
                "5",
            ]
        );
    }

    #[test]
    fn notify_passes_title_body_and_sound() {
        let fake = herdr_ok();
        Herdr::new(&fake, "herdr")
            .notify("shop started", Sound::Done)
            .unwrap();
        assert_eq!(
            fake.calls()[0].argv,
            [
                "herdr",
                "notification",
                "show",
                "ddev",
                "--body",
                "shop started",
                "--sound",
                "done"
            ]
        );
    }

    #[test]
    fn open_pane_passes_env() {
        let fake = herdr_ok();
        let herdr = Herdr::new(&fake, "herdr");
        herdr
            .open_pane("url", &[("HERDR_DDEV_URL", "https://shop.ddev.site")])
            .unwrap();
        assert_eq!(
            fake.calls()[0].argv,
            [
                "herdr",
                "plugin",
                "pane",
                "open",
                "--plugin",
                "danjuls.ddev",
                "--entrypoint",
                "url",
                "--env",
                "HERDR_DDEV_URL=https://shop.ddev.site",
            ]
        );
    }

    #[test]
    fn create_workspace_focuses_it() {
        let fake = herdr_ok();
        Herdr::new(&fake, "herdr")
            .create_workspace(Path::new("/w/My Shop"), "shop")
            .unwrap();
        assert_eq!(
            fake.calls()[0].argv,
            [
                "herdr",
                "workspace",
                "create",
                "--cwd",
                "/w/My Shop",
                "--label",
                "shop",
                "--focus"
            ]
        );
    }

    #[test]
    fn failed_call_is_an_error_with_stderr() {
        let fake = FakeRunner::new();
        fake.on(&["herdr"], Output::fail("no server running"));
        let err = Herdr::new(&fake, "herdr").panes().unwrap_err();
        assert!(err.to_string().contains("no server running"));
    }

    #[test]
    fn config_check_reports_failure_as_false() {
        let fake = FakeRunner::new();
        fake.on(
            &["herdr", "config", "check"],
            Output::fail("config: issues found"),
        );
        assert!(!Herdr::new(&fake, "herdr").config_check().unwrap());
    }

    #[test]
    fn plugin_enabled_reads_this_plugin_only() {
        let both = r#"{"result":{"plugins":[
            {"plugin_id":"persiyanov.reviewr","enabled":true},
            {"plugin_id":"danjuls.ddev","enabled":false}]}}"#;
        assert!(!parse_plugin_enabled(both).unwrap());
        let on = r#"{"result":{"plugins":[{"plugin_id":"danjuls.ddev","enabled":true}]}}"#;
        assert!(parse_plugin_enabled(on).unwrap());
        let gone = r#"{"result":{"plugins":[{"plugin_id":"persiyanov.reviewr","enabled":true}]}}"#;
        assert!(!parse_plugin_enabled(gone).unwrap());
    }
}
