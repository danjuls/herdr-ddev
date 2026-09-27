//! What the keys do: act on the project behind the focused pane, or open a popup.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::app::{Ctx, unix_ms};
use crate::busy::{self, Acquire, Marker, Verb};
use crate::docker::{self, Project, State};
use crate::herdr::Sound;
use crate::open::{self, Opener};
use crate::project::{self, StoppedCache};
use crate::{config, ddev, ticker};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Toggle,
    Start,
    Stop,
    Restart,
    Open,
    Picker,
    Configure,
    Unconfigure,
}

impl Kind {
    pub fn parse(text: &str) -> Option<Kind> {
        match text {
            "toggle" => Some(Kind::Toggle),
            "start" => Some(Kind::Start),
            "stop" => Some(Kind::Stop),
            "restart" => Some(Kind::Restart),
            "open" => Some(Kind::Open),
            "picker" => Some(Kind::Picker),
            "configure" => Some(Kind::Configure),
            "unconfigure" => Some(Kind::Unconfigure),
            _ => None,
        }
    }
}

pub fn toggle_verb(state: State) -> Verb {
    if state == State::Running {
        Verb::Stop
    } else {
        Verb::Start
    }
}

/// Arguments for the background worker; argv, so paths with spaces stay whole.
pub fn worker_args(verb: Verb, root: &Path, name: &str) -> Vec<String> {
    vec![
        "worker".to_string(),
        verb.as_str().to_string(),
        root.to_string_lossy().into_owned(),
        name.to_string(),
    ]
}

/// Run an action. Every action first makes sure a ticker runs, because startup hooks do not
/// run on `plugin link` or `plugin enable`.
pub fn run(kind: Kind, ctx: &Ctx) -> Result<()> {
    ticker::ensure_running(ctx)?;
    match kind {
        Kind::Picker => ctx.herdr.open_pane("picker", &[]),
        Kind::Configure => ctx.herdr.open_pane("configure", &[]),
        Kind::Unconfigure => ctx.herdr.open_pane("unconfigure", &[]),
        Kind::Open => open_site(ctx),
        Kind::Toggle | Kind::Start | Kind::Stop | Kind::Restart => change(kind, ctx),
    }
}

fn change(kind: Kind, ctx: &Ctx) -> Result<()> {
    if ctx.ddev.is_none() {
        return notify_missing(ctx, "ddev", "ddev_command");
    }
    if kind == Kind::Toggle && ctx.docker.is_none() {
        return notify_missing(ctx, "docker", "docker_command");
    }
    let Some(project) = current_project(ctx)? else {
        return ctx.herdr.notify("Not in a ddev project", Sound::Request);
    };
    let verb = match kind {
        Kind::Toggle => toggle_verb(project.state),
        Kind::Start => Verb::Start,
        Kind::Stop => Verb::Stop,
        _ => Verb::Restart,
    };
    (ctx.spawn)(&worker_args(verb, &project.root, &project.name))
}

fn notify_missing(ctx: &Ctx, what: &str, key: &str) -> Result<()> {
    let file = ctx.config_dir.join(config::FILE_NAME);
    let body = format!("{what} not found - set {key} in {}", file.display());
    ctx.herdr.notify(&body, Sound::Request)
}

fn current_dir(ctx: &Ctx) -> Result<String> {
    let pane = match &ctx.pane_id {
        Some(id) => ctx.herdr.pane(id)?,
        None => ctx.herdr.current_pane()?,
    };
    pane.dir()
        .map(str::to_string)
        .context("the focused pane has no folder")
}

/// The project behind the focused pane. A Docker failure counts as "nothing running", so
/// toggle falls through to `ddev start`, which reports the real problem.
pub fn current_project(ctx: &Ctx) -> Result<Option<Project>> {
    let dir = current_dir(ctx)?;
    let running = match &ctx.docker {
        Some(docker) => docker::query(ctx.runner, docker)
            .map(project::canonical_projects)
            .unwrap_or_default(),
        None => Vec::new(),
    };
    let mut cache = StoppedCache::new(Duration::ZERO);
    Ok(project::resolve(
        Path::new(&dir),
        &running,
        &mut cache,
        Instant::now(),
    ))
}

fn open_site(ctx: &Ctx) -> Result<()> {
    let Some(ddev_cmd) = &ctx.ddev else {
        return notify_missing(ctx, "ddev", "ddev_command");
    };
    let Some(project) = current_project(ctx)? else {
        return ctx.herdr.notify("Not in a ddev project", Sound::Request);
    };
    match ddev::site_url(ctx.runner, ddev_cmd, &project.root) {
        Ok(url) => open_url(ctx, &url),
        Err(err) => ctx
            .herdr
            .notify(&format!("{}: {err:#}", project.name), Sound::Request),
    }
}

/// Open `url` in the browser, or hand it to the `url` popup for the clipboard.
pub fn open_url(ctx: &Ctx, url: &str) -> Result<()> {
    match open::decide(ctx.open_mode, std::env::consts::OS, &|key| {
        std::env::var(key).ok()
    }) {
        Opener::Browser(program) => {
            let out = ctx
                .runner
                .run(&[program.to_string(), url.to_string()], None)?;
            if out.success {
                Ok(())
            } else {
                ctx.herdr
                    .notify(&format!("could not open {url}"), Sound::Request)
            }
        }
        Opener::Clipboard => ctx.herdr.open_pane("url", &[("HERDR_DDEV_URL", url)]),
    }
}

/// The background half of start/stop/restart: mark the project busy, run ddev, refresh the
/// badges, notify. Full ddev output on failure goes to stderr, which is `worker.log`.
pub fn worker(ctx: &Ctx, verb: Verb, root: &Path, name: &str) -> Result<()> {
    let ddev_cmd = ctx.ddev.clone().context("ddev not found")?;
    let now = unix_ms() / 1000;
    let marker = Marker {
        project: name.to_string(),
        verb,
        pid: std::process::id(),
        started_unix: now,
    };
    if let Acquire::Busy(existing) = busy::acquire(&ctx.state_dir, &marker, now, ctx.pid_alive)? {
        let body = format!("{name} is already {}", existing.verb.progressive());
        return ctx.herdr.notify(&body, Sound::Request);
    }
    refresh_badges(ctx);
    let result = ddev::run_verb(ctx.runner, &ddev_cmd, root, verb);
    busy::release(&ctx.state_dir, name);
    refresh_badges(ctx);
    match result {
        Ok(out) if out.success => ctx
            .herdr
            .notify(&format!("{name} {}", verb.past()), Sound::Done),
        Ok(out) => {
            eprintln!(
                "ddev {} {name} failed:\n{}{}",
                verb.as_str(),
                out.stdout,
                out.stderr
            );
            let body = format!("{name}: {}", ddev::first_error_line(&out));
            ctx.herdr.notify(&body, Sound::Request)
        }
        Err(err) => ctx
            .herdr
            .notify(&format!("{name}: {err:#}"), Sound::Request),
    }
}

fn refresh_badges(ctx: &Ctx) {
    if let Some(docker) = ctx.docker.clone() {
        let _ = ticker::Ticker::new(ctx, docker).tick(Instant::now(), unix_ms());
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::app::testing;
    use crate::config::OpenMode;
    use crate::runner::Output;
    use crate::runner::fake::FakeRunner;

    const DESCRIBE: &str = r#"{"raw":{"primary_url":"https://shop.ddev.site"}}"#;

    /// A canonical temp folder holding `shop/.ddev/config.yaml`, and a plain `notes` folder.
    fn shop() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = crate::project::canonical(tmp.path());
        let (shop, notes) = (root.join("shop"), root.join("notes"));
        fs::create_dir_all(shop.join(".ddev")).unwrap();
        fs::write(shop.join(".ddev/config.yaml"), "name: shop\n").unwrap();
        fs::create_dir_all(&notes).unwrap();
        (tmp, shop, notes)
    }

    fn pane_json(dir: &Path) -> Output {
        Output::ok(&format!(
            r#"{{"result":{{"pane":{{"pane_id":"w1:p1","workspace_id":"w1","focused":true,"cwd":"{}"}},"type":"pane_info"}}}}"#,
            dir.display()
        ))
    }

    fn pane_list_json(dir: &Path) -> Output {
        Output::ok(&format!(
            r#"{{"result":{{"panes":[{{"pane_id":"w1:p1","workspace_id":"w1","focused":true,"cwd":"{}"}}]}}}}"#,
            dir.display()
        ))
    }

    fn notifications(fake: &FakeRunner) -> Vec<(String, String)> {
        fake.calls_starting_with(&["herdr", "notification", "show"])
            .into_iter()
            .map(|call| (call.argv[5].clone(), call.argv[7].clone()))
            .collect()
    }

    fn recorder() -> RefCell<Vec<Vec<String>>> {
        RefCell::new(Vec::new())
    }

    #[test]
    fn kinds_parse() {
        assert_eq!(Kind::parse("toggle"), Some(Kind::Toggle));
        assert_eq!(Kind::parse("unconfigure"), Some(Kind::Unconfigure));
        assert_eq!(Kind::parse("launch"), None);
    }

    #[test]
    fn toggle_stops_running_and_starts_everything_else() {
        assert_eq!(toggle_verb(State::Running), Verb::Stop);
        assert_eq!(toggle_verb(State::Paused), Verb::Start);
        assert_eq!(toggle_verb(State::Stopped), Verb::Start);
    }

    #[test]
    fn worker_args_keep_paths_with_spaces_intact() {
        let args = worker_args(Verb::Start, Path::new("/My Sites/shop"), "shop");
        assert_eq!(args, ["worker", "start", "/My Sites/shop", "shop"]);
    }

    #[test]
    fn toggle_starts_a_stopped_project_in_the_background() {
        let (_tmp, shop, _) = shop();
        let state = tempfile::tempdir().unwrap();
        let fake = FakeRunner::new();
        fake.on(&["herdr", "pane", "current"], pane_json(&shop));
        fake.on(&["docker"], Output::ok(""));
        let spawned = recorder();
        let spawn = |args: &[String]| -> anyhow::Result<()> {
            spawned.borrow_mut().push(args.to_vec());
            Ok(())
        };
        let ctx = testing::ctx(&fake, state.path(), &spawn);
        run(Kind::Toggle, &ctx).unwrap();
        let spawned = spawned.borrow();
        assert_eq!(spawned[0], ["ticker"]);
        assert_eq!(
            spawned[1],
            ["worker", "start", shop.to_str().unwrap(), "shop"]
        );
    }

    #[test]
    fn toggle_stops_a_running_project() {
        let (_tmp, shop, _) = shop();
        let state = tempfile::tempdir().unwrap();
        let fake = FakeRunner::new();
        fake.on(&["herdr", "pane", "current"], pane_json(&shop));
        fake.on(
            &["docker"],
            Output::ok(&format!("shop\t{}\trunning\tweb\n", shop.display())),
        );
        let spawned = recorder();
        let spawn = |args: &[String]| -> anyhow::Result<()> {
            spawned.borrow_mut().push(args.to_vec());
            Ok(())
        };
        let ctx = testing::ctx(&fake, state.path(), &spawn);
        run(Kind::Toggle, &ctx).unwrap();
        assert_eq!(spawned.borrow()[1][1], "stop");
    }

    #[test]
    fn uses_the_pane_herdr_names() {
        let (_tmp, shop, _) = shop();
        let state = tempfile::tempdir().unwrap();
        let fake = FakeRunner::new();
        fake.on(&["herdr", "pane", "get", "w2:p1"], pane_json(&shop));
        fake.on(&["docker"], Output::ok(""));
        let mut ctx = testing::ctx(&fake, state.path(), &testing::no_spawn);
        ctx.pane_id = Some("w2:p1".to_string());
        run(Kind::Start, &ctx).unwrap();
        assert!(
            fake.calls_starting_with(&["herdr", "pane", "current"])
                .is_empty()
        );
    }

    #[test]
    fn outside_a_project_notifies_and_starts_no_worker() {
        let (_tmp, _, notes) = shop();
        let state = tempfile::tempdir().unwrap();
        let fake = FakeRunner::new();
        fake.on(&["herdr", "pane", "current"], pane_json(&notes));
        fake.on(&["docker"], Output::ok(""));
        fake.on(&["herdr", "notification"], Output::ok("{}"));
        let spawned = recorder();
        let spawn = |args: &[String]| -> anyhow::Result<()> {
            spawned.borrow_mut().push(args.to_vec());
            Ok(())
        };
        let ctx = testing::ctx(&fake, state.path(), &spawn);
        run(Kind::Toggle, &ctx).unwrap();
        assert_eq!(notifications(&fake)[0].0, "Not in a ddev project");
        assert_eq!(spawned.borrow().len(), 1);
    }

    #[test]
    fn missing_ddev_names_the_config_key() {
        let state = tempfile::tempdir().unwrap();
        let fake = FakeRunner::new();
        fake.on(&["herdr", "notification"], Output::ok("{}"));
        let mut ctx = testing::ctx(&fake, state.path(), &testing::no_spawn);
        ctx.ddev = None;
        run(Kind::Toggle, &ctx).unwrap();
        assert!(notifications(&fake)[0].0.contains("ddev_command"));
    }

    #[test]
    fn open_in_browser_mode_runs_the_opener() {
        let (_tmp, shop, _) = shop();
        let state = tempfile::tempdir().unwrap();
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let fake = FakeRunner::new();
        fake.on(&["herdr", "pane", "current"], pane_json(&shop));
        fake.on(&["docker"], Output::ok(""));
        fake.on(&["ddev", "describe"], Output::ok(DESCRIBE));
        fake.on(&[opener], Output::ok(""));
        let mut ctx = testing::ctx(&fake, state.path(), &testing::no_spawn);
        ctx.open_mode = OpenMode::Browser;
        run(Kind::Open, &ctx).unwrap();
        assert_eq!(
            fake.calls_starting_with(&[opener])[0].argv,
            [opener, "https://shop.ddev.site"]
        );
    }

    #[test]
    fn open_in_clipboard_mode_opens_the_url_popup() {
        let (_tmp, shop, _) = shop();
        let state = tempfile::tempdir().unwrap();
        let fake = FakeRunner::new();
        fake.on(&["herdr", "pane", "current"], pane_json(&shop));
        fake.on(&["docker"], Output::ok(""));
        fake.on(&["ddev", "describe"], Output::ok(DESCRIBE));
        fake.on(&["herdr", "plugin", "pane", "open"], Output::ok("{}"));
        let mut ctx = testing::ctx(&fake, state.path(), &testing::no_spawn);
        ctx.open_mode = OpenMode::Clipboard;
        run(Kind::Open, &ctx).unwrap();
        let call = &fake.calls_starting_with(&["herdr", "plugin", "pane", "open"])[0].argv;
        assert!(call.contains(&"url".to_string()));
        assert!(call.contains(&"HERDR_DDEV_URL=https://shop.ddev.site".to_string()));
    }

    #[test]
    fn picker_action_opens_the_picker_pane() {
        let state = tempfile::tempdir().unwrap();
        let fake = FakeRunner::new();
        fake.on(&["herdr", "plugin", "pane", "open"], Output::ok("{}"));
        let ctx = testing::ctx(&fake, state.path(), &testing::no_spawn);
        run(Kind::Picker, &ctx).unwrap();
        let call = &fake.calls()[0].argv;
        assert_eq!(&call[5..8], ["danjuls.ddev", "--entrypoint", "picker"]);
    }

    fn worker_world(ddev_start: Output) -> (tempfile::TempDir, PathBuf, FakeRunner) {
        let (tmp, shop, _) = shop();
        let fake = FakeRunner::new();
        fake.on(&["herdr", "pane", "list"], pane_list_json(&shop));
        fake.on(&["docker"], Output::ok(""));
        fake.on(&["herdr", "workspace", "report-metadata"], Output::ok("{}"));
        fake.on(&["herdr", "notification"], Output::ok("{}"));
        fake.on(&["ddev", "start"], ddev_start);
        (tmp, shop, fake)
    }

    #[test]
    fn worker_marks_busy_runs_ddev_and_notifies() {
        let (_tmp, shop, fake) = worker_world(Output::ok(""));
        let state = tempfile::tempdir().unwrap();
        let ctx = testing::ctx(&fake, state.path(), &testing::no_spawn);
        worker(&ctx, Verb::Start, &shop, "shop").unwrap();
        let ddev_call = &fake.calls_starting_with(&["ddev", "start"])[0];
        assert_eq!(ddev_call.cwd.as_deref(), Some(shop.as_path()));
        let badges: Vec<String> = fake
            .calls_starting_with(&["herdr", "workspace", "report-metadata"])
            .into_iter()
            .map(|call| call.argv[7].clone())
            .collect();
        assert_eq!(badges, ["ddev=◌ ddev…", "ddev=○ ddev"]);
        assert_eq!(
            notifications(&fake),
            [("shop started".to_string(), "done".to_string())]
        );
        assert!(
            busy::read(
                state.path(),
                "shop",
                unix_ms() / 1000,
                &testing::always_alive
            )
            .is_none()
        );
    }

    #[test]
    fn worker_refuses_when_the_project_is_busy() {
        let (_tmp, shop, fake) = worker_world(Output::ok(""));
        let state = tempfile::tempdir().unwrap();
        let now = unix_ms() / 1000;
        let marker = Marker {
            project: "shop".into(),
            verb: Verb::Start,
            pid: 1,
            started_unix: now,
        };
        busy::acquire(state.path(), &marker, now, &testing::always_alive).unwrap();
        let ctx = testing::ctx(&fake, state.path(), &testing::no_spawn);
        worker(&ctx, Verb::Stop, &shop, "shop").unwrap();
        assert_eq!(notifications(&fake)[0].0, "shop is already starting");
        assert!(fake.calls_starting_with(&["ddev"]).is_empty());
    }

    #[test]
    fn worker_reports_the_first_ddev_error_line() {
        let (_tmp, shop, fake) =
            worker_world(Output::fail("Failed to start shop: port 443 in use"));
        let state = tempfile::tempdir().unwrap();
        let ctx = testing::ctx(&fake, state.path(), &testing::no_spawn);
        worker(&ctx, Verb::Start, &shop, "shop").unwrap();
        assert_eq!(
            notifications(&fake),
            [(
                "shop: Failed to start shop: port 443 in use".to_string(),
                "request".to_string()
            )]
        );
        assert!(
            busy::read(
                state.path(),
                "shop",
                unix_ms() / 1000,
                &testing::always_alive
            )
            .is_none()
        );
    }
}
