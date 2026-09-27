//! Keeps each workspace's `$ddev` sidebar badge in step with Docker.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::app::Ctx;
use crate::docker::{self, Project};
use crate::herdr::Pane;
use crate::project::{self, StoppedCache};
use crate::{badge, busy};

use std::path::PathBuf;

use anyhow::Result;

use crate::app::unix_ms;
use crate::herdr::Sound;
use crate::{config, lock};

/// How long a badge lives without a refresh, so badges vanish if the ticker dies.
pub fn ttl(interval: Duration) -> Duration {
    (interval * 4).max(Duration::from_secs(20))
}

/// The poll interval after `docker_failures` failed Docker queries in a row: doubling from
/// the base, capped at 30s, back to the base after a success.
pub fn backoff(base: Duration, docker_failures: u32) -> Duration {
    if docker_failures == 0 {
        return base;
    }
    let factor = 2u32.saturating_pow(docker_failures.min(10));
    base.saturating_mul(factor)
        .min(Duration::from_secs(30))
        .max(base)
}

/// Each workspace's project: the one most of its panes resolve to. Ties go to the project of
/// the focused pane, then to the project of the earliest pane.
pub fn workspace_projects(
    panes: &[Pane],
    resolve: &mut dyn FnMut(&Path) -> Option<Project>,
) -> BTreeMap<String, Project> {
    let mut per_workspace: BTreeMap<&str, Vec<(bool, Project)>> = BTreeMap::new();
    for pane in panes {
        let Some(project) = pane.dir().and_then(|dir| resolve(Path::new(dir))) else {
            continue;
        };
        per_workspace
            .entry(pane.workspace_id.as_str())
            .or_default()
            .push((pane.focused, project));
    }
    per_workspace
        .into_iter()
        .filter_map(|(workspace, entries)| pick(entries).map(|p| (workspace.to_string(), p)))
        .collect()
}

fn pick(entries: Vec<(bool, Project)>) -> Option<Project> {
    // (project, votes, has the focused pane, position of its first pane)
    let mut tally: Vec<(Project, usize, bool, usize)> = Vec::new();
    for (position, (focused, project)) in entries.into_iter().enumerate() {
        match tally.iter_mut().find(|entry| entry.0.root == project.root) {
            Some(entry) => {
                entry.1 += 1;
                entry.2 |= focused;
            }
            None => tally.push((project, 1, focused, position)),
        }
    }
    tally
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)).then(b.3.cmp(&a.3)))
        .map(|entry| entry.0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    Set { workspace: String, value: String },
    Clear { workspace: String },
}

#[derive(Debug, Clone)]
pub struct Reported {
    pub value: String,
    pub at: Instant,
}

/// The badge calls to make: new or changed values, unchanged ones past half their TTL, and
/// clears for workspaces that no longer have a project.
pub fn plan_reports(
    wanted: &BTreeMap<String, String>,
    last: &HashMap<String, Reported>,
    now: Instant,
    ttl: Duration,
) -> Vec<Report> {
    let mut reports: Vec<Report> = wanted
        .iter()
        .filter(|(workspace, value)| {
            !last.get(*workspace).is_some_and(|reported| {
                reported.value == **value && now.duration_since(reported.at) < ttl / 2
            })
        })
        .map(|(workspace, value)| Report::Set {
            workspace: workspace.clone(),
            value: value.clone(),
        })
        .collect();
    let mut gone: Vec<&String> = last.keys().filter(|w| !wanted.contains_key(*w)).collect();
    gone.sort();
    reports.extend(gone.into_iter().map(|w| Report::Clear {
        workspace: w.clone(),
    }));
    reports
}

#[derive(Debug)]
pub enum TickError {
    Herdr(anyhow::Error),
    Docker(anyhow::Error),
}

pub struct Ticker<'a> {
    ctx: &'a Ctx<'a>,
    docker: Vec<String>,
    cache: StoppedCache,
    last: HashMap<String, Reported>,
}

impl<'a> Ticker<'a> {
    pub fn new(ctx: &'a Ctx<'a>, docker: Vec<String>) -> Ticker<'a> {
        let cache = StoppedCache::new(Duration::from_secs(60));
        Ticker {
            ctx,
            docker,
            cache,
            last: HashMap::new(),
        }
    }

    /// One poll: read panes and Docker, then send the badge changes.
    pub fn tick(&mut self, now: Instant, now_unix_ms: u64) -> Result<(), TickError> {
        let ctx = self.ctx;
        let panes = ctx.herdr.panes().map_err(TickError::Herdr)?;
        let found = docker::query(ctx.runner, &self.docker).map_err(TickError::Docker)?;
        let running = project::canonical_projects(found);
        let busy = busy::live_all(&ctx.state_dir, now_unix_ms / 1000, ctx.pid_alive);
        let cache = &mut self.cache;
        let projects = workspace_projects(&panes, &mut |dir| {
            project::resolve(dir, &running, cache, now)
        });
        let wanted: BTreeMap<String, String> = projects
            .into_iter()
            .map(|(workspace, project)| {
                let value = badge::text(project.state, busy.get(&project.name).copied());
                (workspace, value.to_string())
            })
            .collect();
        let live: HashSet<&str> = panes
            .iter()
            .map(|pane| pane.workspace_id.as_str())
            .collect();
        self.last
            .retain(|workspace, _| live.contains(workspace.as_str()));
        let ttl = ttl(ctx.interval);
        for report in plan_reports(&wanted, &self.last, now, ttl) {
            match report {
                Report::Set { workspace, value } => {
                    let ttl_ms = ttl.as_millis() as u64;
                    ctx.herdr
                        .set_badge(&workspace, &value, now_unix_ms, ttl_ms)
                        .map_err(TickError::Herdr)?;
                    self.last.insert(workspace, Reported { value, at: now });
                }
                Report::Clear { workspace } => {
                    ctx.herdr
                        .clear_badge(&workspace, now_unix_ms)
                        .map_err(TickError::Herdr)?;
                    self.last.remove(&workspace);
                }
            }
        }
        Ok(())
    }
}

pub fn lock_path(ctx: &Ctx) -> PathBuf {
    ctx.state_dir.join("ticker.lock")
}

/// Start a background ticker unless one already runs on this machine.
pub fn ensure_running(ctx: &Ctx) -> Result<()> {
    match lock::try_lock(&lock_path(ctx))? {
        Some(lock) => {
            drop(lock);
            (ctx.spawn)(&["ticker".to_string()])
        }
        None => Ok(()),
    }
}

/// The ticker loop. Exits quietly when another ticker holds the lock, when docker is missing,
/// and after three failed Herdr calls in a row (the server is gone).
pub fn run(ctx: &Ctx, config_error: Option<&str>) -> Result<()> {
    let Some(_lock) = lock::try_lock(&lock_path(ctx))? else {
        return Ok(());
    };
    if let Some(error) = config_error {
        let body = format!("invalid config, using defaults: {error}");
        let _ = ctx.herdr.notify(&body, Sound::Request);
    }
    let Some(docker) = ctx.docker.clone() else {
        let file = ctx.config_dir.join(config::FILE_NAME);
        eprintln!(
            "herdr-ddev ticker: docker not found; set docker_command in {}",
            file.display()
        );
        return Ok(());
    };
    let mut ticker = Ticker::new(ctx, docker);
    let (mut herdr_failures, mut docker_failures) = (0u32, 0u32);
    loop {
        match ticker.tick(Instant::now(), unix_ms()) {
            Ok(()) => {
                herdr_failures = 0;
                docker_failures = 0;
            }
            Err(TickError::Docker(err)) => {
                herdr_failures = 0;
                docker_failures += 1;
                eprintln!("herdr-ddev ticker: {err:#}");
            }
            Err(TickError::Herdr(err)) => {
                herdr_failures += 1;
                eprintln!("herdr-ddev ticker: {err:#}");
                if herdr_failures >= 3 {
                    return Ok(());
                }
            }
        }
        std::thread::sleep(backoff(ctx.interval, docker_failures));
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::app::testing;
    use crate::busy::{Marker, Verb};
    use crate::docker::State;
    use crate::runner::Output;
    use crate::runner::fake::FakeRunner;

    fn running_at(path: &str) -> Project {
        Project {
            name: path.to_string(),
            root: path.into(),
            state: State::Running,
        }
    }

    fn pane(workspace: &str, dir: &str, focused: bool) -> Pane {
        Pane {
            pane_id: format!("{workspace}:{dir}"),
            workspace_id: workspace.to_string(),
            focused,
            cwd: Some(dir.to_string()),
            foreground_cwd: None,
        }
    }

    fn by_path(dir: &Path) -> Option<Project> {
        let dir = dir.to_str().unwrap();
        (dir != "/none").then(|| running_at(dir))
    }

    #[test]
    fn ttl_is_four_intervals_but_at_least_20s() {
        assert_eq!(ttl(Duration::from_secs(5)), Duration::from_secs(20));
        assert_eq!(ttl(Duration::from_secs(15)), Duration::from_secs(60));
        assert_eq!(ttl(Duration::from_secs(1)), Duration::from_secs(20));
    }

    #[test]
    fn backoff_doubles_up_to_30s() {
        let base = Duration::from_secs(5);
        let steps: Vec<u64> = [0, 1, 2, 3, 9]
            .iter()
            .map(|n| backoff(base, *n).as_secs())
            .collect();
        assert_eq!(steps, [5, 10, 20, 30, 30]);
        assert_eq!(backoff(Duration::from_secs(15), 1), Duration::from_secs(30));
    }

    #[test]
    fn workspace_takes_the_majority_project() {
        let panes = [
            pane("w1", "/a", false),
            pane("w1", "/b", false),
            pane("w1", "/b", false),
        ];
        assert_eq!(workspace_projects(&panes, &mut by_path)["w1"].name, "/b");
    }

    #[test]
    fn ties_go_to_the_focused_pane_then_the_first_pane() {
        let focused = [pane("w1", "/a", false), pane("w1", "/b", true)];
        assert_eq!(workspace_projects(&focused, &mut by_path)["w1"].name, "/b");
        let plain = [pane("w1", "/a", false), pane("w1", "/b", false)];
        assert_eq!(workspace_projects(&plain, &mut by_path)["w1"].name, "/a");
    }

    #[test]
    fn workspaces_without_a_project_are_left_out() {
        let panes = [pane("w1", "/a", false), pane("w2", "/none", false)];
        let map = workspace_projects(&panes, &mut by_path);
        assert_eq!(map.keys().collect::<Vec<_>>(), ["w1"]);
    }

    fn wanted(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(w, v)| (w.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn plan_sets_new_and_changed_values_and_clears_lost_ones() {
        let now = Instant::now();
        let mut last = HashMap::new();
        last.insert(
            "w1".to_string(),
            Reported {
                value: "● ddev".into(),
                at: now,
            },
        );
        last.insert(
            "w2".to_string(),
            Reported {
                value: "● ddev".into(),
                at: now,
            },
        );
        let reports = plan_reports(
            &wanted(&[("w1", "○ ddev"), ("w3", "● ddev")]),
            &last,
            now,
            ttl(Duration::from_secs(5)),
        );
        assert_eq!(
            reports,
            [
                Report::Set {
                    workspace: "w1".into(),
                    value: "○ ddev".into()
                },
                Report::Set {
                    workspace: "w3".into(),
                    value: "● ddev".into()
                },
                Report::Clear {
                    workspace: "w2".into()
                },
            ]
        );
    }

    #[test]
    fn plan_refreshes_unchanged_values_after_half_the_ttl() {
        let now = Instant::now();
        let ttl = ttl(Duration::from_secs(5));
        let mut last = HashMap::new();
        last.insert(
            "w1".to_string(),
            Reported {
                value: "● ddev".into(),
                at: now,
            },
        );
        let same = wanted(&[("w1", "● ddev")]);
        assert!(plan_reports(&same, &last, now + Duration::from_secs(9), ttl).is_empty());
        assert_eq!(
            plan_reports(&same, &last, now + Duration::from_secs(10), ttl).len(),
            1
        );
    }

    /// shop and blog have `.ddev/config.yaml`; notes is a plain folder.
    struct World {
        _tmp: tempfile::TempDir,
        shop: PathBuf,
        blog: PathBuf,
        notes: PathBuf,
    }

    fn world() -> World {
        let tmp = tempfile::tempdir().unwrap();
        let root = crate::project::canonical(tmp.path());
        let (shop, blog, notes) = (root.join("shop"), root.join("blog"), root.join("notes"));
        for (dir, name) in [(&shop, Some("shop")), (&blog, Some("blog")), (&notes, None)] {
            fs::create_dir_all(dir).unwrap();
            if let Some(name) = name {
                fs::create_dir_all(dir.join(".ddev")).unwrap();
                fs::write(dir.join(".ddev/config.yaml"), format!("name: {name}\n")).unwrap();
            }
        }
        World {
            _tmp: tmp,
            shop,
            blog,
            notes,
        }
    }

    fn pane_list(panes: &[(&str, &Path)]) -> Output {
        let panes: Vec<String> = panes
            .iter()
            .enumerate()
            .map(|(i, (ws, dir))| {
                format!(
                    r#"{{"pane_id":"{ws}:p{i}","workspace_id":"{ws}","focused":false,"cwd":"{}"}}"#,
                    dir.display()
                )
            })
            .collect();
        Output::ok(&format!(
            r#"{{"result":{{"panes":[{}]}}}}"#,
            panes.join(",")
        ))
    }

    fn docker_rows(name: &str, root: &Path, state: &str) -> Output {
        Output::ok(&format!("{name}\t{}\t{state}\tweb\n", root.display()))
    }

    fn badge_calls(fake: &FakeRunner) -> Vec<(String, String)> {
        fake.calls_starting_with(&["herdr", "workspace", "report-metadata"])
            .into_iter()
            .map(|call| {
                (
                    call.argv[3].clone(),
                    format!("{} {}", call.argv[6], call.argv[7]),
                )
            })
            .collect()
    }

    fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
        expected
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn tick_reports_running_and_stopped_badges() {
        let w = world();
        let fake = FakeRunner::new();
        fake.on(
            &["herdr", "pane", "list"],
            pane_list(&[("w1", &w.shop), ("w2", &w.blog), ("w3", &w.notes)]),
        );
        fake.on(&["docker"], docker_rows("shop", &w.shop, "running"));
        fake.on(&["herdr", "workspace", "report-metadata"], Output::ok("{}"));
        let state = tempfile::tempdir().unwrap();
        let ctx = testing::ctx(&fake, state.path(), &testing::no_spawn);
        let mut ticker = Ticker::new(&ctx, vec!["docker".into()]);
        ticker.tick(Instant::now(), 1_000_000).unwrap();
        assert_eq!(
            badge_calls(&fake),
            pairs(&[("w1", "--token ddev=● ddev"), ("w2", "--token ddev=○ ddev")])
        );
    }

    #[test]
    fn paused_project_shows_half_circle() {
        let w = world();
        let fake = FakeRunner::new();
        fake.on(&["herdr", "pane", "list"], pane_list(&[("w1", &w.shop)]));
        fake.on(&["docker"], docker_rows("shop", &w.shop, "exited"));
        fake.on(&["herdr", "workspace", "report-metadata"], Output::ok("{}"));
        let state = tempfile::tempdir().unwrap();
        let ctx = testing::ctx(&fake, state.path(), &testing::no_spawn);
        Ticker::new(&ctx, vec!["docker".into()])
            .tick(Instant::now(), 1_000_000)
            .unwrap();
        assert_eq!(badge_calls(&fake), pairs(&[("w1", "--token ddev=◐ ddev")]));
    }

    #[test]
    fn unchanged_badges_are_not_resent_until_half_the_ttl() {
        let w = world();
        let fake = FakeRunner::new();
        fake.on(&["herdr", "pane", "list"], pane_list(&[("w1", &w.shop)]));
        fake.on(&["docker"], docker_rows("shop", &w.shop, "running"));
        fake.on(&["herdr", "workspace", "report-metadata"], Output::ok("{}"));
        let state = tempfile::tempdir().unwrap();
        let ctx = testing::ctx(&fake, state.path(), &testing::no_spawn);
        let mut ticker = Ticker::new(&ctx, vec!["docker".into()]);
        let t0 = Instant::now();
        ticker.tick(t0, 1_000_000).unwrap();
        ticker.tick(t0 + Duration::from_secs(5), 1_005_000).unwrap();
        assert_eq!(badge_calls(&fake).len(), 1);
        ticker
            .tick(t0 + Duration::from_secs(11), 1_011_000)
            .unwrap();
        assert_eq!(badge_calls(&fake).len(), 2);
    }

    #[test]
    fn a_project_stopped_outside_the_plugin_turns_grey() {
        let w = world();
        let fake = FakeRunner::new();
        fake.once(&["docker"], docker_rows("shop", &w.shop, "running"));
        fake.on(&["docker"], Output::ok(""));
        fake.on(&["herdr", "pane", "list"], pane_list(&[("w1", &w.shop)]));
        fake.on(&["herdr", "workspace", "report-metadata"], Output::ok("{}"));
        let state = tempfile::tempdir().unwrap();
        let ctx = testing::ctx(&fake, state.path(), &testing::no_spawn);
        let mut ticker = Ticker::new(&ctx, vec!["docker".into()]);
        ticker.tick(Instant::now(), 1_000_000).unwrap();
        ticker.tick(Instant::now(), 1_005_000).unwrap();
        assert_eq!(
            badge_calls(&fake),
            pairs(&[("w1", "--token ddev=● ddev"), ("w1", "--token ddev=○ ddev")])
        );
    }

    #[test]
    fn workspace_that_leaves_its_project_is_cleared() {
        let w = world();
        let fake = FakeRunner::new();
        fake.once(&["herdr", "pane", "list"], pane_list(&[("w1", &w.shop)]));
        fake.on(&["herdr", "pane", "list"], pane_list(&[("w1", &w.notes)]));
        fake.on(&["docker"], docker_rows("shop", &w.shop, "running"));
        fake.on(&["herdr", "workspace", "report-metadata"], Output::ok("{}"));
        let state = tempfile::tempdir().unwrap();
        let ctx = testing::ctx(&fake, state.path(), &testing::no_spawn);
        let mut ticker = Ticker::new(&ctx, vec!["docker".into()]);
        ticker.tick(Instant::now(), 1_000_000).unwrap();
        ticker.tick(Instant::now(), 1_005_000).unwrap();
        assert_eq!(
            badge_calls(&fake),
            pairs(&[("w1", "--token ddev=● ddev"), ("w1", "--clear-token ddev")])
        );
    }

    #[test]
    fn busy_marker_shows_the_busy_badge() {
        let w = world();
        let fake = FakeRunner::new();
        fake.on(&["herdr", "pane", "list"], pane_list(&[("w1", &w.shop)]));
        fake.on(&["docker"], docker_rows("shop", &w.shop, "running"));
        fake.on(&["herdr", "workspace", "report-metadata"], Output::ok("{}"));
        let state = tempfile::tempdir().unwrap();
        let marker = Marker {
            project: "shop".into(),
            verb: Verb::Stop,
            pid: 1,
            started_unix: 1_000,
        };
        busy::acquire(state.path(), &marker, 1_000, &testing::always_alive).unwrap();
        let ctx = testing::ctx(&fake, state.path(), &testing::no_spawn);
        Ticker::new(&ctx, vec!["docker".into()])
            .tick(Instant::now(), 1_000_000)
            .unwrap();
        assert_eq!(badge_calls(&fake), pairs(&[("w1", "--token ddev=◌ ddev…")]));
    }

    #[test]
    fn docker_failure_sends_nothing() {
        let w = world();
        let fake = FakeRunner::new();
        fake.on(&["herdr", "pane", "list"], pane_list(&[("w1", &w.shop)]));
        fake.on(
            &["docker"],
            Output::fail("Cannot connect to the Docker daemon"),
        );
        let state = tempfile::tempdir().unwrap();
        let ctx = testing::ctx(&fake, state.path(), &testing::no_spawn);
        let result = Ticker::new(&ctx, vec!["docker".into()]).tick(Instant::now(), 1_000_000);
        assert!(matches!(result, Err(TickError::Docker(_))));
        assert!(badge_calls(&fake).is_empty());
    }

    #[test]
    fn herdr_failure_is_a_herdr_error() {
        let fake = FakeRunner::new();
        fake.on(&["herdr"], Output::fail("no server"));
        let state = tempfile::tempdir().unwrap();
        let ctx = testing::ctx(&fake, state.path(), &testing::no_spawn);
        let result = Ticker::new(&ctx, vec!["docker".into()]).tick(Instant::now(), 1_000_000);
        assert!(matches!(result, Err(TickError::Herdr(_))));
    }

    #[test]
    fn ensure_running_spawns_only_when_no_ticker_holds_the_lock() {
        let state = tempfile::tempdir().unwrap();
        let fake = FakeRunner::new();
        let spawned = std::cell::RefCell::new(Vec::new());
        let spawn = |args: &[String]| -> anyhow::Result<()> {
            spawned.borrow_mut().push(args.to_vec());
            Ok(())
        };
        let ctx = testing::ctx(&fake, state.path(), &spawn);
        ensure_running(&ctx).unwrap();
        assert_eq!(spawned.borrow().as_slice(), [vec!["ticker".to_string()]]);
        let held = crate::lock::try_lock(&lock_path(&ctx)).unwrap().unwrap();
        ensure_running(&ctx).unwrap();
        assert_eq!(spawned.borrow().len(), 1);
        drop(held);
    }

    #[test]
    fn run_exits_quietly_when_another_ticker_holds_the_lock() {
        let state = tempfile::tempdir().unwrap();
        let fake = FakeRunner::new();
        let ctx = testing::ctx(&fake, state.path(), &testing::no_spawn);
        let _held = crate::lock::try_lock(&lock_path(&ctx)).unwrap().unwrap();
        run(&ctx, None).unwrap();
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn run_exits_after_three_failed_herdr_calls() {
        let state = tempfile::tempdir().unwrap();
        let fake = FakeRunner::new();
        fake.on(&["herdr"], Output::fail("no server"));
        let mut ctx = testing::ctx(&fake, state.path(), &testing::no_spawn);
        ctx.interval = Duration::from_millis(1);
        run(&ctx, None).unwrap();
        assert_eq!(
            fake.calls_starting_with(&["herdr", "pane", "list"]).len(),
            3
        );
    }
}
