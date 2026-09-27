# herdr-ddev v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build herdr-ddev v1, a Herdr plugin with norns-companion's ddev features: live
per-workspace status badges, start/stop/restart/open from a key, a project picker, and
configure/unconfigure popups.

**Architecture:** One Rust binary (`herdr-ddev`) with subcommands, split into a library crate of
small modules plus a thin `main.rs`. Every call to ddev, docker, herdr or a browser opener goes
through a `Runner` trait so scenario tests can script the world. A background ticker polls Docker
labels every 5s and writes each workspace's `$ddev` token with a TTL; actions hand slow ddev
calls to a detached worker.

**Tech Stack:** Rust 2024 (MSRV 1.89), serde + serde_json, toml 1, toml_edit 0.25, ratatui 0.30,
crossterm 0.29, anyhow; tempfile for tests; POSIX `sh` for the install script; GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-09-27-herdr-ddev-design.md`

## Global Constraints

- Crate `herdr-ddev`, `edition = "2024"`, `rust-version = "1.89"`, `rustfmt.toml` `max_width = 100`.
- Library `herdr_ddev` (`src/lib.rs`, every module `pub mod`) plus binary `herdr-ddev`
  (`src/main.rs`). Public library items never trip dead-code lints while the binary catches up.
- Plugin id `danjuls.ddev`. Action ids: `toggle`, `start`, `stop`, `restart`, `open`, `picker`,
  `configure`, `unconfigure`. Pane ids: `picker`, `configure`, `unconfigure`, `url`.
- Sidebar token `ddev`, metadata source `danjuls.ddev`. Badge values exactly `● ddev` (running),
  `◐ ddev` (paused), `○ ddev` (stopped), `◌ ddev…` (busy).
- Default keys `prefix+shift+s` toggle, `prefix+shift+o` open, `prefix+shift+e` picker. Picker
  keys `ctrl+s` start/stop, `ctrl+r` restart, `ctrl+o` open, `ctrl+n`/`ctrl+p` and arrows move.
- Manifest `min_herdr_version = "0.9.1"`, `platforms = ["macos", "linux"]`.
- Dependencies are exactly: `anyhow 1`, `serde 1` (derive), `serde_json 1`, `toml 1`,
  `toml_edit 0.25`, `ratatui 0.30`, `crossterm 0.29`; dev: `tempfile 3`. No async runtime. Ask
  Daniel before adding any other crate.
- Only `detach::spawn_background` and `busy::pid_alive` spawn processes directly; everything
  else goes through `runner::Runner`.
- Numbers: poll 5s default; badge TTL `max(4 x interval, 20s)`; `--seq` = Unix milliseconds;
  Docker backoff 5s, 10s, 20s, 30s cap; ticker exits after 3 failed herdr calls in a row; busy
  markers expire after 15 minutes; stopped-project cache 60s.
- Tests never touch Daniel's real Herdr config, state or session: temp dirs, `HERDR_CONFIG_PATH`,
  `FakeRunner`. Task 15's smoke test is the one exception and says so.
- Before every commit: `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
  Conventional commits, at most 3 lines, no AI attribution.
- User-facing text (README, notifications, popups) uses plain hyphens, never em or en dashes.
- Spec deviation: actions find the focused pane from `HERDR_PANE_ID` (documented) via
  `herdr pane get`, else `herdr pane current`, instead of parsing `HERDR_PLUGIN_CONTEXT_JSON`,
  whose shape Herdr 0.9.1 does not document.
- Spec deviation, documented in Task 17: the detached worker's full ddev output goes to
  `worker.log` in the plugin state dir (`~/.local/state/herdr/plugins/danjuls.ddev/`), not to
  `herdr plugin log`, because the worker runs detached from Herdr.

## Review Focus

1. Folders reached through symlinks, with trailing slashes, under macOS `/tmp` vs `/private/tmp`,
   or not existing (yet) must still match their project. Pinned in Task 5.
2. Paths and folder names with spaces must survive Docker parsing and worker arguments intact.
   Pinned in Tasks 4 and 12.
3. A real-world Herdr config with comments and existing `[[keys.command]]` entries must keep
   both through configure, and unconfigure must return it to the same settings. Pinned in Task 14.
4. ddev `-j` output that prints warning objects before the result must still parse. Pinned in
   Task 10.
5. A project name containing `/` or `..` must not write a busy marker outside the busy folder.
   Pinned in Task 7.

---

### Task 1: Crate scaffold, CI and license

**Files:**
- Create: `Cargo.toml`, `rustfmt.toml`, `.gitignore`, `LICENSE`, `src/lib.rs`, `src/main.rs`,
  `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: nothing.
- Produces: `fn run(args: &[String]) -> anyhow::Result<()>` in `src/main.rs` (extended by later
  tasks), `const USAGE: &str`.

- [ ] **Step 1: Create the manifest files**

`Cargo.toml`:

```toml
[package]
name = "herdr-ddev"
version = "0.1.0"
edition = "2024"
rust-version = "1.89"
description = "ddev for Herdr: status badges, start/stop/open from a key, and a project picker"
license = "MIT"
repository = "https://github.com/danjuls/herdr-ddev"

[dependencies]
anyhow = "1"

[profile.release]
lto = true
strip = true
```

`rustfmt.toml`:

```toml
max_width = 100
```

`.gitignore`:

```text
/target
/bin
```

`LICENSE`:

```text
MIT License

Copyright (c) 2026 Daniel Andreasson

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

`src/lib.rs`:

```rust
//! herdr-ddev: ddev status badges, start/stop/open actions and a project picker for Herdr.
```

- [ ] **Step 2: Write the failing tests**

`src/main.rs`:

```rust
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
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test`
Expected: compile error `cannot find function 'run' in this scope`.

- [ ] **Step 4: Implement `run`**

Add to `src/main.rs`, above `#[cfg(test)]`:

```rust
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
```

- [ ] **Step 5: Add CI**

`.github/workflows/ci.yml`:

```yaml
name: ci
on:
  push:
    branches: [main]
  pull_request:
jobs:
  test:
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - run: cargo fmt --check
      - run: cargo clippy --all-targets -- -D warnings
      - run: cargo test
```

- [ ] **Step 6: Run everything**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test && cargo run -q -- --version`
Expected: 2 tests pass, last line `herdr-ddev 0.1.0`.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock rustfmt.toml .gitignore LICENSE src .github
git commit -m "chore: scaffold herdr-ddev crate with CI"
```

---

### Task 2: Runner and binary lookup

**Files:**
- Create: `src/runner.rs`, `src/bins.rs`
- Modify: `src/lib.rs`, `Cargo.toml` (dev-dependency `tempfile`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `runner::Output { success: bool, stdout: String, stderr: String }`, `Output::ok(&str)`,
    `Output::fail(&str)`
  - `trait runner::Runner { fn run(&self, argv: &[String], cwd: Option<&Path>) -> Result<Output>; }`
  - `runner::SystemRunner::new(path: String)`
  - `runner::Call { argv: Vec<String>, cwd: Option<PathBuf> }`
  - test-only `runner::fake::FakeRunner` with `new()`, `on(&[&str], Output)`,
    `once(&[&str], Output)`, `calls() -> Vec<Call>`, `calls_starting_with(&[&str]) -> Vec<Call>`
  - `bins::DDEV_CANDIDATES`, `bins::DOCKER_CANDIDATES: &[&str]`
  - `bins::resolve(configured: Option<&[String]>, name: &str, candidates: &[&str],
    path_var: &str, home: &str, is_executable: &dyn Fn(&Path) -> bool) -> Option<Vec<String>>`
  - `bins::widened_path(binaries: &[&[String]], inherited: &str, home: &str) -> String`
  - `bins::expand_home(&str, &str) -> PathBuf`, `bins::is_executable_file(&Path) -> bool`

- [ ] **Step 1: Add the test dependency and modules**

In `Cargo.toml` add:

```toml
[dev-dependencies]
tempfile = "3"
```

In `src/lib.rs` add:

```rust
pub mod bins;
pub mod runner;
```

- [ ] **Step 2: Write the failing tests**

`src/runner.rs` (tests only for now):

```rust
//! The only place that spawns ddev, docker and herdr, so tests can script them.

#[cfg(test)]
mod tests {
    use super::fake::FakeRunner;
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn system_runner_captures_output_and_status() {
        let runner = SystemRunner::new(std::env::var("PATH").unwrap_or_default());
        let script = argv(&["sh", "-c", "echo hi; echo oops >&2; exit 3"]);
        let out = runner.run(&script, None).unwrap();
        assert!(!out.success);
        assert_eq!(out.stdout, "hi\n");
        assert_eq!(out.stderr, "oops\n");
    }

    #[test]
    fn system_runner_uses_cwd() {
        let dir = std::env::temp_dir().canonicalize().unwrap();
        let runner = SystemRunner::new(std::env::var("PATH").unwrap_or_default());
        let out = runner.run(&argv(&["pwd"]), Some(&dir)).unwrap();
        assert_eq!(Path::new(out.stdout.trim()).canonicalize().unwrap(), dir);
    }

    #[test]
    fn system_runner_reports_missing_programs() {
        let runner = SystemRunner::new(String::new());
        assert!(runner.run(&argv(&["/no/such/program"]), None).is_err());
    }

    #[test]
    fn fake_runner_matches_prefix_and_records_calls() {
        let fake = FakeRunner::new();
        fake.on(&["docker", "ps"], Output::ok("rows"));
        let out = fake.run(&argv(&["docker", "ps", "-a"]), Some(Path::new("/x"))).unwrap();
        assert_eq!(out.stdout, "rows");
        assert_eq!(fake.calls()[0].cwd.as_deref(), Some(Path::new("/x")));
        assert!(fake.run(&argv(&["ddev", "list"]), None).is_err());
        assert_eq!(fake.calls_starting_with(&["docker"]).len(), 1);
    }

    #[test]
    fn fake_runner_once_rules_are_used_up() {
        let fake = FakeRunner::new();
        fake.once(&["docker"], Output::ok("first")).on(&["docker"], Output::ok("later"));
        assert_eq!(fake.run(&argv(&["docker"]), None).unwrap().stdout, "first");
        assert_eq!(fake.run(&argv(&["docker"]), None).unwrap().stdout, "later");
    }
}
```

`src/bins.rs` (tests only for now):

```rust
//! Finding ddev and docker when Herdr's server started with a bare PATH.

#[cfg(test)]
mod tests {
    use super::*;

    fn exists_in(paths: &'static [&'static str]) -> impl Fn(&Path) -> bool {
        move |path| paths.iter().any(|p| Path::new(p) == path)
    }

    #[test]
    fn configured_command_wins() {
        let configured: Vec<String> =
            ["distrobox", "enter", "--", "ddev"].iter().map(|s| s.to_string()).collect();
        let found = resolve(
            Some(configured.as_slice()),
            "ddev",
            DDEV_CANDIDATES,
            "/usr/bin",
            "/home/u",
            &|_| true,
        );
        assert_eq!(found, Some(configured));
    }

    #[test]
    fn empty_configured_command_is_ignored() {
        let empty: Vec<String> = Vec::new();
        let is_exec = exists_in(&["/p/ddev"]);
        let found = resolve(Some(empty.as_slice()), "ddev", DDEV_CANDIDATES, "/p", "/home/u", &is_exec);
        assert_eq!(found, Some(vec!["/p/ddev".to_string()]));
    }

    #[test]
    fn path_is_searched_before_candidates() {
        let is_exec = exists_in(&["/custom/bin/ddev", "/usr/bin/ddev"]);
        let found =
            resolve(None, "ddev", DDEV_CANDIDATES, "/nothing:/custom/bin", "/home/u", &is_exec);
        assert_eq!(found, Some(vec!["/custom/bin/ddev".to_string()]));
    }

    #[test]
    fn candidates_expand_home() {
        let is_exec = exists_in(&["/home/u/.orbstack/bin/docker"]);
        let found = resolve(None, "docker", DOCKER_CANDIDATES, "", "/home/u", &is_exec);
        assert_eq!(found, Some(vec!["/home/u/.orbstack/bin/docker".to_string()]));
    }

    #[test]
    fn nothing_found_gives_none() {
        assert_eq!(resolve(None, "ddev", DDEV_CANDIDATES, "/x", "/home/u", &|_| false), None);
    }

    #[test]
    fn widened_path_puts_binary_folders_first_without_duplicates() {
        let ddev = vec!["/opt/tools/ddev".to_string()];
        let wrapper = vec!["distrobox".to_string(), "enter".to_string()];
        let path = widened_path(&[&ddev, &wrapper], "/usr/bin:/extra", "/home/u");
        let dirs: Vec<&str> = path.split(':').collect();
        assert_eq!(dirs[0], "/opt/tools");
        assert!(dirs.contains(&"/extra"));
        assert!(dirs.contains(&"/home/u/.orbstack/bin"));
        assert_eq!(dirs.iter().filter(|d| **d == "/usr/bin").count(), 1);
    }

    #[test]
    fn is_executable_file_checks_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tool");
        std::fs::write(&file, "#!/bin/sh\n").unwrap();
        assert!(!is_executable_file(&file));
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(is_executable_file(&file));
        assert!(!is_executable_file(dir.path()));
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test`
Expected: compile errors: `SystemRunner`, `Output`, `resolve`, `widened_path` not found.

- [ ] **Step 4: Implement `runner.rs`**

Add above the tests in `src/runner.rs`:

```rust
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

/// A finished command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    /// A successful run printing `stdout`.
    pub fn ok(stdout: &str) -> Output {
        Output { success: true, stdout: stdout.to_string(), stderr: String::new() }
    }

    /// A failed run printing `stderr`.
    pub fn fail(stderr: &str) -> Output {
        Output { success: false, stdout: String::new(), stderr: stderr.to_string() }
    }
}

pub trait Runner {
    /// Run `argv` (program first) in `cwd` and wait for it to finish.
    fn run(&self, argv: &[String], cwd: Option<&Path>) -> Result<Output>;
}

/// Runs real processes with a fixed PATH.
pub struct SystemRunner {
    path: String,
}

impl SystemRunner {
    pub fn new(path: String) -> SystemRunner {
        SystemRunner { path }
    }
}

impl Runner for SystemRunner {
    fn run(&self, argv: &[String], cwd: Option<&Path>) -> Result<Output> {
        let (program, args) = argv.split_first().context("empty command")?;
        let mut command = Command::new(program);
        command.args(args).env("PATH", &self.path);
        if let Some(dir) = cwd {
            command.current_dir(dir);
        }
        let output = command.output().with_context(|| format!("could not run {program}"))?;
        Ok(Output {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// One call recorded by the test runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
}

#[cfg(test)]
pub mod fake {
    use std::cell::RefCell;
    use std::path::Path;

    use anyhow::{Result, bail};

    use super::{Call, Output, Runner};

    struct Rule {
        prefix: Vec<String>,
        output: Output,
        once: bool,
    }

    /// Answers each call with the first rule whose argv prefix matches and records every call.
    /// A `once` rule is used up by its first match; register `once` rules before `on` rules
    /// for the same prefix so they are found first.
    #[derive(Default)]
    pub struct FakeRunner {
        rules: RefCell<Vec<Rule>>,
        calls: RefCell<Vec<Call>>,
    }

    impl FakeRunner {
        pub fn new() -> FakeRunner {
            FakeRunner::default()
        }

        pub fn on(&self, prefix: &[&str], output: Output) -> &FakeRunner {
            self.push(prefix, output, false)
        }

        pub fn once(&self, prefix: &[&str], output: Output) -> &FakeRunner {
            self.push(prefix, output, true)
        }

        fn push(&self, prefix: &[&str], output: Output, once: bool) -> &FakeRunner {
            let prefix = prefix.iter().map(|s| s.to_string()).collect();
            self.rules.borrow_mut().push(Rule { prefix, output, once });
            self
        }

        pub fn calls(&self) -> Vec<Call> {
            self.calls.borrow().clone()
        }

        pub fn calls_starting_with(&self, prefix: &[&str]) -> Vec<Call> {
            self.calls().into_iter().filter(|call| starts_with(&call.argv, prefix)).collect()
        }
    }

    fn starts_with(argv: &[String], prefix: &[impl AsRef<str>]) -> bool {
        argv.len() >= prefix.len() && argv.iter().zip(prefix).all(|(a, p)| a == p.as_ref())
    }

    impl Runner for FakeRunner {
        fn run(&self, argv: &[String], cwd: Option<&Path>) -> Result<Output> {
            let call = Call { argv: argv.to_vec(), cwd: cwd.map(Path::to_path_buf) };
            self.calls.borrow_mut().push(call);
            let mut rules = self.rules.borrow_mut();
            let Some(index) = rules.iter().position(|rule| starts_with(argv, &rule.prefix)) else {
                bail!("FakeRunner: no rule for {argv:?}");
            };
            let output = rules[index].output.clone();
            if rules[index].once {
                rules.remove(index);
            }
            Ok(output)
        }
    }
}
```

- [ ] **Step 5: Implement `bins.rs`**

Add above the tests in `src/bins.rs`:

```rust
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// ddev install locations checked after PATH (ported from norns-companion `util.rs`).
pub const DDEV_CANDIDATES: &[&str] = &[
    "/opt/homebrew/bin/ddev",
    "/usr/local/bin/ddev",
    "/usr/bin/ddev",
    "/home/linuxbrew/.linuxbrew/bin/ddev",
];

/// docker install locations checked after PATH: Homebrew, Docker Desktop, OrbStack, Rancher.
pub const DOCKER_CANDIDATES: &[&str] = &[
    "/opt/homebrew/bin/docker",
    "/usr/local/bin/docker",
    "/usr/bin/docker",
    "/home/linuxbrew/.linuxbrew/bin/docker",
    "~/.orbstack/bin/docker",
    "~/.docker/bin/docker",
    "~/.rd/bin/docker",
    "/Applications/Docker.app/Contents/Resources/bin/docker",
];

/// Folders added to every spawned command's PATH after the binaries' own folders.
const STANDARD_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
    "/bin",
    "/home/linuxbrew/.linuxbrew/bin",
    "~/.orbstack/bin",
    "~/.docker/bin",
];

/// Replace a leading `~/` with `home`.
pub fn expand_home(path: &str, home: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => Path::new(home).join(rest),
        None => PathBuf::from(path),
    }
}

/// The command to run for `name`: a configured argv wins, then `name` on `path_var`, then the
/// first existing candidate.
pub fn resolve(
    configured: Option<&[String]>,
    name: &str,
    candidates: &[&str],
    path_var: &str,
    home: &str,
    is_executable: &dyn Fn(&Path) -> bool,
) -> Option<Vec<String>> {
    if let Some(argv) = configured.filter(|argv| !argv.is_empty()) {
        return Some(argv.to_vec());
    }
    let on_path = std::env::split_paths(path_var)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(name));
    let fixed = candidates.iter().map(|candidate| expand_home(candidate, home));
    on_path
        .chain(fixed)
        .find(|path| is_executable(path))
        .map(|path| vec![path.to_string_lossy().into_owned()])
}

/// PATH for spawned commands: the resolved binaries' folders, the standard folders, then the
/// inherited PATH, without duplicates. ddev needs mkcert and docker next to it.
pub fn widened_path(binaries: &[&[String]], inherited: &str, home: &str) -> String {
    let own_dirs = binaries
        .iter()
        .filter_map(|argv| argv.first())
        .filter_map(|program| Path::new(program).parent().map(Path::to_path_buf))
        .filter(|dir| !dir.as_os_str().is_empty());
    let standard = STANDARD_DIRS.iter().map(|dir| expand_home(dir, home));
    let inherited_dirs =
        std::env::split_paths(inherited).filter(|dir| !dir.as_os_str().is_empty());
    let mut seen = HashSet::new();
    let dirs: Vec<PathBuf> = own_dirs
        .chain(standard)
        .chain(inherited_dirs)
        .filter(|dir| seen.insert(dir.clone()))
        .collect();
    std::env::join_paths(dirs)
        .map(|joined| joined.to_string_lossy().into_owned())
        .unwrap_or_else(|_| inherited.to_string())
}

/// True for an existing file with an execute bit.
pub fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: all tests pass (12 in `runner` + `bins`, 2 in `main`).

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock src
git commit -m "feat: add process runner and ddev/docker binary lookup"
```

---

### Task 3: Plugin config file

**Files:**
- Create: `src/config.rs`
- Modify: `src/lib.rs`, `Cargo.toml`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `config::OpenMode { Auto, Browser, Clipboard }` (Copy, Default = Auto)
  - `config::Config { ddev_command: Option<Vec<String>>, docker_command: Option<Vec<String>>,
    poll_interval_secs: u64, open_mode: OpenMode }` with `Default`
  - `config::FILE_NAME = "config.toml"`
  - `config::parse(&str) -> Result<Config, String>`
  - `config::load(dir: &Path) -> (Config, Option<String>)`

- [ ] **Step 1: Add dependencies and the module**

In `Cargo.toml` `[dependencies]` add:

```toml
serde = { version = "1", features = ["derive"] }
toml = "1"
```

In `src/lib.rs` add `pub mod config;`.

- [ ] **Step 2: Write the failing tests**

`src/config.rs`:

```rust
//! The optional plugin config file in Herdr's per-plugin config folder.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        assert_eq!(parse("").unwrap(), Config::default());
        assert_eq!(Config::default().poll_interval_secs, 5);
        assert_eq!(Config::default().open_mode, OpenMode::Auto);
    }

    #[test]
    fn all_keys_parse() {
        let config = parse(
            r#"
            ddev_command = ["distrobox", "enter", "-n", "dev", "--", "ddev"]
            docker_command = ["docker"]
            poll_interval_secs = 15
            open_mode = "clipboard"
            "#,
        )
        .unwrap();
        assert_eq!(config.ddev_command.unwrap()[0], "distrobox");
        assert_eq!(config.docker_command.unwrap(), ["docker"]);
        assert_eq!(config.poll_interval_secs, 15);
        assert_eq!(config.open_mode, OpenMode::Clipboard);
    }

    #[test]
    fn unknown_key_is_an_error() {
        assert!(parse("pol_interval = 3").is_err());
    }

    #[test]
    fn bad_open_mode_is_an_error() {
        assert!(parse(r#"open_mode = "firefox""#).is_err());
    }

    #[test]
    fn zero_interval_is_an_error() {
        assert!(parse("poll_interval_secs = 0").unwrap_err().contains("at least 1"));
    }

    #[test]
    fn missing_file_gives_defaults_without_error() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()), (Config::default(), None));
    }

    #[test]
    fn invalid_file_gives_defaults_and_names_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE_NAME), "open_mode = 3").unwrap();
        let (config, error) = load(dir.path());
        assert_eq!(config, Config::default());
        assert!(error.unwrap().contains("config.toml"));
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test config`
Expected: compile errors: `parse`, `Config`, `load` not found.

- [ ] **Step 4: Implement**

Add above the tests in `src/config.rs`:

```rust
use std::io::ErrorKind;
use std::path::Path;

use serde::Deserialize;

pub const FILE_NAME: &str = "config.toml";

/// How `open` shows a site URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenMode {
    /// Browser with a local desktop and no SSH session, clipboard otherwise.
    #[default]
    Auto,
    Browser,
    Clipboard,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub ddev_command: Option<Vec<String>>,
    pub docker_command: Option<Vec<String>>,
    pub poll_interval_secs: u64,
    pub open_mode: OpenMode,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            ddev_command: None,
            docker_command: None,
            poll_interval_secs: 5,
            open_mode: OpenMode::Auto,
        }
    }
}

pub fn parse(text: &str) -> Result<Config, String> {
    let config: Config = toml::from_str(text).map_err(|err| err.message().to_string())?;
    if config.poll_interval_secs == 0 {
        return Err("poll_interval_secs must be at least 1".to_string());
    }
    Ok(config)
}

/// Load `<dir>/config.toml`. A missing file gives defaults; an unreadable or invalid one gives
/// defaults plus the error text, so the caller can report it once.
pub fn load(dir: &Path) -> (Config, Option<String>) {
    let path = dir.join(FILE_NAME);
    match std::fs::read_to_string(&path) {
        Ok(text) => match parse(&text) {
            Ok(config) => (config, None),
            Err(err) => (Config::default(), Some(format!("{}: {err}", path.display()))),
        },
        Err(err) if err.kind() == ErrorKind::NotFound => (Config::default(), None),
        Err(err) => (Config::default(), Some(format!("{}: {err}", path.display()))),
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src
git commit -m "feat: read optional plugin config"
```

---

### Task 4: Docker query

**Files:**
- Create: `src/docker.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `runner::{Runner, Output}`, test-only `runner::fake::FakeRunner`.
- Produces:
  - `docker::State { Running, Paused, Stopped }` (Copy, Eq, Hash)
  - `docker::Project { name: String, root: PathBuf, state: State }`
  - `docker::PS_FORMAT: &str`, `docker::ps_argv(&[String]) -> Vec<String>`
  - `docker::parse_ps(&str) -> Vec<Project>` (sorted by name)
  - `docker::query(&dyn Runner, docker: &[String]) -> Result<Vec<Project>>`

- [ ] **Step 1: Write the failing tests**

In `src/lib.rs` add `pub mod docker;`. Create `src/docker.rs`:

```rust
//! ddev projects as Docker sees them: one `docker ps` per tick (~0.04s against ~1.3s for
//! `ddev list`).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::Output;
    use crate::runner::fake::FakeRunner;

    const PS: &str = concat!(
        "shop\t/home/dev/sites/shop\trunning\tweb\n",
        "shop\t/home/dev/sites/shop\trunning\tdb\n",
        "blog\t/home/dev/My Sites/blog\texited\tweb\n",
        "blog\t/home/dev/My Sites/blog\texited\tdb\n",
        "half\t/home/dev/half\texited\tweb\n",
        "half\t/home/dev/half\trunning\tdb\n",
        "\t\trunning\tddev-router\n",
        "\t\trunning\tddev-ssh-agent\n",
        "garbage line\n",
    );

    #[test]
    fn groups_rows_into_projects_sorted_by_name() {
        let projects = parse_ps(PS);
        let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["blog", "half", "shop"]);
        assert_eq!(projects[2].state, State::Running);
        assert_eq!(projects[2].root, PathBuf::from("/home/dev/sites/shop"));
    }

    #[test]
    fn exited_containers_mean_paused() {
        assert_eq!(parse_ps(PS)[0].state, State::Paused);
    }

    #[test]
    fn web_down_with_db_up_is_paused() {
        assert_eq!(parse_ps(PS)[1].state, State::Paused);
    }

    #[test]
    fn roots_with_spaces_survive() {
        assert_eq!(parse_ps(PS)[0].root, PathBuf::from("/home/dev/My Sites/blog"));
    }

    #[test]
    fn query_runs_docker_ps_with_the_label_filter() {
        let fake = FakeRunner::new();
        fake.on(&["docker", "ps"], Output::ok(PS));
        let projects = query(&fake, &["docker".to_string()]).unwrap();
        assert_eq!(projects.len(), 3);
        let call = &fake.calls()[0].argv;
        assert!(call.contains(&"label=com.ddev.site-name".to_string()));
        assert_eq!(call.last().unwrap(), PS_FORMAT);
    }

    #[test]
    fn query_reports_docker_failure() {
        let fake = FakeRunner::new();
        fake.on(&["docker"], Output::fail("Cannot connect to the Docker daemon"));
        let err = query(&fake, &["docker".to_string()]).unwrap_err();
        assert!(err.to_string().contains("Cannot connect"));
    }

    #[test]
    fn wrapped_docker_command_keeps_the_wrapper_first() {
        let wrapper: Vec<String> =
            ["distrobox", "enter", "--", "docker"].iter().map(|s| s.to_string()).collect();
        let argv = ps_argv(&wrapper);
        assert_eq!(&argv[..5], ["distrobox", "enter", "--", "docker", "ps"]);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test docker`
Expected: compile errors: `parse_ps`, `State`, `query`, `ps_argv` not found.

- [ ] **Step 3: Implement**

Add above the tests in `src/docker.rs`:

```rust
use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Result, bail};

use crate::runner::Runner;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    Running,
    Paused,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    pub root: PathBuf,
    pub state: State,
}

/// Docker converts the literal `\t` into a tab.
pub const PS_FORMAT: &str = concat!(
    r#"{{.Label "com.ddev.site-name"}}\t{{.Label "com.ddev.approot"}}\t"#,
    r#"{{.State}}\t{{.Label "com.docker.compose.service"}}"#
);

/// `docker ps` listing every ddev container with its project, root, state and service.
pub fn ps_argv(docker: &[String]) -> Vec<String> {
    let mut argv = docker.to_vec();
    for arg in ["ps", "-a", "--filter", "label=com.ddev.site-name", "--format", PS_FORMAT] {
        argv.push(arg.to_string());
    }
    argv
}

/// Group container rows into projects: running when the project's `web` container runs,
/// paused when its containers exist but `web` does not run. Rows without a site name or root
/// (ddev-router, ddev-ssh-agent) and malformed rows are skipped.
pub fn parse_ps(stdout: &str) -> Vec<Project> {
    let mut projects: BTreeMap<String, (PathBuf, bool)> = BTreeMap::new();
    for line in stdout.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        let &[name, root, state, service] = fields.as_slice() else {
            continue;
        };
        if name.is_empty() || root.is_empty() {
            continue;
        }
        let entry =
            projects.entry(name.to_string()).or_insert_with(|| (PathBuf::from(root), false));
        if service == "web" && state == "running" {
            entry.1 = true;
        }
    }
    projects
        .into_iter()
        .map(|(name, (root, web_running))| Project {
            name,
            root,
            state: if web_running { State::Running } else { State::Paused },
        })
        .collect()
}

/// Every ddev project that currently has containers.
pub fn query(runner: &dyn Runner, docker: &[String]) -> Result<Vec<Project>> {
    let out = runner.run(&ps_argv(docker), None)?;
    if !out.success {
        bail!("docker ps failed: {}", out.stderr.trim());
    }
    Ok(parse_ps(&out.stdout))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: all pass.

- [ ] **Step 5: Check the format against real Docker (read-only)**

Run: `docker ps -a --filter label=com.ddev.site-name --format '{{.Label "com.ddev.site-name"}}\t{{.Label "com.ddev.approot"}}\t{{.State}}\t{{.Label "com.docker.compose.service"}}' | head -4 | od -c | head -3`
Expected: tab characters (`\t`) between fields, `web`/`db` as services. Skip if Docker is
not running; note that in the task report.

- [ ] **Step 6: Commit**

```bash
git add src
git commit -m "feat: read ddev project state from docker labels"
```

---

### Task 5: Project detection

**Files:**
- Create: `src/project.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `docker::{Project, State}`.
- Produces:
  - `project::canonical(&Path) -> PathBuf`
  - `project::canonical_projects(Vec<Project>) -> Vec<Project>`
  - `project::best_match<'a>(dir: &Path, projects: &'a [Project]) -> Option<&'a Project>`
  - `project::find_config_root(&Path) -> Option<PathBuf>`
  - `project::project_name(root: &Path) -> String`
  - `project::StoppedCache::new(ttl: Duration)`,
    `StoppedCache::lookup(&mut self, dir: &Path, now: Instant) -> Option<Project>`
  - `project::resolve(dir: &Path, docker: &[Project], cache: &mut StoppedCache, now: Instant)
    -> Option<Project>` (expects `docker` already canonical)

- [ ] **Step 1: Write the failing tests**

In `src/lib.rs` add `pub mod project;`. Create `src/project.rs`:

```rust
//! Which ddev project a folder belongs to.

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_project(root: &Path, config: &str) {
        fs::create_dir_all(root.join(".ddev")).unwrap();
        fs::write(root.join(".ddev/config.yaml"), config).unwrap();
    }

    fn running(name: &str, root: &Path) -> Project {
        Project { name: name.to_string(), root: canonical(root), state: State::Running }
    }

    fn cache() -> StoppedCache {
        StoppedCache::new(Duration::from_secs(60))
    }

    #[test]
    fn longest_root_wins_for_nested_projects() {
        let parent = Project { name: "parent".into(), root: "/w/site".into(), state: State::Running };
        let child = Project { name: "child".into(), root: "/w/site/sub".into(), state: State::Running };
        let projects = [parent, child];
        assert_eq!(best_match(Path::new("/w/site/sub/web"), &projects).unwrap().name, "child");
        assert_eq!(best_match(Path::new("/w/site/other"), &projects).unwrap().name, "parent");
        assert!(best_match(Path::new("/w/sitex"), &projects).is_none());
    }

    #[test]
    fn canonical_strips_trailing_slash_for_missing_paths() {
        assert_eq!(canonical(Path::new("/no/such/dir/")), PathBuf::from("/no/such/dir"));
    }

    #[test]
    fn canonical_resolves_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(canonical(&link), canonical(&real));
    }

    #[test]
    fn missing_subfolder_still_matches_a_canonical_root() {
        // On macOS the temp dir lives under /var, which is a symlink to /private/var.
        let tmp = tempfile::tempdir().unwrap();
        let docker = [running("shop", tmp.path())];
        let gone = tmp.path().join("web/not-created-yet");
        let found = resolve(&gone, &docker, &mut cache(), Instant::now());
        assert_eq!(found.unwrap().name, "shop");
    }

    #[test]
    fn name_comes_from_config_yaml() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path(), "name: shop\ntype: drupal11\n");
        assert_eq!(project_name(tmp.path()), "shop");
    }

    #[test]
    fn later_config_files_override_the_name() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path(), "name: shop\n");
        fs::write(tmp.path().join(".ddev/config.local.yaml"), "name: \"shop-local\" # mine\n")
            .unwrap();
        assert_eq!(project_name(tmp.path()), "shop-local");
    }

    #[test]
    fn nested_name_keys_are_ignored_and_the_folder_name_is_the_default() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("my-site");
        make_project(&root, "web_environment:\n  name: nope\n");
        assert_eq!(project_name(&root), "my-site");
    }

    #[test]
    fn config_root_is_found_walking_up() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path(), "name: shop\n");
        let deep = tmp.path().join("web/themes/custom");
        fs::create_dir_all(&deep).unwrap();
        assert_eq!(find_config_root(&deep).unwrap(), tmp.path());
    }

    #[test]
    fn stopped_project_is_found_from_config_and_cached() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path(), "name: shop\n");
        let mut cache = cache();
        let now = Instant::now();
        let found = resolve(tmp.path(), &[], &mut cache, now).unwrap();
        assert_eq!((found.name.as_str(), found.state), ("shop", State::Stopped));
        fs::remove_dir_all(tmp.path().join(".ddev")).unwrap();
        assert!(resolve(tmp.path(), &[], &mut cache, now + Duration::from_secs(30)).is_some());
        assert!(resolve(tmp.path(), &[], &mut cache, now + Duration::from_secs(61)).is_none());
    }

    #[test]
    fn docker_state_wins_for_the_same_root() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path(), "name: shop\n");
        fs::create_dir(tmp.path().join("web")).unwrap();
        let docker = [running("shop", tmp.path())];
        let found = resolve(&tmp.path().join("web"), &docker, &mut cache(), Instant::now());
        assert_eq!(found.unwrap().state, State::Running);
    }

    #[test]
    fn nested_stopped_project_beats_a_running_parent() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path(), "name: parent\n");
        let child = tmp.path().join("tools/child");
        make_project(&child, "name: child\n");
        let docker = [running("parent", tmp.path())];
        let found = resolve(&child, &docker, &mut cache(), Instant::now()).unwrap();
        assert_eq!((found.name.as_str(), found.state), ("child", State::Stopped));
    }

    #[test]
    fn folder_outside_any_project_resolves_to_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(resolve(tmp.path(), &[], &mut cache(), Instant::now()).is_none());
    }

    #[test]
    fn canonical_projects_canonicalizes_roots() {
        let tmp = tempfile::tempdir().unwrap();
        let raw = Project { name: "shop".into(), root: tmp.path().join("./"), state: State::Running };
        assert_eq!(canonical_projects(vec![raw])[0].root, canonical(tmp.path()));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test project`
Expected: compile errors: `canonical`, `resolve`, `StoppedCache`, `project_name` not found.

- [ ] **Step 3: Implement**

Add above the tests in `src/project.rs`:

```rust
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::docker::{Project, State};

/// The real path: symlinks resolved (macOS `/tmp` is `/private/tmp`) and trailing slashes
/// gone. For a path that does not exist, the longest existing ancestor is resolved and the
/// rest appended, so a missing subfolder still compares against canonical roots.
pub fn canonical(path: &Path) -> PathBuf {
    let clean: PathBuf = path.components().collect();
    for ancestor in clean.ancestors() {
        if let Ok(real) = std::fs::canonicalize(ancestor) {
            let rest = clean.strip_prefix(ancestor).unwrap_or(Path::new(""));
            return if rest.as_os_str().is_empty() { real } else { real.join(rest) };
        }
    }
    clean
}

/// Docker's projects with canonical roots, ready for matching.
pub fn canonical_projects(projects: Vec<Project>) -> Vec<Project> {
    projects.into_iter().map(|p| Project { root: canonical(&p.root), ..p }).collect()
}

/// The project whose root is `dir` or contains it; the longest root wins, so a nested project
/// beats its parent (norns-companion `matchProject()`). Paths must already be canonical.
pub fn best_match<'a>(dir: &Path, projects: &'a [Project]) -> Option<&'a Project> {
    projects
        .iter()
        .filter(|project| dir.starts_with(&project.root))
        .max_by_key(|project| project.root.components().count())
}

/// The nearest folder at or above `dir` that has `.ddev/config.yaml`.
pub fn find_config_root(dir: &Path) -> Option<PathBuf> {
    dir.ancestors().find(|d| d.join(".ddev").join("config.yaml").is_file()).map(Path::to_path_buf)
}

/// ddev's project name: the last top-level `name:` in `config.yaml`, then in `config.*.yaml`
/// in lexical order (ddev's merge order), else the folder name (ddev's default).
pub fn project_name(root: &Path) -> String {
    let ddev = root.join(".ddev");
    let mut overrides: Vec<PathBuf> = std::fs::read_dir(&ddev)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name().and_then(|name| name.to_str()).is_some_and(|name| {
                name.starts_with("config.") && name.ends_with(".yaml") && name != "config.yaml"
            })
        })
        .collect();
    overrides.sort();
    let mut name = None;
    for file in std::iter::once(ddev.join("config.yaml")).chain(overrides) {
        if let Some(found) = std::fs::read_to_string(&file).ok().and_then(|t| name_line(&t)) {
            name = Some(found);
        }
    }
    name.unwrap_or_else(|| {
        root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    })
}

/// The value of the last top-level `name:` line, without quotes or trailing comment.
fn name_line(text: &str) -> Option<String> {
    text.lines()
        .filter_map(|line| line.strip_prefix("name:"))
        .map(|value| {
            let value = value.split('#').next().unwrap_or("").trim();
            value.trim_matches(|c| c == '"' || c == '\'').to_string()
        })
        .filter(|value| !value.is_empty())
        .last()
}

/// Stopped projects found by walking up to `.ddev/config.yaml`, cached per folder.
pub struct StoppedCache {
    ttl: Duration,
    entries: HashMap<PathBuf, (Instant, Option<Project>)>,
}

impl StoppedCache {
    pub fn new(ttl: Duration) -> StoppedCache {
        StoppedCache { ttl, entries: HashMap::new() }
    }

    /// The stopped project owning `dir` (canonical), from cache while younger than the TTL.
    pub fn lookup(&mut self, dir: &Path, now: Instant) -> Option<Project> {
        if let Some((at, found)) = self.entries.get(dir) {
            if now.duration_since(*at) < self.ttl {
                return found.clone();
            }
        }
        let found = find_config_root(dir).map(|root| Project {
            name: project_name(&root),
            root: canonical(&root),
            state: State::Stopped,
        });
        self.entries.insert(dir.to_path_buf(), (now, found.clone()));
        found
    }
}

/// The project owning `dir`: the longest root wins, whether it comes from Docker (running or
/// paused) or from `.ddev/config.yaml` (stopped). On equal roots Docker's state wins.
pub fn resolve(
    dir: &Path,
    docker: &[Project],
    cache: &mut StoppedCache,
    now: Instant,
) -> Option<Project> {
    let dir = canonical(dir);
    let from_docker = best_match(&dir, docker);
    let from_config = cache.lookup(&dir, now);
    match (from_docker, from_config) {
        (Some(d), Some(c)) if c.root.components().count() > d.root.components().count() => {
            Some(c)
        }
        (Some(d), _) => Some(d.clone()),
        (None, c) => c,
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: all pass. If clippy asks to collapse the nested `if let` in `lookup`, rewrite it as
a let chain (`if let Some((at, found)) = self.entries.get(dir) && now.duration_since(*at) <
self.ttl { ... }`).

- [ ] **Step 5: Commit**

```bash
git add src
git commit -m "feat: match folders to running and stopped ddev projects"
```

---

### Task 6: Herdr CLI wrapper

**Files:**
- Create: `src/herdr.rs`, `tests/fixtures/pane-list.json`, `tests/fixtures/pane-get.json`,
  `tests/fixtures/workspace-list.json`
- Modify: `src/lib.rs`, `Cargo.toml`

**Interfaces:**
- Consumes: `runner::Runner`.
- Produces:
  - `herdr::SOURCE = "danjuls.ddev"`, `herdr::TOKEN = "ddev"`, `herdr::PLUGIN_ID = "danjuls.ddev"`
  - `herdr::Pane { pane_id, workspace_id: String, focused: bool, cwd: Option<String>,
    foreground_cwd: Option<String> }`, `Pane::dir(&self) -> Option<&str>`
  - `herdr::Workspace { workspace_id: String, label: String, focused: bool }`
  - `herdr::Sound { None, Done, Request }`
  - `herdr::parse_pane_list`, `parse_pane`, `parse_workspace_list` (`&str -> Result<...>`)
  - `herdr::Herdr<'a>::new(runner: &'a dyn Runner, bin: &str)` with methods
    `panes() -> Result<Vec<Pane>>`, `pane(&str) -> Result<Pane>`, `current_pane()`,
    `workspaces()`, `set_badge(ws: &str, value: &str, seq: u64, ttl_ms: u64) -> Result<()>`,
    `clear_badge(ws: &str, seq: u64)`, `notify(body: &str, sound: Sound)`,
    `focus_workspace(&str)`, `create_workspace(cwd: &Path, label: &str)`,
    `open_pane(entrypoint: &str, env: &[(&str, &str)])`, `config_check() -> Result<bool>`,
    `reload_config()`

- [ ] **Step 1: Add the dependency, module and fixtures**

In `Cargo.toml` `[dependencies]` add `serde_json = "1"`. In `src/lib.rs` add `pub mod herdr;`.

These fixtures copy the shape of real Herdr 0.9.1 output (paths replaced).

`tests/fixtures/pane-list.json`:

```json
{"id":"cli:pane:list","result":{"panes":[
{"agent":"claude","agent_session":{"agent":"claude","kind":"id","source":"herdr:claude","value":"35532a69"},"agent_status":"working","cwd":"/home/dev/sites/shop","focused":true,"foreground_cwd":"/home/dev/sites/shop/web","pane_id":"w1:p1","revision":12,"scroll":{"max_offset_from_bottom":0,"offset_from_bottom":0,"viewport_rows":50},"tab_id":"w1:t1","terminal_id":"term_65c52cf1","terminal_title":"Working","terminal_title_stripped":"Working","workspace_id":"w1"},
{"agent_status":"unknown","cwd":"/home/dev/sites/shop","focused":false,"foreground_cwd":null,"pane_id":"w1:p2","revision":3,"tab_id":"w1:t2","terminal_id":"term_2","workspace_id":"w1"},
{"agent_status":"unknown","cwd":"/home/dev/notes","focused":false,"pane_id":"w2:p1","revision":1,"tab_id":"w2:t1","terminal_id":"term_3","workspace_id":"w2"}
]}}
```

`tests/fixtures/pane-get.json`:

```json
{"id":"cli:pane:get","result":{"pane":{"agent_status":"unknown","cwd":"/home/dev/sites/shop","focused":true,"foreground_cwd":"/home/dev/sites/shop","pane_id":"w1:p1","revision":4,"tab_id":"w1:t1","terminal_id":"term_1","workspace_id":"w1"},"type":"pane_info"}}
```

`tests/fixtures/workspace-list.json`:

```json
{"id":"cli:workspace:list","result":{"workspaces":[
{"active_tab_id":"w1:t1","agent_status":"working","focused":true,"label":"shop","number":1,"pane_count":2,"tab_count":2,"workspace_id":"w1"},
{"active_tab_id":"w2:t1","agent_status":"unknown","focused":false,"label":"notes","number":2,"pane_count":1,"tab_count":1,"workspace_id":"w2"}
]}}
```

- [ ] **Step 2: Write the failing tests**

`src/herdr.rs`:

```rust
//! Typed wrappers over the herdr CLI calls this plugin makes.

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
        Herdr::new(&fake, "herdr").set_badge("w1", "● ddev", 1_700_000_000_000, 20_000).unwrap();
        assert_eq!(
            fake.calls()[0].argv,
            [
                "herdr", "workspace", "report-metadata", "w1", "--source", "danjuls.ddev",
                "--token", "ddev=● ddev", "--seq", "1700000000000", "--ttl-ms", "20000",
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
                "herdr", "workspace", "report-metadata", "w2", "--source", "danjuls.ddev",
                "--clear-token", "ddev", "--seq", "5",
            ]
        );
    }

    #[test]
    fn notify_passes_title_body_and_sound() {
        let fake = herdr_ok();
        Herdr::new(&fake, "herdr").notify("shop started", Sound::Done).unwrap();
        assert_eq!(
            fake.calls()[0].argv,
            ["herdr", "notification", "show", "ddev", "--body", "shop started", "--sound", "done"]
        );
    }

    #[test]
    fn open_pane_passes_env() {
        let fake = herdr_ok();
        let herdr = Herdr::new(&fake, "herdr");
        herdr.open_pane("url", &[("HERDR_DDEV_URL", "https://shop.ddev.site")]).unwrap();
        assert_eq!(
            fake.calls()[0].argv,
            [
                "herdr", "plugin", "pane", "open", "--plugin", "danjuls.ddev", "--entrypoint",
                "url", "--env", "HERDR_DDEV_URL=https://shop.ddev.site",
            ]
        );
    }

    #[test]
    fn create_workspace_focuses_it() {
        let fake = herdr_ok();
        Herdr::new(&fake, "herdr").create_workspace(Path::new("/w/My Shop"), "shop").unwrap();
        assert_eq!(
            fake.calls()[0].argv,
            ["herdr", "workspace", "create", "--cwd", "/w/My Shop", "--label", "shop", "--focus"]
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
        fake.on(&["herdr", "config", "check"], Output::fail("config: issues found"));
        assert!(!Herdr::new(&fake, "herdr").config_check().unwrap());
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test herdr`
Expected: compile errors: `parse_pane_list`, `Herdr`, `Pane`, `Sound` not found.

- [ ] **Step 4: Implement**

Add above the tests in `src/herdr.rs`:

```rust
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
        Herdr { runner, bin: bin.to_string() }
    }

    fn argv(&self, args: &[&str]) -> Vec<String> {
        std::iter::once(self.bin.clone()).chain(args.iter().map(|a| a.to_string())).collect()
    }

    fn call(&self, args: &[&str]) -> Result<String> {
        let out = self.runner.run(&self.argv(args), None)?;
        if !out.success {
            bail!("herdr {} failed: {}", args.join(" "), out.stderr.trim());
        }
        Ok(out.stdout)
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
            "workspace", "report-metadata", workspace_id, "--source", SOURCE, "--token", &token,
            "--seq", &seq, "--ttl-ms", &ttl,
        ];
        self.call(&args).map(drop)
    }

    pub fn clear_badge(&self, workspace_id: &str, seq: u64) -> Result<()> {
        let seq = seq.to_string();
        let args = [
            "workspace", "report-metadata", workspace_id, "--source", SOURCE, "--clear-token",
            TOKEN, "--seq", &seq,
        ];
        self.call(&args).map(drop)
    }

    pub fn notify(&self, body: &str, sound: Sound) -> Result<()> {
        let args = ["notification", "show", "ddev", "--body", body, "--sound", sound.as_arg()];
        self.call(&args).map(drop)
    }

    pub fn focus_workspace(&self, workspace_id: &str) -> Result<()> {
        self.call(&["workspace", "focus", workspace_id]).map(drop)
    }

    pub fn create_workspace(&self, cwd: &Path, label: &str) -> Result<()> {
        let cwd = cwd.to_string_lossy();
        self.call(&["workspace", "create", "--cwd", &cwd, "--label", label, "--focus"]).map(drop)
    }

    pub fn open_pane(&self, entrypoint: &str, env: &[(&str, &str)]) -> Result<()> {
        let pairs: Vec<String> = env.iter().map(|(key, value)| format!("{key}={value}")).collect();
        let mut args = vec!["plugin", "pane", "open", "--plugin", PLUGIN_ID, "--entrypoint"];
        args.push(entrypoint);
        for pair in &pairs {
            args.push("--env");
            args.push(pair);
        }
        self.call(&args).map(drop)
    }

    /// Whether `herdr config check` accepts the current config file.
    pub fn config_check(&self) -> Result<bool> {
        Ok(self.runner.run(&self.argv(&["config", "check"]), None)?.success)
    }

    pub fn reload_config(&self) -> Result<()> {
        self.call(&["server", "reload-config"]).map(drop)
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: all pass.

- [ ] **Step 6: Compare field names with live Herdr (read-only)**

Run: `herdr pane list | jq '.result.panes[0] | keys'` and `herdr pane current | jq '.result | keys'`
Expected: `pane_id`, `workspace_id`, `focused`, `cwd`, `foreground_cwd` present; `pane` and
`type` under `result`. If Herdr is not running, skip and say so in the task report.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock src tests
git commit -m "feat: add typed wrappers for herdr CLI calls"
```

---

### Task 7: Badge text and busy markers

**Files:**
- Create: `src/badge.rs`, `src/busy.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `docker::State`.
- Produces:
  - `busy::Verb { Start, Stop, Restart }` (Copy, serde lowercase) with `parse(&str) ->
    Option<Verb>`, `as_str()`, `progressive()` ("starting"...), `past()` ("started"...)
  - `busy::Marker { project: String, verb: Verb, pid: u32, started_unix: u64 }`
  - `busy::Acquire { Acquired, Busy(Marker) }`, `busy::MAX_AGE_SECS = 900`
  - `busy::marker_path(state_dir: &Path, project: &str) -> PathBuf`
  - `busy::acquire(state_dir: &Path, marker: &Marker, now_unix: u64,
    pid_alive: &dyn Fn(u32) -> bool) -> Result<Acquire>`
  - `busy::read(state_dir, project, now_unix, pid_alive) -> Option<Marker>`
  - `busy::release(state_dir: &Path, project: &str)`
  - `busy::live_all(state_dir, now_unix, pid_alive) -> HashMap<String, Verb>`
  - `busy::pid_alive(pid: u32) -> bool`
  - `badge::RUNNING`, `PAUSED`, `STOPPED`, `BUSY: &str`,
    `badge::text(State, Option<Verb>) -> &'static str`,
    `badge::symbol(State, Option<Verb>) -> &'static str`

- [ ] **Step 1: Write the failing tests**

In `src/lib.rs` add `pub mod badge;` and `pub mod busy;`.

`src/badge.rs`:

```rust
//! The `$ddev` sidebar token text.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_follows_state() {
        assert_eq!(text(State::Running, None), "● ddev");
        assert_eq!(text(State::Paused, None), "◐ ddev");
        assert_eq!(text(State::Stopped, None), "○ ddev");
    }

    #[test]
    fn busy_wins_over_state() {
        assert_eq!(text(State::Running, Some(Verb::Stop)), "◌ ddev…");
    }

    #[test]
    fn symbol_is_the_first_character() {
        assert_eq!(symbol(State::Running, None), "●");
        assert_eq!(symbol(State::Stopped, Some(Verb::Start)), "◌");
    }
}
```

`src/busy.rs`:

```rust
//! Busy markers: one file per project while a start, stop or restart runs.

#[cfg(test)]
mod tests {
    use super::*;

    fn alive(_: u32) -> bool {
        true
    }

    fn dead(_: u32) -> bool {
        false
    }

    fn marker(project: &str, verb: Verb, started_unix: u64) -> Marker {
        Marker { project: project.to_string(), verb, pid: 42, started_unix }
    }

    #[test]
    fn first_acquire_wins_and_the_second_is_busy() {
        let dir = tempfile::tempdir().unwrap();
        let first = acquire(dir.path(), &marker("shop", Verb::Start, 100), 100, &alive).unwrap();
        assert!(matches!(first, Acquire::Acquired));
        match acquire(dir.path(), &marker("shop", Verb::Stop, 101), 101, &alive).unwrap() {
            Acquire::Busy(existing) => assert_eq!(existing.verb, Verb::Start),
            Acquire::Acquired => panic!("second acquire must be refused"),
        }
    }

    #[test]
    fn marker_of_a_dead_process_is_ignored_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        acquire(dir.path(), &marker("shop", Verb::Start, 100), 100, &alive).unwrap();
        assert!(read(dir.path(), "shop", 100, &dead).is_none());
        assert!(!marker_path(dir.path(), "shop").exists());
    }

    #[test]
    fn old_marker_expires() {
        let dir = tempfile::tempdir().unwrap();
        acquire(dir.path(), &marker("shop", Verb::Start, 100), 100, &alive).unwrap();
        assert!(read(dir.path(), "shop", 100 + MAX_AGE_SECS - 1, &alive).is_some());
        assert!(read(dir.path(), "shop", 100 + MAX_AGE_SECS, &alive).is_none());
    }

    #[test]
    fn release_allows_a_new_acquire() {
        let dir = tempfile::tempdir().unwrap();
        acquire(dir.path(), &marker("shop", Verb::Start, 100), 100, &alive).unwrap();
        release(dir.path(), "shop");
        let again = acquire(dir.path(), &marker("shop", Verb::Stop, 101), 101, &alive).unwrap();
        assert!(matches!(again, Acquire::Acquired));
    }

    #[test]
    fn live_all_maps_projects_to_verbs_and_skips_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        acquire(dir.path(), &marker("shop", Verb::Start, 100), 100, &alive).unwrap();
        acquire(dir.path(), &marker("blog", Verb::Restart, 100), 100, &alive).unwrap();
        std::fs::write(dir.path().join("busy/.123.100.tmp"), "junk").unwrap();
        let live = live_all(dir.path(), 100, &alive);
        assert_eq!(live.len(), 2);
        assert_eq!(live["shop"], Verb::Start);
        assert_eq!(live["blog"], Verb::Restart);
    }

    #[test]
    fn strange_project_names_stay_inside_the_busy_folder() {
        let path = marker_path(Path::new("/s"), "../../etc/passwd");
        assert_eq!(path.parent(), Some(Path::new("/s/busy")));
        assert_eq!(path.file_name().unwrap(), "_.._.._etc_passwd.json");
    }

    #[test]
    fn pid_alive_knows_this_process() {
        assert!(pid_alive(std::process::id()));
        assert!(!pid_alive(0));
    }

    #[test]
    fn verbs_round_trip() {
        for verb in [Verb::Start, Verb::Stop, Verb::Restart] {
            assert_eq!(Verb::parse(verb.as_str()), Some(verb));
        }
        assert_eq!(Verb::parse("pause"), None);
        assert_eq!(Verb::Restart.progressive(), "restarting");
        assert_eq!(Verb::Stop.past(), "stopped");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test badge busy`
Expected: compile errors: `text`, `Verb`, `acquire`, `marker_path` not found.

- [ ] **Step 3: Implement `busy.rs`**

Add above the tests in `src/busy.rs`:

```rust
use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// A marker older than this is ignored, in case its process was reused by another program.
pub const MAX_AGE_SECS: u64 = 15 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verb {
    Start,
    Stop,
    Restart,
}

impl Verb {
    pub fn parse(text: &str) -> Option<Verb> {
        match text {
            "start" => Some(Verb::Start),
            "stop" => Some(Verb::Stop),
            "restart" => Some(Verb::Restart),
            _ => None,
        }
    }

    /// The ddev subcommand.
    pub fn as_str(self) -> &'static str {
        match self {
            Verb::Start => "start",
            Verb::Stop => "stop",
            Verb::Restart => "restart",
        }
    }

    pub fn progressive(self) -> &'static str {
        match self {
            Verb::Start => "starting",
            Verb::Stop => "stopping",
            Verb::Restart => "restarting",
        }
    }

    pub fn past(self) -> &'static str {
        match self {
            Verb::Start => "started",
            Verb::Stop => "stopped",
            Verb::Restart => "restarted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    pub project: String,
    pub verb: Verb,
    pub pid: u32,
    pub started_unix: u64,
}

#[derive(Debug)]
pub enum Acquire {
    Acquired,
    Busy(Marker),
}

/// The marker file for a project. The name is sanitized so it cannot leave the busy folder.
pub fn marker_path(state_dir: &Path, project: &str) -> PathBuf {
    let safe: String = project
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' })
        .collect();
    let safe = if safe.is_empty() || safe.starts_with('.') { format!("_{safe}") } else { safe };
    state_dir.join("busy").join(format!("{safe}.json"))
}

fn is_live(marker: &Marker, now_unix: u64, pid_alive: &dyn Fn(u32) -> bool) -> bool {
    now_unix.saturating_sub(marker.started_unix) < MAX_AGE_SECS && pid_alive(marker.pid)
}

/// A live marker at `path`; a stale or unreadable one is removed.
fn read_path(path: &Path, now_unix: u64, pid_alive: &dyn Fn(u32) -> bool) -> Option<Marker> {
    let text = fs::read_to_string(path).ok()?;
    match serde_json::from_str::<Marker>(&text) {
        Ok(marker) if is_live(&marker, now_unix, pid_alive) => Some(marker),
        _ => {
            let _ = fs::remove_file(path);
            None
        }
    }
}

pub fn read(
    state_dir: &Path,
    project: &str,
    now_unix: u64,
    pid_alive: &dyn Fn(u32) -> bool,
) -> Option<Marker> {
    read_path(&marker_path(state_dir, project), now_unix, pid_alive)
}

/// Create the marker unless a live one exists. The complete marker is written to a temp file
/// and hard-linked into place, so racing workers never see a half-written file and only one
/// link can succeed.
pub fn acquire(
    state_dir: &Path,
    marker: &Marker,
    now_unix: u64,
    pid_alive: &dyn Fn(u32) -> bool,
) -> Result<Acquire> {
    let path = marker_path(state_dir, &marker.project);
    if let Some(existing) = read_path(&path, now_unix, pid_alive) {
        return Ok(Acquire::Busy(existing));
    }
    let dir = path.parent().context("marker path has no folder")?;
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.{}.tmp", std::process::id(), marker.started_unix));
    fs::write(&tmp, serde_json::to_string(marker)?)?;
    let linked = fs::hard_link(&tmp, &path);
    let _ = fs::remove_file(&tmp);
    match linked {
        Ok(()) => Ok(Acquire::Acquired),
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {
            let existing = read_path(&path, now_unix, pid_alive).unwrap_or_else(|| marker.clone());
            Ok(Acquire::Busy(existing))
        }
        Err(err) => Err(err.into()),
    }
}

pub fn release(state_dir: &Path, project: &str) {
    let _ = fs::remove_file(marker_path(state_dir, project));
}

/// Every live marker, by project name.
pub fn live_all(
    state_dir: &Path,
    now_unix: u64,
    pid_alive: &dyn Fn(u32) -> bool,
) -> HashMap<String, Verb> {
    let Ok(entries) = fs::read_dir(state_dir.join("busy")) else {
        return HashMap::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter_map(|path| read_path(&path, now_unix, pid_alive))
        .map(|marker| (marker.project, marker.verb))
        .collect()
}

/// Whether a process with this id exists (`kill -0`).
pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
```

- [ ] **Step 4: Implement `badge.rs`**

Add above the tests in `src/badge.rs`:

```rust
use crate::busy::Verb;
use crate::docker::State;

pub const RUNNING: &str = "● ddev";
pub const PAUSED: &str = "◐ ddev";
pub const STOPPED: &str = "○ ddev";
pub const BUSY: &str = "◌ ddev…";

/// The `$ddev` token for a project: busy while an action runs, else its state.
pub fn text(state: State, busy: Option<Verb>) -> &'static str {
    match (busy, state) {
        (Some(_), _) => BUSY,
        (None, State::Running) => RUNNING,
        (None, State::Paused) => PAUSED,
        (None, State::Stopped) => STOPPED,
    }
}

/// Just the symbol, for picker rows.
pub fn symbol(state: State, busy: Option<Verb>) -> &'static str {
    text(state, busy).split(' ').next().unwrap_or("")
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add src
git commit -m "feat: add badge text and busy markers"
```

---

### Task 8: Ticker core

**Files:**
- Create: `src/app.rs`, `src/ticker.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `runner::Runner`, `herdr::{Herdr, Pane}`, `docker`, `project`, `badge`, `busy`,
  `config::OpenMode`.
- Produces:
  - `app::Ctx<'a> { runner: &'a dyn Runner, herdr: Herdr<'a>, ddev: Option<Vec<String>>,
    docker: Option<Vec<String>>, state_dir: PathBuf, config_dir: PathBuf, open_mode: OpenMode,
    interval: Duration, pane_id: Option<String>, spawn: &'a dyn Fn(&[String]) -> Result<()>,
    pid_alive: &'a dyn Fn(u32) -> bool }`
  - `app::unix_ms() -> u64`
  - test-only `app::testing::{ctx(&FakeRunner, &Path, &dyn Fn(&[String]) -> Result<()>) -> Ctx,
    always_alive, no_spawn}`
  - `ticker::ttl(Duration) -> Duration`, `ticker::backoff(Duration, u32) -> Duration`
  - `ticker::workspace_projects(&[Pane], &mut dyn FnMut(&Path) -> Option<Project>)
    -> BTreeMap<String, Project>`
  - `ticker::Report { Set { workspace, value }, Clear { workspace } }`,
    `ticker::Reported { value: String, at: Instant }`,
    `ticker::plan_reports(&BTreeMap<String, String>, &HashMap<String, Reported>, Instant,
    Duration) -> Vec<Report>`
  - `ticker::TickError { Herdr(anyhow::Error), Docker(anyhow::Error) }`
  - `ticker::Ticker::new(&'a Ctx<'a>, docker: Vec<String>)`,
    `Ticker::tick(&mut self, now: Instant, now_unix_ms: u64) -> Result<(), TickError>`

- [ ] **Step 1: Create `app.rs` with the context**

In `src/lib.rs` add `pub mod app;` and `pub mod ticker;`. Create `src/app.rs`:

```rust
//! What every command needs: how to run things, where state lives, which binaries to use.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;

use crate::config::OpenMode;
use crate::herdr::Herdr;
use crate::runner::Runner;

pub struct Ctx<'a> {
    pub runner: &'a dyn Runner,
    pub herdr: Herdr<'a>,
    pub ddev: Option<Vec<String>>,
    pub docker: Option<Vec<String>>,
    pub state_dir: PathBuf,
    pub config_dir: PathBuf,
    pub open_mode: OpenMode,
    pub interval: Duration,
    /// The pane Herdr invoked us from, when it says.
    pub pane_id: Option<String>,
    /// Starts `herdr-ddev <args>` in the background.
    pub spawn: &'a dyn Fn(&[String]) -> Result<()>,
    pub pid_alive: &'a dyn Fn(u32) -> bool,
}

pub fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
pub mod testing {
    use std::path::Path;

    use super::*;
    use crate::runner::fake::FakeRunner;

    pub fn always_alive(_: u32) -> bool {
        true
    }

    pub fn no_spawn(_: &[String]) -> Result<()> {
        Ok(())
    }

    /// A context over a fake runner, with ddev and docker present and state in `state_dir`.
    pub fn ctx<'a>(
        runner: &'a FakeRunner,
        state_dir: &Path,
        spawn: &'a dyn Fn(&[String]) -> Result<()>,
    ) -> Ctx<'a> {
        Ctx {
            runner,
            herdr: Herdr::new(runner, "herdr"),
            ddev: Some(vec!["ddev".to_string()]),
            docker: Some(vec!["docker".to_string()]),
            state_dir: state_dir.to_path_buf(),
            config_dir: state_dir.join("config"),
            open_mode: OpenMode::Browser,
            interval: Duration::from_secs(5),
            pane_id: None,
            spawn,
            pid_alive: &always_alive,
        }
    }
}
```

- [ ] **Step 2: Write the failing ticker tests**

`src/ticker.rs`:

```rust
//! Keeps each workspace's `$ddev` sidebar badge in step with Docker.

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
        Project { name: path.to_string(), root: path.into(), state: State::Running }
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
        let steps: Vec<u64> = [0, 1, 2, 3, 9].iter().map(|n| backoff(base, *n).as_secs()).collect();
        assert_eq!(steps, [5, 10, 20, 30, 30]);
        assert_eq!(backoff(Duration::from_secs(15), 1), Duration::from_secs(30));
    }

    #[test]
    fn workspace_takes_the_majority_project() {
        let panes = [pane("w1", "/a", false), pane("w1", "/b", false), pane("w1", "/b", false)];
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
        pairs.iter().map(|(w, v)| (w.to_string(), v.to_string())).collect()
    }

    #[test]
    fn plan_sets_new_and_changed_values_and_clears_lost_ones() {
        let now = Instant::now();
        let mut last = HashMap::new();
        last.insert("w1".to_string(), Reported { value: "● ddev".into(), at: now });
        last.insert("w2".to_string(), Reported { value: "● ddev".into(), at: now });
        let reports = plan_reports(&wanted(&[("w1", "○ ddev"), ("w3", "● ddev")]), &last, now, ttl(Duration::from_secs(5)));
        assert_eq!(
            reports,
            [
                Report::Set { workspace: "w1".into(), value: "○ ddev".into() },
                Report::Set { workspace: "w3".into(), value: "● ddev".into() },
                Report::Clear { workspace: "w2".into() },
            ]
        );
    }

    #[test]
    fn plan_refreshes_unchanged_values_after_half_the_ttl() {
        let now = Instant::now();
        let ttl = ttl(Duration::from_secs(5));
        let mut last = HashMap::new();
        last.insert("w1".to_string(), Reported { value: "● ddev".into(), at: now });
        let same = wanted(&[("w1", "● ddev")]);
        assert!(plan_reports(&same, &last, now + Duration::from_secs(9), ttl).is_empty());
        assert_eq!(plan_reports(&same, &last, now + Duration::from_secs(10), ttl).len(), 1);
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
        World { _tmp: tmp, shop, blog, notes }
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
        Output::ok(&format!(r#"{{"result":{{"panes":[{}]}}}}"#, panes.join(",")))
    }

    fn docker_rows(name: &str, root: &Path, state: &str) -> Output {
        Output::ok(&format!("{name}\t{}\t{state}\tweb\n", root.display()))
    }

    fn badge_calls(fake: &FakeRunner) -> Vec<(String, String)> {
        fake.calls_starting_with(&["herdr", "workspace", "report-metadata"])
            .into_iter()
            .map(|call| (call.argv[3].clone(), format!("{} {}", call.argv[6], call.argv[7])))
            .collect()
    }

    fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
        expected.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn tick_reports_running_and_stopped_badges() {
        let w = world();
        let fake = FakeRunner::new();
        fake.on(&["herdr", "pane", "list"], pane_list(&[("w1", &w.shop), ("w2", &w.blog), ("w3", &w.notes)]));
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
        Ticker::new(&ctx, vec!["docker".into()]).tick(Instant::now(), 1_000_000).unwrap();
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
        ticker.tick(t0 + Duration::from_secs(11), 1_011_000).unwrap();
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
        let marker = Marker { project: "shop".into(), verb: Verb::Stop, pid: 1, started_unix: 1_000 };
        busy::acquire(state.path(), &marker, 1_000, &testing::always_alive).unwrap();
        let ctx = testing::ctx(&fake, state.path(), &testing::no_spawn);
        Ticker::new(&ctx, vec!["docker".into()]).tick(Instant::now(), 1_000_000).unwrap();
        assert_eq!(badge_calls(&fake), pairs(&[("w1", "--token ddev=◌ ddev…")]));
    }

    #[test]
    fn docker_failure_sends_nothing() {
        let w = world();
        let fake = FakeRunner::new();
        fake.on(&["herdr", "pane", "list"], pane_list(&[("w1", &w.shop)]));
        fake.on(&["docker"], Output::fail("Cannot connect to the Docker daemon"));
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
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test ticker`
Expected: compile errors: `ttl`, `backoff`, `workspace_projects`, `Ticker` not found.

- [ ] **Step 4: Implement**

Add above the tests in `src/ticker.rs`:

```rust
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::app::Ctx;
use crate::docker::{self, Project};
use crate::herdr::Pane;
use crate::project::{self, StoppedCache};
use crate::{badge, busy};

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
    base.saturating_mul(factor).min(Duration::from_secs(30)).max(base)
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
        per_workspace.entry(pane.workspace_id.as_str()).or_default().push((pane.focused, project));
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
    reports.extend(gone.into_iter().map(|w| Report::Clear { workspace: w.clone() }));
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
        Ticker { ctx, docker, cache, last: HashMap::new() }
    }

    /// One poll: read panes and Docker, then send the badge changes.
    pub fn tick(&mut self, now: Instant, now_unix_ms: u64) -> Result<(), TickError> {
        let ctx = self.ctx;
        let panes = ctx.herdr.panes().map_err(TickError::Herdr)?;
        let found = docker::query(ctx.runner, &self.docker).map_err(TickError::Docker)?;
        let running = project::canonical_projects(found);
        let busy = busy::live_all(&ctx.state_dir, now_unix_ms / 1000, ctx.pid_alive);
        let cache = &mut self.cache;
        let projects =
            workspace_projects(&panes, &mut |dir| project::resolve(dir, &running, cache, now));
        let wanted: BTreeMap<String, String> = projects
            .into_iter()
            .map(|(workspace, project)| {
                let value = badge::text(project.state, busy.get(&project.name).copied());
                (workspace, value.to_string())
            })
            .collect();
        let live: HashSet<&str> = panes.iter().map(|pane| pane.workspace_id.as_str()).collect();
        self.last.retain(|workspace, _| live.contains(workspace.as_str()));
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
                    ctx.herdr.clear_badge(&workspace, now_unix_ms).map_err(TickError::Herdr)?;
                    self.last.remove(&workspace);
                }
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add src
git commit -m "feat: compute and report workspace ddev badges"
```

---

### Task 9: Ticker process, lock and detached start

**Files:**
- Create: `src/lock.rs`, `src/detach.rs`
- Modify: `src/lib.rs`, `src/app.rs`, `src/ticker.rs`, `src/main.rs`

**Interfaces:**
- Consumes: everything from Tasks 2-8.
- Produces:
  - `lock::try_lock(&Path) -> Result<Option<File>>`
  - `detach::spawn_background(exe: &Path, args: &[String], log: &Path) -> Result<Child>`,
    `detach::LOG_LIMIT`
  - `app::App { runner, herdr_bin, ddev, docker, config, config_error: Option<String>,
    state_dir, config_dir, exe, pane_id }`, `App::from_env() -> Result<App>`,
    `App::spawn(&self, &[String]) -> Result<()>`,
    `App::ctx<'a>(&'a self, spawn: &'a dyn Fn(&[String]) -> Result<()>) -> Ctx<'a>`
  - `ticker::lock_path(&Ctx) -> PathBuf`, `ticker::ensure_running(&Ctx) -> Result<()>`,
    `ticker::run(&Ctx, config_error: Option<&str>) -> Result<()>`
  - binary subcommands `ticker` and `ticker --detach`

- [ ] **Step 1: Write the failing tests**

In `src/lib.rs` add `pub mod detach;` and `pub mod lock;`.

`src/lock.rs`:

```rust
//! One ticker per machine.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_lock_fails_until_the_first_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/ticker.lock");
        let first = try_lock(&path).unwrap();
        assert!(first.is_some());
        assert!(try_lock(&path).unwrap().is_none());
        drop(first);
        assert!(try_lock(&path).unwrap().is_some());
    }
}
```

`src/detach.rs`:

```rust
//! Starting the ticker and workers so they outlive the Herdr hook or action that started them.

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Vec<String> {
        vec!["-c".to_string(), script.to_string()]
    }

    #[test]
    fn background_process_writes_to_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("logs/out.log");
        let mut child =
            spawn_background(Path::new("/bin/sh"), &sh("echo hello; echo oops >&2"), &log).unwrap();
        child.wait().unwrap();
        let text = fs::read_to_string(&log).unwrap();
        assert!(text.contains("hello") && text.contains("oops"));
    }

    #[test]
    fn oversized_log_is_emptied_first() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("out.log");
        fs::write(&log, vec![b'x'; LOG_LIMIT as usize + 1]).unwrap();
        let mut child = spawn_background(Path::new("/bin/sh"), &sh("echo new"), &log).unwrap();
        child.wait().unwrap();
        assert_eq!(fs::read_to_string(&log).unwrap(), "new\n");
    }
}
```

Append these tests inside the existing `mod tests` in `src/ticker.rs`:

```rust
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
        assert_eq!(fake.calls_starting_with(&["herdr", "pane", "list"]).len(), 3);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test`
Expected: compile errors: `try_lock`, `spawn_background`, `ensure_running`, `run`,
`lock_path` not found.

- [ ] **Step 3: Implement `lock.rs` and `detach.rs`**

Add above the tests in `src/lock.rs`:

```rust
use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::Path;

use anyhow::Result;

/// Take an exclusive lock on `path` without waiting; `None` means another process holds it.
/// The OS drops the lock when its holder exits, so a crash leaves no stale lock.
pub fn try_lock(path: &Path) -> Result<Option<File>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new().create(true).truncate(false).write(true).open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(err)) => Err(err.into()),
    }
}
```

Add above the tests in `src/detach.rs`:

```rust
use std::fs::{self, OpenOptions};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result};

pub const LOG_LIMIT: u64 = 1024 * 1024;

/// Start `exe args` in its own process group with no stdin, appending stdout and stderr to
/// `log` (emptied first once it passes 1 MB).
pub fn spawn_background(exe: &Path, args: &[String], log: &Path) -> Result<Child> {
    if let Some(parent) = log.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::metadata(log).is_ok_and(|meta| meta.len() > LOG_LIMIT) {
        fs::write(log, "")?;
    }
    let out = OpenOptions::new().create(true).append(true).open(log)?;
    let err = out.try_clone()?;
    Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .process_group(0)
        .spawn()
        .with_context(|| format!("could not start {}", exe.display()))
}
```

- [ ] **Step 4: Add `App` to `app.rs`**

Replace the `use` block at the top of `src/app.rs` with:

```rust
use std::env;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::config::{self, Config, OpenMode};
use crate::herdr::Herdr;
use crate::runner::{Runner, SystemRunner};
use crate::{bins, busy, detach};
```

Add below `unix_ms`:

```rust
/// Everything read from Herdr's plugin environment.
pub struct App {
    pub runner: SystemRunner,
    pub herdr_bin: String,
    pub ddev: Option<Vec<String>>,
    pub docker: Option<Vec<String>>,
    pub config: Config,
    pub config_error: Option<String>,
    pub state_dir: PathBuf,
    pub config_dir: PathBuf,
    pub exe: PathBuf,
    pub pane_id: Option<String>,
}

impl App {
    /// Outside Herdr (development) state and config fall back to `~/.local/state/herdr-ddev`.
    pub fn from_env() -> Result<App> {
        let home = env::var("HOME").unwrap_or_default();
        let fallback = PathBuf::from(&home).join(".local/state/herdr-ddev");
        let dir_var = |name: &str| env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        let state_dir = dir_var("HERDR_PLUGIN_STATE_DIR").unwrap_or_else(|| fallback.clone());
        let config_dir = dir_var("HERDR_PLUGIN_CONFIG_DIR").unwrap_or(fallback);
        let (config, config_error) = config::load(&config_dir);
        let inherited = env::var("PATH").unwrap_or_default();
        let is_exec = &bins::is_executable_file;
        let ddev = bins::resolve(
            config.ddev_command.as_deref(),
            "ddev",
            bins::DDEV_CANDIDATES,
            &inherited,
            &home,
            is_exec,
        );
        let docker = bins::resolve(
            config.docker_command.as_deref(),
            "docker",
            bins::DOCKER_CANDIDATES,
            &inherited,
            &home,
            is_exec,
        );
        let binaries = [ddev.as_deref().unwrap_or(&[]), docker.as_deref().unwrap_or(&[])];
        let path = bins::widened_path(&binaries, &inherited, &home);
        let non_empty = |name: &str| env::var(name).ok().filter(|v| !v.is_empty());
        Ok(App {
            runner: SystemRunner::new(path),
            herdr_bin: non_empty("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".to_string()),
            ddev,
            docker,
            config,
            config_error,
            state_dir,
            config_dir,
            exe: env::current_exe().context("cannot find the herdr-ddev binary")?,
            pane_id: non_empty("HERDR_PANE_ID"),
        })
    }

    /// Start `herdr-ddev <args>` in the background, logging to ticker.log or worker.log.
    pub fn spawn(&self, args: &[String]) -> Result<()> {
        let is_ticker = args.first().map(String::as_str) == Some("ticker");
        let log = self.state_dir.join(if is_ticker { "ticker.log" } else { "worker.log" });
        detach::spawn_background(&self.exe, args, &log).map(drop)
    }

    pub fn ctx<'a>(&'a self, spawn: &'a dyn Fn(&[String]) -> Result<()>) -> Ctx<'a> {
        Ctx {
            runner: &self.runner,
            herdr: Herdr::new(&self.runner, &self.herdr_bin),
            ddev: self.ddev.clone(),
            docker: self.docker.clone(),
            state_dir: self.state_dir.clone(),
            config_dir: self.config_dir.clone(),
            open_mode: self.config.open_mode,
            interval: Duration::from_secs(self.config.poll_interval_secs),
            pane_id: self.pane_id.clone(),
            spawn,
            pid_alive: &busy::pid_alive,
        }
    }
}
```

- [ ] **Step 5: Add the loop to `ticker.rs`**

Extend the `use` block at the top of `src/ticker.rs`:

```rust
use std::path::PathBuf;

use anyhow::Result;

use crate::app::unix_ms;
use crate::herdr::Sound;
use crate::{config, lock};
```

Add below `impl Ticker`:

```rust
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
        eprintln!("herdr-ddev ticker: docker not found; set docker_command in {}", file.display());
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
```

- [ ] **Step 6: Wire the subcommands in `main.rs`**

Replace `run` in `src/main.rs` with:

```rust
use herdr_ddev::app::App;
use herdr_ddev::ticker;

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
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add src
git commit -m "feat: run the badge ticker as a single background process"
```

---

### Task 10: ddev commands

**Files:**
- Create: `src/ddev.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: `runner::{Runner, Output}`, `busy::Verb`.
- Produces:
  - `ddev::run_verb(&dyn Runner, ddev: &[String], root: &Path, Verb) -> Result<Output>`
  - `ddev::first_error_line(&Output) -> String`
  - `ddev::parse_site_url(&str) -> Result<String>`,
    `ddev::site_url(&dyn Runner, &[String], root: &Path) -> Result<String>`
  - `ddev::Listed { name, status, approot, kind, primary_url: String }`,
    `ddev::parse_list(&str) -> Result<Vec<Listed>>`,
    `ddev::list(&dyn Runner, &[String]) -> Result<Vec<Listed>>`

- [ ] **Step 1: Write the failing tests**

In `src/lib.rs` add `pub mod ddev;`. Create `src/ddev.rs`:

```rust
//! ddev commands: start/stop/restart, the site URL, and the project list for the picker.

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
        let text = format!("{}\n{DESCRIBE}\n", r#"{"level":"warning","msg":"Docker is old"}"#);
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
        assert_eq!(call.argv, ["ddev", "start"]);
        assert_eq!(call.cwd.as_deref(), Some(Path::new("/w/shop")));
    }

    #[test]
    fn wrapped_ddev_keeps_the_wrapper() {
        let fake = FakeRunner::new();
        fake.on(&["distrobox"], Output::ok(""));
        let wrapper: Vec<String> =
            ["distrobox", "enter", "--", "ddev"].iter().map(|s| s.to_string()).collect();
        run_verb(&fake, &wrapper, Path::new("/w/shop"), Verb::Stop).unwrap();
        assert_eq!(fake.calls()[0].argv, ["distrobox", "enter", "--", "ddev", "stop"]);
    }

    #[test]
    fn first_error_line_prefers_stderr_and_caps_length() {
        let out = Output {
            success: false,
            stdout: "out\n".into(),
            stderr: "\n  Failed to start shop: port 443 in use\nmore".into(),
        };
        assert_eq!(first_error_line(&out), "Failed to start shop: port 443 in use");
        assert_eq!(first_error_line(&Output::fail(&"x".repeat(500))).len(), 200);
        assert_eq!(first_error_line(&Output::fail("")), "ddev failed without output");
    }

    #[test]
    fn site_url_reports_ddev_failure() {
        let fake = FakeRunner::new();
        fake.on(&["ddev", "describe"], Output::fail("not a ddev project"));
        let err = site_url(&fake, &ddev(), Path::new("/w")).unwrap_err();
        assert!(err.to_string().contains("not a ddev project"));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test ddev`
Expected: compile errors: `parse_site_url`, `run_verb`, `parse_list` not found.

- [ ] **Step 3: Implement**

Add above the tests in `src/ddev.rs`:

```rust
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::busy::Verb;
use crate::runner::{Output, Runner};

fn argv(ddev: &[String], args: &[&str]) -> Vec<String> {
    ddev.iter().cloned().chain(args.iter().map(|a| a.to_string())).collect()
}

/// Run `ddev start|stop|restart` in the project root. `Err` only when ddev could not run.
pub fn run_verb(runner: &dyn Runner, ddev: &[String], root: &Path, verb: Verb) -> Result<Output> {
    runner.run(&argv(ddev, &[verb.as_str()]), Some(root))
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
    serde_json::from_str(text.trim())
        .ok()
        .or_else(|| text.lines().rev().find_map(|line| serde_json::from_str(line.trim()).ok()))
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: all pass.

- [ ] **Step 5: Check against real ddev (read-only)**

Run: `cd <any ddev project> && ddev describe -j | jq -c '.raw | {primary_url, httpsurl}' && ddev list -j | jq -c '.raw[0] | {name, status, approot, type, primary_url}'`
Expected: the same field names the code reads. Skip if ddev is not installed; say so.

- [ ] **Step 6: Commit**

```bash
git add src
git commit -m "feat: run ddev verbs and read site URLs and project list"
```

---

### Task 11: Open site and the URL popup

**Files:**
- Create: `src/open.rs`
- Modify: `src/lib.rs`, `Cargo.toml`

**Interfaces:**
- Consumes: `config::OpenMode`.
- Produces:
  - `open::Opener { Browser(&'static str), Clipboard }`
  - `open::decide(OpenMode, os: &str, env: &dyn Fn(&str) -> Option<String>) -> Opener`
  - `open::base64(&[u8]) -> String`, `open::osc52(&str) -> String`
  - `open::run_url_popup(url: &str) -> Result<()>`

- [ ] **Step 1: Add dependencies and the module**

In `Cargo.toml` `[dependencies]` add:

```toml
crossterm = "0.29"
ratatui = "0.30"
```

In `src/lib.rs` add `pub mod open;`.

- [ ] **Step 2: Write the failing tests**

`src/open.rs`:

```rust
//! Opening a site: a browser when there is a local desktop, the clipboard otherwise.

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string())
    }

    #[test]
    fn explicit_modes_win() {
        let none = env_with(&[]);
        assert_eq!(decide(OpenMode::Browser, "linux", &none), Opener::Browser("xdg-open"));
        assert_eq!(decide(OpenMode::Clipboard, "macos", &none), Opener::Clipboard);
    }

    #[test]
    fn auto_uses_the_browser_on_a_local_mac() {
        assert_eq!(decide(OpenMode::Auto, "macos", &env_with(&[])), Opener::Browser("open"));
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
        assert_eq!(decide(OpenMode::Auto, "linux", &x11), Opener::Browser("xdg-open"));
        assert_eq!(decide(OpenMode::Auto, "linux", &wayland), Opener::Browser("xdg-open"));
        assert_eq!(decide(OpenMode::Auto, "linux", &env_with(&[])), Opener::Clipboard);
    }

    #[test]
    fn empty_variables_count_as_unset() {
        let empty_ssh = env_with(&[("SSH_CONNECTION", "")]);
        assert_eq!(decide(OpenMode::Auto, "macos", &empty_ssh), Opener::Browser("open"));
    }

    #[test]
    fn base64_matches_rfc4648_vectors() {
        let cases = [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foobar", "Zm9vYmFy")];
        for (input, expected) in cases {
            assert_eq!(base64(input.as_bytes()), expected);
        }
    }

    #[test]
    fn osc52_wraps_base64_in_the_clipboard_escape() {
        assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07");
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test open`
Expected: compile errors: `decide`, `Opener`, `base64`, `osc52` not found.

- [ ] **Step 4: Implement**

Add above the tests in `src/open.rs`:

```rust
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
            if desktop && !set("SSH_CONNECTION") { Opener::Browser(browser) } else { Opener::Clipboard }
        }
    }
}

pub fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
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
    writeln!(stdout, "\n\n  {url}\n\n  Copied - or drag to select it. Press any key to close.")?;
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
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: all pass. Check `cargo tree -d | grep crossterm` shows no duplicate crossterm
versions (ratatui 0.30 uses crossterm 0.29).

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src
git commit -m "feat: decide how to open sites and add the URL popup"
```

---

### Task 12: Actions and the background worker

**Files:**
- Create: `src/actions.rs`
- Modify: `src/lib.rs`, `src/main.rs`

**Interfaces:**
- Consumes: `app::{Ctx, unix_ms}`, `busy`, `docker`, `project`, `ddev`, `open`, `ticker`,
  `herdr::Sound`, `config::FILE_NAME`.
- Produces:
  - `actions::Kind { Toggle, Start, Stop, Restart, Open, Picker, Configure, Unconfigure }`,
    `Kind::parse(&str) -> Option<Kind>`
  - `actions::toggle_verb(State) -> Verb`
  - `actions::worker_args(Verb, root: &Path, name: &str) -> Vec<String>`
  - `actions::current_project(&Ctx) -> Result<Option<Project>>`
  - `actions::open_url(&Ctx, url: &str) -> Result<()>`
  - `actions::run(Kind, &Ctx) -> Result<()>`
  - `actions::worker(&Ctx, Verb, root: &Path, name: &str) -> Result<()>`
  - binary subcommands `action <id>`, `worker <verb> <root> <name>`, `url`

- [ ] **Step 1: Write the failing tests**

In `src/lib.rs` add `pub mod actions;`. Create `src/actions.rs`:

```rust
//! What the keys do: act on the project behind the focused pane, or open a popup.

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
        assert_eq!(spawned[1], ["worker", "start", shop.to_str().unwrap(), "shop"]);
    }

    #[test]
    fn toggle_stops_a_running_project() {
        let (_tmp, shop, _) = shop();
        let state = tempfile::tempdir().unwrap();
        let fake = FakeRunner::new();
        fake.on(&["herdr", "pane", "current"], pane_json(&shop));
        fake.on(&["docker"], Output::ok(&format!("shop\t{}\trunning\tweb\n", shop.display())));
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
        assert!(fake.calls_starting_with(&["herdr", "pane", "current"]).is_empty());
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
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        let fake = FakeRunner::new();
        fake.on(&["herdr", "pane", "current"], pane_json(&shop));
        fake.on(&["docker"], Output::ok(""));
        fake.on(&["ddev", "describe"], Output::ok(DESCRIBE));
        fake.on(&[opener], Output::ok(""));
        let mut ctx = testing::ctx(&fake, state.path(), &testing::no_spawn);
        ctx.open_mode = OpenMode::Browser;
        run(Kind::Open, &ctx).unwrap();
        assert_eq!(fake.calls_starting_with(&[opener])[0].argv, [opener, "https://shop.ddev.site"]);
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
        assert_eq!(notifications(&fake), [("shop started".to_string(), "done".to_string())]);
        assert!(busy::read(state.path(), "shop", unix_ms() / 1000, &testing::always_alive).is_none());
    }

    #[test]
    fn worker_refuses_when_the_project_is_busy() {
        let (_tmp, shop, fake) = worker_world(Output::ok(""));
        let state = tempfile::tempdir().unwrap();
        let now = unix_ms() / 1000;
        let marker = Marker { project: "shop".into(), verb: Verb::Start, pid: 1, started_unix: now };
        busy::acquire(state.path(), &marker, now, &testing::always_alive).unwrap();
        let ctx = testing::ctx(&fake, state.path(), &testing::no_spawn);
        worker(&ctx, Verb::Stop, &shop, "shop").unwrap();
        assert_eq!(notifications(&fake)[0].0, "shop is already starting");
        assert!(fake.calls_starting_with(&["ddev"]).is_empty());
    }

    #[test]
    fn worker_reports_the_first_ddev_error_line() {
        let (_tmp, shop, fake) = worker_world(Output::fail("Failed to start shop: port 443 in use"));
        let state = tempfile::tempdir().unwrap();
        let ctx = testing::ctx(&fake, state.path(), &testing::no_spawn);
        worker(&ctx, Verb::Start, &shop, "shop").unwrap();
        assert_eq!(
            notifications(&fake),
            [("shop: Failed to start shop: port 443 in use".to_string(), "request".to_string())]
        );
        assert!(busy::read(state.path(), "shop", unix_ms() / 1000, &testing::always_alive).is_none());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test actions`
Expected: compile errors: `Kind`, `toggle_verb`, `worker_args`, `run`, `worker` not found.

- [ ] **Step 3: Implement**

Add above the tests in `src/actions.rs`:

```rust
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
    if state == State::Running { Verb::Stop } else { Verb::Start }
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
    pane.dir().map(str::to_string).context("the focused pane has no folder")
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
    Ok(project::resolve(Path::new(&dir), &running, &mut cache, Instant::now()))
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
        Err(err) => ctx.herdr.notify(&format!("{}: {err:#}", project.name), Sound::Request),
    }
}

/// Open `url` in the browser, or hand it to the `url` popup for the clipboard.
pub fn open_url(ctx: &Ctx, url: &str) -> Result<()> {
    match open::decide(ctx.open_mode, std::env::consts::OS, &|key| std::env::var(key).ok()) {
        Opener::Browser(program) => {
            let out = ctx.runner.run(&[program.to_string(), url.to_string()], None)?;
            if out.success {
                Ok(())
            } else {
                ctx.herdr.notify(&format!("could not open {url}"), Sound::Request)
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
    let marker =
        Marker { project: name.to_string(), verb, pid: std::process::id(), started_unix: now };
    if let Acquire::Busy(existing) = busy::acquire(&ctx.state_dir, &marker, now, ctx.pid_alive)? {
        let body = format!("{name} is already {}", existing.verb.progressive());
        return ctx.herdr.notify(&body, Sound::Request);
    }
    refresh_badges(ctx);
    let result = ddev::run_verb(ctx.runner, &ddev_cmd, root, verb);
    busy::release(&ctx.state_dir, name);
    refresh_badges(ctx);
    match result {
        Ok(out) if out.success => ctx.herdr.notify(&format!("{name} {}", verb.past()), Sound::Done),
        Ok(out) => {
            eprintln!("ddev {} {name} failed:\n{}{}", verb.as_str(), out.stdout, out.stderr);
            let body = format!("{name}: {}", ddev::first_error_line(&out));
            ctx.herdr.notify(&body, Sound::Request)
        }
        Err(err) => ctx.herdr.notify(&format!("{name}: {err:#}"), Sound::Request),
    }
}

fn refresh_badges(ctx: &Ctx) {
    if let Some(docker) = ctx.docker.clone() {
        let _ = ticker::Ticker::new(ctx, docker).tick(Instant::now(), unix_ms());
    }
}
```

- [ ] **Step 4: Wire the subcommands in `main.rs`**

Replace the `use herdr_ddev::...` lines and `run` in `src/main.rs` with:

```rust
use std::path::Path;

use anyhow::Context;
use herdr_ddev::actions::{self, Kind};
use herdr_ddev::app::App;
use herdr_ddev::busy::Verb;
use herdr_ddev::{open, ticker};

fn run(args: &[String]) -> Result<()> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["--version" | "-V"] => {
            println!("herdr-ddev {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        ["url"] => return open::run_url_popup(&std::env::var("HERDR_DDEV_URL").unwrap_or_default()),
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
        _ => bail!("{USAGE}"),
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add src
git commit -m "feat: add start/stop/restart/open actions and background worker"
```

---

### Task 13: Project picker

**Files:**
- Create: `src/picker.rs`
- Modify: `src/lib.rs`, `src/main.rs`

**Interfaces:**
- Consumes: `app::{Ctx, unix_ms}`, `ddev::{self, Listed}`, `docker::{self, Project, State}`,
  `herdr::Pane`, `project`, `busy`, `badge`, `open`, `actions::{toggle_verb, worker_args}`,
  `ticker::ensure_running`.
- Produces:
  - `picker::Row { name: String, root: PathBuf, state: State, kind: String, url: String,
    workspace: Option<String>, busy: Option<Verb> }`
  - `picker::build_rows(&[Listed], running: &[Project], &[Pane]) -> Vec<Row>`
  - `picker::row_line(&Row) -> String`
  - `picker::Command { None, Quit, Jump(usize), Change(usize, Verb), Open(usize) }`
  - `picker::Picker { rows, filter: String, selected: usize, status: String }`,
    `Picker::new(Vec<Row>)`, `Picker::visible() -> Vec<usize>`,
    `Picker::handle(KeyEvent) -> Command`
  - `picker::run(&Ctx) -> Result<()>`; binary subcommand `picker`

- [ ] **Step 1: Write the failing tests**

In `src/lib.rs` add `pub mod picker;`. Create `src/picker.rs`:

```rust
//! The project picker popup.

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(name: &str, root: &str, status: &str) -> Listed {
        Listed {
            name: name.into(),
            status: status.into(),
            approot: root.into(),
            kind: "drupal11".into(),
            primary_url: format!("https://{name}.ddev.site"),
        }
    }

    fn pane(workspace: &str, dir: &str) -> Pane {
        Pane {
            pane_id: format!("{workspace}:p1"),
            workspace_id: workspace.into(),
            focused: false,
            cwd: Some(dir.into()),
            foreground_cwd: None,
        }
    }

    fn rows() -> Vec<Row> {
        let listed = [
            listed("zeta", "/w/zeta", "stopped"),
            listed("blog", "/w/blog", "stopped"),
            listed("shop", "/w/shop", "stopped"),
            listed("orphan", "", "stopped"),
        ];
        let running = [
            Project { name: "shop".into(), root: "/w/shop".into(), state: State::Running },
            Project { name: "extra".into(), root: "/w/extra".into(), state: State::Paused },
        ];
        let panes = [pane("w1", "/w/shop/web"), pane("w2", "/w/blog"), pane("w3", "/w/shop")];
        build_rows(&listed, &running, &panes)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn rows_merge_ddev_list_and_docker_and_sort_by_state_then_name() {
        let names: Vec<&str> = rows().iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["shop", "extra", "blog", "zeta"]);
        assert_eq!(rows()[1].state, State::Paused);
        assert_eq!(rows()[1].kind, "");
    }

    #[test]
    fn rows_link_the_first_workspace_inside_the_project() {
        let rows = rows();
        assert_eq!(rows[0].workspace.as_deref(), Some("w1"));
        assert_eq!(rows[2].workspace.as_deref(), Some("w2"));
        assert_eq!(rows[3].workspace, None);
    }

    #[test]
    fn typing_filters_by_name_or_folder_case_insensitively() {
        let mut picker = Picker::new(rows());
        for c in "BLO".chars() {
            picker.handle(key(KeyCode::Char(c)));
        }
        assert_eq!(picker.visible(), [2]);
        picker.handle(key(KeyCode::Backspace));
        picker.handle(key(KeyCode::Backspace));
        picker.handle(key(KeyCode::Backspace));
        for c in "/w/z".chars() {
            picker.handle(key(KeyCode::Char(c)));
        }
        assert_eq!(picker.visible(), [3]);
    }

    #[test]
    fn plain_letters_type_instead_of_acting() {
        let mut picker = Picker::new(rows());
        assert_eq!(picker.handle(key(KeyCode::Char('s'))), Command::None);
        assert_eq!(picker.filter, "s");
    }

    #[test]
    fn escape_clears_the_filter_then_quits() {
        let mut picker = Picker::new(rows());
        picker.handle(key(KeyCode::Char('x')));
        assert_eq!(picker.handle(key(KeyCode::Esc)), Command::None);
        assert_eq!(picker.filter, "");
        assert_eq!(picker.handle(key(KeyCode::Esc)), Command::Quit);
    }

    #[test]
    fn movement_stays_in_bounds() {
        let mut picker = Picker::new(rows());
        picker.handle(key(KeyCode::Up));
        assert_eq!(picker.selected, 0);
        for _ in 0..10 {
            picker.handle(key(KeyCode::Down));
        }
        assert_eq!(picker.selected, 3);
        picker.handle(ctrl('p'));
        assert_eq!(picker.selected, 2);
        picker.handle(ctrl('n'));
        assert_eq!(picker.selected, 3);
    }

    #[test]
    fn control_keys_act_on_the_selected_row() {
        let mut picker = Picker::new(rows());
        assert_eq!(picker.handle(ctrl('s')), Command::Change(0, Verb::Stop));
        assert_eq!(picker.handle(ctrl('r')), Command::Change(0, Verb::Restart));
        assert_eq!(picker.handle(ctrl('o')), Command::Open(0));
        picker.handle(key(KeyCode::Down));
        picker.handle(key(KeyCode::Down));
        assert_eq!(picker.handle(ctrl('s')), Command::Change(2, Verb::Start));
        assert_eq!(picker.handle(key(KeyCode::Enter)), Command::Jump(2));
    }

    #[test]
    fn no_match_means_no_action() {
        let mut picker = Picker::new(rows());
        for c in "nothing".chars() {
            picker.handle(key(KeyCode::Char(c)));
        }
        assert_eq!(picker.handle(key(KeyCode::Enter)), Command::None);
        assert_eq!(picker.handle(ctrl('s')), Command::None);
    }

    #[test]
    fn row_line_shows_symbol_status_type_and_url() {
        let mut row = rows().remove(0);
        assert_eq!(row_line(&row), "● shop  ·  running  ·  drupal11  ·  https://shop.ddev.site");
        row.busy = Some(Verb::Stop);
        assert!(row_line(&row).starts_with("◌ shop  ·  stopping"));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test picker`
Expected: compile errors: `build_rows`, `Picker`, `Row`, `Command` not found.

- [ ] **Step 3: Implement the picker state**

Add above the tests in `src/picker.rs`:

```rust
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use crate::actions::{toggle_verb, worker_args};
use crate::app::{Ctx, unix_ms};
use crate::busy::{self, Verb};
use crate::ddev::{self, Listed};
use crate::docker::{self, Project, State};
use crate::herdr::Pane;
use crate::open::{self, Opener};
use crate::{badge, project, ticker};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub root: PathBuf,
    pub state: State,
    pub kind: String,
    pub url: String,
    pub workspace: Option<String>,
    pub busy: Option<Verb>,
}

fn state_from_status(status: &str) -> State {
    match status {
        "running" => State::Running,
        "paused" => State::Paused,
        _ => State::Stopped,
    }
}

fn rank(state: State) -> u8 {
    match state {
        State::Running => 0,
        State::Paused => 1,
        State::Stopped => 2,
    }
}

/// Rows from `ddev list` (every known project, with type and URL) updated with Docker's
/// state (`running` must be canonical), each linked to the first workspace with a pane inside
/// it. Running first, then paused, then stopped, then by name.
pub fn build_rows(listed: &[Listed], running: &[Project], panes: &[Pane]) -> Vec<Row> {
    let mut rows: Vec<Row> = listed
        .iter()
        .filter(|l| !l.approot.is_empty())
        .map(|l| Row {
            name: l.name.clone(),
            root: project::canonical(Path::new(&l.approot)),
            state: state_from_status(&l.status),
            kind: l.kind.clone(),
            url: l.primary_url.clone(),
            workspace: None,
            busy: None,
        })
        .collect();
    for found in running {
        match rows.iter_mut().find(|row| row.root == found.root) {
            Some(row) => row.state = found.state,
            None => rows.push(Row {
                name: found.name.clone(),
                root: found.root.clone(),
                state: found.state,
                kind: String::new(),
                url: String::new(),
                workspace: None,
                busy: None,
            }),
        }
    }
    for pane in panes {
        let Some(dir) = pane.dir() else { continue };
        let dir = project::canonical(Path::new(dir));
        let owner = rows
            .iter_mut()
            .filter(|row| dir.starts_with(&row.root))
            .max_by_key(|row| row.root.components().count());
        if let Some(row) = owner.filter(|row| row.workspace.is_none()) {
            row.workspace = Some(pane.workspace_id.clone());
        }
    }
    rows.sort_by(|a, b| rank(a.state).cmp(&rank(b.state)).then_with(|| a.name.cmp(&b.name)));
    rows
}

pub fn row_line(row: &Row) -> String {
    let status = match (row.busy, row.state) {
        (Some(verb), _) => verb.progressive(),
        (None, State::Running) => "running",
        (None, State::Paused) => "paused",
        (None, State::Stopped) => "stopped",
    };
    let mut parts = vec![format!("{} {}", badge::symbol(row.state, row.busy), row.name)];
    parts.push(status.to_string());
    parts.extend([&row.kind, &row.url].into_iter().filter(|s| !s.is_empty()).cloned());
    parts.join("  ·  ")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    None,
    Quit,
    Jump(usize),
    Change(usize, Verb),
    Open(usize),
}

pub struct Picker {
    pub rows: Vec<Row>,
    pub filter: String,
    pub selected: usize,
    pub status: String,
}

impl Picker {
    pub fn new(rows: Vec<Row>) -> Picker {
        Picker { rows, filter: String::new(), selected: 0, status: String::new() }
    }

    /// Indices of rows whose name or folder contains the filter, ignoring case.
    pub fn visible(&self) -> Vec<usize> {
        let needle = self.filter.to_lowercase();
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                needle.is_empty()
                    || row.name.to_lowercase().contains(&needle)
                    || row.root.to_string_lossy().to_lowercase().contains(&needle)
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn selected_row(&self) -> Option<usize> {
        self.visible().get(self.selected).copied()
    }

    fn move_by(&mut self, delta: isize) {
        let count = self.visible().len();
        self.selected =
            if count == 0 { 0 } else { self.selected.saturating_add_signed(delta).min(count - 1) };
    }

    fn type_char(&mut self, c: Option<char>) -> Command {
        match c {
            Some(c) => self.filter.push(c),
            None => {
                self.filter.pop();
            }
        }
        self.selected = 0;
        Command::None
    }

    pub fn handle(&mut self, key: KeyEvent) -> Command {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc if !self.filter.is_empty() => self.clear_filter(),
            KeyCode::Esc => Command::Quit,
            KeyCode::Char('c') if ctrl => Command::Quit,
            KeyCode::Enter => self.selected_row().map_or(Command::None, Command::Jump),
            KeyCode::Up => self.moved(-1),
            KeyCode::Char('p') if ctrl => self.moved(-1),
            KeyCode::Down => self.moved(1),
            KeyCode::Char('n') if ctrl => self.moved(1),
            KeyCode::Char('s') if ctrl => self.selected_row().map_or(Command::None, |index| {
                Command::Change(index, toggle_verb(self.rows[index].state))
            }),
            KeyCode::Char('r') if ctrl => self
                .selected_row()
                .map_or(Command::None, |index| Command::Change(index, Verb::Restart)),
            KeyCode::Char('o') if ctrl => self.selected_row().map_or(Command::None, Command::Open),
            KeyCode::Backspace => self.type_char(None),
            KeyCode::Char(c) if !ctrl && !alt => self.type_char(Some(c)),
            _ => Command::None,
        }
    }

    fn moved(&mut self, delta: isize) -> Command {
        self.move_by(delta);
        Command::None
    }

    fn clear_filter(&mut self) -> Command {
        self.filter.clear();
        self.selected = 0;
        Command::None
    }
}
```

- [ ] **Step 4: Run the state tests to verify they pass**

Run: `cargo test picker`
Expected: all picker tests pass (the UI code below is not compiled in yet).

- [ ] **Step 5: Add the terminal UI**

Append below `impl Picker` in `src/picker.rs`:

```rust
/// The popup: list projects, filter, act, jump.
pub fn run(ctx: &Ctx) -> Result<()> {
    let _ = ticker::ensure_running(ctx);
    let mut picker = Picker::new(load_rows(ctx));
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut picker, ctx);
    ratatui::restore();
    result
}

fn current_running(ctx: &Ctx) -> Vec<Project> {
    ctx.docker
        .as_ref()
        .and_then(|docker| docker::query(ctx.runner, docker).ok())
        .map(project::canonical_projects)
        .unwrap_or_default()
}

fn mark_busy(ctx: &Ctx, rows: &mut [Row]) {
    let busy = busy::live_all(&ctx.state_dir, unix_ms() / 1000, ctx.pid_alive);
    for row in rows {
        row.busy = busy.get(&row.name).copied();
    }
}

fn load_rows(ctx: &Ctx) -> Vec<Row> {
    let listed =
        ctx.ddev.as_ref().and_then(|d| ddev::list(ctx.runner, d).ok()).unwrap_or_default();
    let panes = ctx.herdr.panes().unwrap_or_default();
    let mut rows = build_rows(&listed, &current_running(ctx), &panes);
    mark_busy(ctx, &mut rows);
    rows
}

/// Re-read Docker and busy markers so changes show without reopening. Rows keep their order.
fn refresh(ctx: &Ctx, picker: &mut Picker) {
    let running = current_running(ctx);
    for row in &mut picker.rows {
        row.state = running.iter().find(|p| p.root == row.root).map_or(State::Stopped, |p| p.state);
    }
    mark_busy(ctx, &mut picker.rows);
}

fn event_loop(terminal: &mut DefaultTerminal, picker: &mut Picker, ctx: &Ctx) -> Result<()> {
    let mut last_refresh = Instant::now();
    loop {
        terminal.draw(|frame| draw(frame, picker))?;
        if event::poll(Duration::from_millis(500))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match picker.handle(key) {
                Command::None => {}
                Command::Quit => return Ok(()),
                Command::Jump(index) => return jump(ctx, &picker.rows[index]),
                Command::Change(index, verb) => change(ctx, picker, index, verb),
                Command::Open(index) => picker.status = open_row(ctx, &picker.rows[index]),
            }
        }
        if last_refresh.elapsed() >= Duration::from_secs(2) {
            refresh(ctx, picker);
            last_refresh = Instant::now();
        }
    }
}

fn change(ctx: &Ctx, picker: &mut Picker, index: usize, verb: Verb) {
    let row = &mut picker.rows[index];
    match (ctx.spawn)(&worker_args(verb, &row.root, &row.name)) {
        Ok(()) => {
            row.busy = Some(verb);
            picker.status = format!("{} {}…", row.name, verb.progressive());
        }
        Err(err) => picker.status = format!("could not start the worker: {err:#}"),
    }
}

fn jump(ctx: &Ctx, row: &Row) -> Result<()> {
    match &row.workspace {
        Some(workspace) => ctx.herdr.focus_workspace(workspace),
        None => ctx.herdr.create_workspace(&row.root, &row.name),
    }
}

/// Open the row's site. Only one Herdr popup can be open, so clipboard mode copies from the
/// picker itself and shows the URL in the status line.
fn open_row(ctx: &Ctx, row: &Row) -> String {
    let url = if row.url.is_empty() {
        match ctx.ddev.as_ref().map(|d| ddev::site_url(ctx.runner, d, &row.root)) {
            Some(Ok(url)) => url,
            Some(Err(err)) => return format!("{}: {err:#}", row.name),
            None => return "ddev not found".to_string(),
        }
    } else {
        row.url.clone()
    };
    match open::decide(ctx.open_mode, std::env::consts::OS, &|key| std::env::var(key).ok()) {
        Opener::Browser(program) => match ctx.runner.run(&[program.to_string(), url.clone()], None) {
            Ok(out) if out.success => format!("opened {url}"),
            _ => format!("could not open {url}"),
        },
        Opener::Clipboard => {
            print!("{}", open::osc52(&url));
            let _ = std::io::Write::flush(&mut std::io::stdout());
            format!("Copied {url} - or select it here")
        }
    }
}

fn draw(frame: &mut Frame, picker: &Picker) {
    let [header, list_area, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)])
            .areas(frame.area());
    frame.render_widget(Paragraph::new(format!(" ddev > {}", picker.filter)), header);
    let visible = picker.visible();
    let items: Vec<ListItem> =
        visible.iter().map(|&index| ListItem::new(row_line(&picker.rows[index]))).collect();
    let mut state = ListState::default().with_selected((!visible.is_empty()).then_some(picker.selected));
    let list = List::new(items).highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    frame.render_stateful_widget(list, list_area, &mut state);
    let help = if picker.status.is_empty() {
        " enter jump · ctrl+s start/stop · ctrl+r restart · ctrl+o open · esc close".to_string()
    } else {
        format!(" {}", picker.status)
    };
    frame.render_widget(Paragraph::new(help).style(Style::new().add_modifier(Modifier::DIM)), footer);
}
```

- [ ] **Step 6: Wire `picker` in `main.rs`**

In `src/main.rs` change `use herdr_ddev::{open, ticker};` to
`use herdr_ddev::{open, picker, ticker};` and add this arm before `_ => bail!("{USAGE}")`:

```rust
        ["picker"] => picker::run(&ctx),
```

- [ ] **Step 7: Run everything**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add src
git commit -m "feat: add the project picker popup"
```

---

### Task 14: Configure and unconfigure

**Files:**
- Create: `src/configure.rs`, `tests/fixtures/herdr-config.toml`
- Modify: `src/lib.rs`, `src/main.rs`, `Cargo.toml`

**Interfaces:**
- Consumes: `app::{Ctx, unix_ms}`, `herdr::Herdr::{config_check, reload_config}`.
- Produces:
  - `configure::Binding { key, command, description: &'static str }`,
    `configure::BINDINGS: [Binding; 3]`
  - `configure::SidebarChange { None, CreatedRows, AppendedRow }` (serde)
  - `configure::OwnedKey { key: String, command: String }`,
    `configure::Owned { keys: Vec<OwnedKey>, sidebar: SidebarChange }`, `Owned::merge(Owned)`
  - `configure::Plan { add: Vec<Binding>, skipped: Vec<String>, sidebar: SidebarChange }`
  - `configure::used_keys(&DocumentMut) -> Vec<(String, String)>`
  - `configure::plan(&DocumentMut) -> Plan`,
    `configure::apply(&mut DocumentMut, &Plan) -> Result<Owned>`,
    `configure::unapply(&mut DocumentMut, &Owned)`, `configure::describe(&Plan) -> Vec<String>`
  - `configure::herdr_config_path() -> PathBuf`, `load_owned(&Path) -> Owned`,
    `save_owned(&Path, &Owned) -> Result<()>`
  - `configure::run_configure(&Ctx) -> Result<()>`, `configure::run_unconfigure(&Ctx)
    -> Result<()>`; binary subcommands `configure`, `unconfigure`

- [ ] **Step 1: Add the dependency, module and fixture**

In `Cargo.toml` `[dependencies]` add `toml_edit = "0.25"`. In `src/lib.rs` add
`pub mod configure;`.

`tests/fixtures/herdr-config.toml` (the shape of Daniel's real config, with comments and
bindings from other plugins):

```toml
onboarding = false

[ui.toast]
delivery = "system"

[theme]
name = "catppuccin"
auto_switch = false

# vim-herdr-navigation: Ctrl+h/j/k/l across Neovim splits and herdr panes
[[keys.command]]
key = "ctrl+h"
type = "plugin_action"
command = "vim-herdr-navigation.left"
description = "navigate left (vim/herdr)"

# reviewr: toggle the diff review pane
[[keys.command]]
key = "prefix+d"
type = "plugin_action"
command = "persiyanov.reviewr.toggle"
description = "toggle reviewr diff pane"
```

- [ ] **Step 2: Write the failing tests**

`src/configure.rs`:

```rust
//! Adding and removing the `$ddev` badge and keybindings in Herdr's `config.toml`.

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/herdr-config.toml");

    fn doc(text: &str) -> DocumentMut {
        text.parse().unwrap()
    }

    fn settings(text: &str) -> toml::Table {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn empty_config_gets_all_keys_and_default_rows() {
        let plan = plan(&doc(""));
        assert_eq!(plan.add.len(), 3);
        assert!(plan.skipped.is_empty());
        assert_eq!(plan.sidebar, SidebarChange::CreatedRows);
    }

    #[test]
    fn keys_in_use_are_skipped_and_explained() {
        let text = concat!(
            "[keys]\nsettings = [\"prefix+s\", \"prefix+shift+s\"]\n",
            "[[keys.command]]\nkey = \"prefix+shift+o\"\ntype = \"shell\"\ncommand = \"x\"\n"
        );
        let plan = plan(&doc(text));
        assert_eq!(plan.add.iter().map(|b| b.key).collect::<Vec<_>>(), ["prefix+shift+e"]);
        assert!(plan.skipped[0].contains("prefix+shift+s is already bound to settings"));
        assert!(plan.skipped[1].contains("prefix+shift+o is already bound to x"));
    }

    #[test]
    fn existing_ddev_bindings_are_recognized() {
        let mut first = doc("");
        let plan_one = plan(&first);
        apply(&mut first, &plan_one).unwrap();
        let again = plan(&first);
        assert!(again.add.is_empty());
        assert_eq!(again.sidebar, SidebarChange::None);
        assert!(again.skipped[0].contains("already runs danjuls.ddev.toggle"));
    }

    #[test]
    fn custom_rows_get_an_appended_ddev_row() {
        let text = "[ui.sidebar.spaces]\nrows = [[\"workspace\"]]\n";
        assert_eq!(plan(&doc(text)).sidebar, SidebarChange::AppendedRow);
    }

    #[test]
    fn rows_that_already_show_ddev_are_left_alone() {
        let text = "[ui.sidebar.spaces]\nrows = [[\"workspace\", \"$ddev\"]]\n";
        assert_eq!(plan(&doc(text)).sidebar, SidebarChange::None);
    }

    #[test]
    fn apply_writes_bindings_and_a_styled_token() {
        let mut document = doc("");
        let plan = plan(&document);
        let owned = apply(&mut document, &plan).unwrap();
        let text = document.to_string();
        let parsed = settings(&text);
        let commands = parsed["keys"]["command"].as_array().unwrap();
        assert_eq!(commands.len(), 3);
        assert_eq!(commands[0]["type"].as_str(), Some("plugin_action"));
        assert_eq!(commands[0]["command"].as_str(), Some("danjuls.ddev.toggle"));
        let rows = parsed["ui"]["sidebar"]["spaces"]["rows"].as_array().unwrap();
        let token = rows[1].as_array().unwrap()[2].as_table().unwrap();
        assert_eq!(token["token"].as_str(), Some("$ddev"));
        assert_eq!(token["rules"].as_array().unwrap().len(), 4);
        assert_eq!(owned.keys.len(), 3);
        assert_eq!(owned.sidebar, SidebarChange::CreatedRows);
    }

    #[test]
    fn apply_keeps_comments_and_other_bindings() {
        let mut document = doc(FIXTURE);
        let plan = plan(&document);
        apply(&mut document, &plan).unwrap();
        let text = document.to_string();
        assert!(text.contains("# vim-herdr-navigation"));
        assert!(text.contains("# reviewr"));
        let keys: Vec<String> = used_keys(&document).into_iter().map(|(k, _)| k).collect();
        assert!(keys.contains(&"prefix+d".to_string()));
        assert!(keys.contains(&"prefix+shift+e".to_string()));
    }

    #[test]
    fn unapply_returns_the_same_settings() {
        for original in [FIXTURE, ""] {
            let mut document = doc(original);
            let plan = plan(&document);
            let owned = apply(&mut document, &plan).unwrap();
            unapply(&mut document, &owned);
            let after = document.to_string();
            assert_eq!(settings(&after), settings(original));
            assert!(after.contains("# reviewr") || original.is_empty());
        }
    }

    #[test]
    fn unapply_removes_only_the_appended_row() {
        let text = "[ui.sidebar.spaces]\nrows = [[\"workspace\"]]\n";
        let mut document = doc(text);
        let plan = plan(&document);
        let owned = apply(&mut document, &plan).unwrap();
        unapply(&mut document, &owned);
        assert_eq!(settings(&document.to_string()), settings(text));
    }

    #[test]
    fn owned_merge_keeps_unique_keys_and_the_first_sidebar_change() {
        let key = |k: &str| OwnedKey { key: k.into(), command: "c".into() };
        let mut owned = Owned { keys: vec![key("a")], sidebar: SidebarChange::CreatedRows };
        owned.merge(Owned { keys: vec![key("a"), key("b")], sidebar: SidebarChange::AppendedRow });
        assert_eq!(owned.keys, [key("a"), key("b")]);
        assert_eq!(owned.sidebar, SidebarChange::CreatedRows);
    }

    #[test]
    fn owned_round_trips_through_the_state_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_owned(dir.path()), Owned::default());
        let owned = Owned {
            keys: vec![OwnedKey { key: "prefix+shift+s".into(), command: "danjuls.ddev.toggle".into() }],
            sidebar: SidebarChange::AppendedRow,
        };
        save_owned(dir.path(), &owned).unwrap();
        assert_eq!(load_owned(dir.path()), owned);
    }

    #[test]
    fn describe_lists_every_change() {
        let lines = describe(&plan(&doc("")));
        assert_eq!(lines.len(), 4);
        assert!(lines[0].starts_with("add key prefix+shift+s"));
        assert!(lines[3].contains("$ddev"));
    }

    #[test]
    #[ignore = "needs herdr on PATH; run with cargo test -- --ignored"]
    fn applied_config_passes_herdr_config_check() {
        for original in [FIXTURE, ""] {
            let mut document = doc(original);
            let plan = plan(&document);
            apply(&mut document, &plan).unwrap();
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            std::fs::write(&path, document.to_string()).unwrap();
            let out = std::process::Command::new("herdr")
                .args(["config", "check"])
                .env("HERDR_CONFIG_PATH", &path)
                .output()
                .unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
        }
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test configure`
Expected: compile errors: `plan`, `apply`, `DocumentMut`, `SidebarChange` not found.

- [ ] **Step 4: Implement planning and editing**

Add above the tests in `src/configure.rs`:

```rust
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value, value};

use crate::app::{Ctx, unix_ms};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub key: &'static str,
    pub command: &'static str,
    pub description: &'static str,
}

pub const BINDINGS: [Binding; 3] = [
    Binding {
        key: "prefix+shift+s",
        command: "danjuls.ddev.toggle",
        description: "ddev: start/stop project",
    },
    Binding { key: "prefix+shift+o", command: "danjuls.ddev.open", description: "ddev: open site" },
    Binding { key: "prefix+shift+e", command: "danjuls.ddev.picker", description: "ddev: projects" },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SidebarChange {
    #[default]
    None,
    /// No `rows` existed: the defaults were written with `$ddev` added to the second row.
    CreatedRows,
    /// Custom rows existed: a row holding only `$ddev` was appended.
    AppendedRow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnedKey {
    pub key: String,
    pub command: String,
}

/// Exactly what configure added, so unconfigure removes nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Owned {
    pub keys: Vec<OwnedKey>,
    pub sidebar: SidebarChange,
}

impl Owned {
    pub fn merge(&mut self, other: Owned) {
        for key in other.keys {
            if !self.keys.contains(&key) {
                self.keys.push(key);
            }
        }
        if self.sidebar == SidebarChange::None {
            self.sidebar = other.sidebar;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub add: Vec<Binding>,
    pub skipped: Vec<String>,
    pub sidebar: SidebarChange,
}

fn norm(key: &str) -> String {
    key.trim().to_lowercase()
}

/// Every bound key and what it does: `[keys]` action values (string or list) and
/// `[[keys.command]]` entries.
pub fn used_keys(doc: &DocumentMut) -> Vec<(String, String)> {
    let mut used = Vec::new();
    let Some(keys) = doc.get("keys").and_then(Item::as_table_like) else {
        return used;
    };
    for (action, item) in keys.iter() {
        match item.as_value() {
            Some(Value::String(s)) => used.push((norm(s.value()), action.to_string())),
            Some(Value::Array(list)) => {
                for entry in list.iter().filter_map(Value::as_str) {
                    used.push((norm(entry), action.to_string()));
                }
            }
            _ => {}
        }
    }
    if let Some(commands) = keys.get("command").and_then(Item::as_array_of_tables) {
        for table in commands.iter() {
            if let Some(key) = table.get("key").and_then(Item::as_str) {
                let command = table.get("command").and_then(Item::as_str).unwrap_or("a command");
                used.push((norm(key), command.to_string()));
            }
        }
    }
    used
}

fn rows(doc: &DocumentMut) -> Option<&Array> {
    doc.get("ui")?.get("sidebar")?.get("spaces")?.get("rows")?.as_array()
}

fn rows_mut(doc: &mut DocumentMut) -> Option<&mut Array> {
    doc.get_mut("ui")?.get_mut("sidebar")?.get_mut("spaces")?.get_mut("rows")?.as_array_mut()
}

fn is_ddev_token(entry: &Value) -> bool {
    match entry {
        Value::String(s) => s.value() == "$ddev",
        Value::InlineTable(t) => t.get("token").and_then(Value::as_str) == Some("$ddev"),
        _ => false,
    }
}

fn has_ddev(rows: &Array) -> bool {
    rows.iter().filter_map(Value::as_array).any(|row| row.iter().any(is_ddev_token))
}

pub fn plan(doc: &DocumentMut) -> Plan {
    let used = used_keys(doc);
    let mut add = Vec::new();
    let mut skipped = Vec::new();
    for binding in BINDINGS {
        match used.iter().find(|(key, _)| *key == norm(binding.key)) {
            Some((_, what)) if what == binding.command => {
                skipped.push(format!("{} already runs {}", binding.key, binding.command));
            }
            Some((_, what)) => skipped.push(format!(
                "{} is already bound to {what}; bind {} to another key yourself",
                binding.key, binding.command
            )),
            None => add.push(binding),
        }
    }
    let sidebar = match rows(doc) {
        None => SidebarChange::CreatedRows,
        Some(rows) if has_ddev(rows) => SidebarChange::None,
        Some(_) => SidebarChange::AppendedRow,
    };
    Plan { add, skipped, sidebar }
}

pub fn describe(plan: &Plan) -> Vec<String> {
    let mut lines: Vec<String> =
        plan.add.iter().map(|b| format!("add key {} -> {}", b.key, b.command)).collect();
    lines.extend(plan.skipped.iter().map(|reason| format!("skip: {reason}")));
    lines.push(
        match plan.sidebar {
            SidebarChange::CreatedRows => "add the $ddev badge to the sidebar's second row",
            SidebarChange::AppendedRow => "add the $ddev badge as a new sidebar row",
            SidebarChange::None => "the sidebar already shows $ddev",
        }
        .to_string(),
    );
    lines
}

/// The `$ddev` token with colour rules (Catppuccin green and yellow, dim when stopped).
fn ddev_token() -> Value {
    let mut rules = Array::new();
    for (prefix, colour) in [("●", Some("#a6e3a1")), ("◐", Some("#f9e2af")), ("◌", Some("#f9e2af")), ("○", None)] {
        let mut rule = InlineTable::new();
        rule.insert("starts_with", prefix.into());
        match colour {
            Some(colour) => rule.insert("fg", colour.into()),
            None => rule.insert("dim", true.into()),
        };
        rules.push(rule);
    }
    let mut token = InlineTable::new();
    token.insert("token", "$ddev".into());
    token.insert("rules", Value::Array(rules));
    Value::InlineTable(token)
}

/// Herdr's default Space rows with `$ddev` added to the second one, one row per line.
fn default_rows_with_ddev() -> Array {
    let mut first = Array::new();
    first.push("state_icon");
    first.push("workspace");
    let mut second = Array::new();
    second.push("branch");
    second.push("git_status");
    second.push(ddev_token());
    let mut rows = Array::new();
    rows.push(first);
    rows.push(second);
    for row in rows.iter_mut() {
        row.decor_mut().set_prefix("\n  ");
    }
    rows.set_trailing("\n");
    rows.set_trailing_comma(true);
    rows
}

fn implicit_table() -> Item {
    let mut table = Table::new();
    table.set_implicit(true);
    Item::Table(table)
}

fn command_array(doc: &mut DocumentMut) -> Result<&mut ArrayOfTables> {
    let keys = doc
        .entry("keys")
        .or_insert(implicit_table())
        .as_table_mut()
        .context("`keys` in config.toml is not a table")?;
    keys.entry("command")
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()))
        .as_array_of_tables_mut()
        .context("`keys.command` in config.toml is not a list of tables")
}

fn spaces_table(doc: &mut DocumentMut) -> Result<&mut Table> {
    let ui = doc.entry("ui").or_insert(implicit_table()).as_table_mut().context("`ui` is not a table")?;
    let sidebar = ui
        .entry("sidebar")
        .or_insert(implicit_table())
        .as_table_mut()
        .context("`ui.sidebar` is not a table")?;
    sidebar
        .entry("spaces")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .context("`ui.sidebar.spaces` is not a table")
}

pub fn apply(doc: &mut DocumentMut, plan: &Plan) -> Result<Owned> {
    let mut owned = Owned { keys: Vec::new(), sidebar: plan.sidebar };
    if !plan.add.is_empty() {
        let commands = command_array(doc)?;
        for binding in &plan.add {
            let mut table = Table::new();
            table["key"] = value(binding.key);
            table["type"] = value("plugin_action");
            table["command"] = value(binding.command);
            table["description"] = value(binding.description);
            commands.push(table);
            owned.keys.push(OwnedKey {
                key: binding.key.to_string(),
                command: binding.command.to_string(),
            });
        }
    }
    match plan.sidebar {
        SidebarChange::None => {}
        SidebarChange::CreatedRows => {
            spaces_table(doc)?.insert("rows", value(default_rows_with_ddev()));
        }
        SidebarChange::AppendedRow => {
            let rows = rows_mut(doc).context("sidebar rows disappeared")?;
            let mut row = Array::new();
            row.push(ddev_token());
            rows.push(row);
        }
    }
    Ok(owned)
}

pub fn unapply(doc: &mut DocumentMut, owned: &Owned) {
    remove_owned_keys(doc, &owned.keys);
    match owned.sidebar {
        SidebarChange::None => {}
        SidebarChange::CreatedRows => remove_created_rows(doc),
        SidebarChange::AppendedRow => {
            if let Some(rows) = rows_mut(doc) {
                rows.retain(|row| {
                    !row.as_array().is_some_and(|r| r.len() == 1 && r.iter().all(is_ddev_token))
                });
            }
        }
    }
}

fn remove_owned_keys(doc: &mut DocumentMut, owned: &[OwnedKey]) {
    let Some(keys) = doc.get_mut("keys").and_then(Item::as_table_mut) else {
        return;
    };
    if let Some(commands) = keys.get_mut("command").and_then(Item::as_array_of_tables_mut) {
        let is_owned = |table: &Table| {
            owned.iter().any(|o| {
                table.get("key").and_then(Item::as_str) == Some(o.key.as_str())
                    && table.get("command").and_then(Item::as_str) == Some(o.command.as_str())
            })
        };
        let mut index = 0;
        while index < commands.len() {
            if commands.get(index).is_some_and(is_owned) {
                commands.remove(index);
            } else {
                index += 1;
            }
        }
        if commands.is_empty() {
            keys.remove("command");
        }
    }
    if keys.is_empty() {
        doc.remove("keys");
    }
}

fn remove_created_rows(doc: &mut DocumentMut) {
    let Some(ui) = doc.get_mut("ui").and_then(Item::as_table_mut) else {
        return;
    };
    if let Some(sidebar) = ui.get_mut("sidebar").and_then(Item::as_table_mut) {
        if let Some(spaces) = sidebar.get_mut("spaces").and_then(Item::as_table_mut) {
            spaces.remove("rows");
            if spaces.is_empty() {
                sidebar.remove("spaces");
            }
        }
        if sidebar.is_empty() {
            ui.remove("sidebar");
        }
    }
    if ui.is_empty() {
        doc.remove("ui");
    }
}

/// Herdr's config file: `HERDR_CONFIG_PATH`, else `~/.config/herdr/config.toml`.
pub fn herdr_config_path() -> PathBuf {
    std::env::var_os("HERDR_CONFIG_PATH").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config/herdr/config.toml")
    })
}

fn owned_path(state_dir: &Path) -> PathBuf {
    state_dir.join("owned.json")
}

pub fn load_owned(state_dir: &Path) -> Owned {
    fs::read_to_string(owned_path(state_dir))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save_owned(state_dir: &Path, owned: &Owned) -> Result<()> {
    fs::create_dir_all(state_dir)?;
    fs::write(owned_path(state_dir), serde_json::to_string_pretty(owned)?)?;
    Ok(())
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test configure`
Expected: all configure tests pass (the ignored one is skipped). If clippy flags the nested
`if let` in `remove_created_rows` as collapsible, leave it: the inner block has statements after
it, so it is not collapsible; if clippy still complains, follow its let-chain suggestion.

- [ ] **Step 6: Add the popup flows**

Append below `save_owned` in `src/configure.rs`:

```rust
fn read_config(path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(err).with_context(|| format!("cannot read {}", path.display())),
    }
}

fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

fn pause() -> Result<()> {
    print!("\nPress enter to close.");
    std::io::stdout().flush()?;
    std::io::stdin().read_line(&mut String::new())?;
    Ok(())
}

/// Back up, write, and run `herdr config check`; restore the old text if the check fails.
fn write_checked(ctx: &Ctx, path: &Path, original: &str, updated: &str) -> Result<bool> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let backup = path.with_extension(format!("toml.bak-{}", unix_ms() / 1000));
    fs::write(&backup, original)?;
    fs::write(path, updated)?;
    if ctx.herdr.config_check()? {
        println!("Backup: {}", backup.display());
        return Ok(true);
    }
    fs::write(path, original)?;
    println!("herdr config check failed, so your config was restored. Backup: {}", backup.display());
    Ok(false)
}

pub fn run_configure(ctx: &Ctx) -> Result<()> {
    let path = herdr_config_path();
    let original = read_config(&path)?;
    let mut doc: DocumentMut =
        original.parse().with_context(|| format!("{} is not valid TOML", path.display()))?;
    let plan = plan(&doc);
    println!("herdr-ddev configure - {}\n", path.display());
    for line in describe(&plan) {
        println!("  {line}");
    }
    if plan.add.is_empty() && plan.sidebar == SidebarChange::None {
        println!("\nNothing to change.");
        return pause();
    }
    if !confirm("\nApply these changes? [y/N] ")? {
        return Ok(());
    }
    let added = apply(&mut doc, &plan)?;
    if write_checked(ctx, &path, &original, &doc.to_string())? {
        let mut owned = load_owned(&ctx.state_dir);
        owned.merge(added);
        save_owned(&ctx.state_dir, &owned)?;
        let _ = ctx.herdr.reload_config();
        println!("Done. Herdr reloaded its config.");
    }
    pause()
}

pub fn run_unconfigure(ctx: &Ctx) -> Result<()> {
    let owned = load_owned(&ctx.state_dir);
    if owned.keys.is_empty() && owned.sidebar == SidebarChange::None {
        println!("herdr-ddev has not added anything to your Herdr config.");
        return pause();
    }
    let path = herdr_config_path();
    let original = read_config(&path)?;
    let mut doc: DocumentMut =
        original.parse().with_context(|| format!("{} is not valid TOML", path.display()))?;
    println!("herdr-ddev unconfigure - {}\n", path.display());
    for key in &owned.keys {
        println!("  remove key {} -> {}", key.key, key.command);
    }
    if owned.sidebar != SidebarChange::None {
        println!("  remove the $ddev badge from the sidebar");
    }
    if !confirm("\nRemove these? [y/N] ")? {
        return Ok(());
    }
    unapply(&mut doc, &owned);
    if write_checked(ctx, &path, &original, &doc.to_string())? {
        let _ = fs::remove_file(owned_path(&ctx.state_dir));
        let _ = ctx.herdr.reload_config();
        println!("Done. Herdr reloaded its config.");
    }
    pause()
}
```

- [ ] **Step 7: Wire the subcommands in `main.rs`**

Change `use herdr_ddev::{open, picker, ticker};` to
`use herdr_ddev::{configure, open, picker, ticker};` and add before `_ => bail!("{USAGE}")`:

```rust
        ["configure"] => configure::run_configure(&ctx),
        ["unconfigure"] => configure::run_unconfigure(&ctx),
```

- [ ] **Step 8: Run everything, including the herdr check**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test && cargo test -- --ignored`
Expected: all pass, including `applied_config_passes_herdr_config_check` (it writes only to a
temp dir through `HERDR_CONFIG_PATH`). If herdr rejects the sidebar token, the test output shows
herdr's message; fix `ddev_token()` to match and note it for Task 17's verify list.

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml Cargo.lock src tests
git commit -m "feat: add configure and unconfigure popups"
```

---

### Task 15: Plugin manifest, install script and link smoke test

**Files:**
- Create: `herdr-plugin.toml`, `scripts/install.sh`

**Interfaces:**
- Consumes: binary subcommands from Tasks 9, 12, 13, 14.
- Produces: an installable Herdr plugin; `bin/herdr-ddev` after the build step.

- [ ] **Step 1: Write the manifest**

`herdr-plugin.toml`:

```toml
id = "danjuls.ddev"
name = "ddev"
version = "0.1.0"
min_herdr_version = "0.9.1"
description = "ddev status badges, start/stop/open from a key, and a project picker"
platforms = ["macos", "linux"]

[[build]]
command = ["sh", "scripts/install.sh"]

[[startup]]
command = ["bin/herdr-ddev", "ticker", "--detach"]

[[actions]]
id = "toggle"
title = "ddev: start/stop project"
contexts = ["global"]
command = ["bin/herdr-ddev", "action", "toggle"]

[[actions]]
id = "start"
title = "ddev: start project"
contexts = ["global"]
command = ["bin/herdr-ddev", "action", "start"]

[[actions]]
id = "stop"
title = "ddev: stop project"
contexts = ["global"]
command = ["bin/herdr-ddev", "action", "stop"]

[[actions]]
id = "restart"
title = "ddev: restart project"
contexts = ["global"]
command = ["bin/herdr-ddev", "action", "restart"]

[[actions]]
id = "open"
title = "ddev: open site"
contexts = ["global"]
command = ["bin/herdr-ddev", "action", "open"]

[[actions]]
id = "picker"
title = "ddev: projects"
contexts = ["global"]
command = ["bin/herdr-ddev", "action", "picker"]

[[actions]]
id = "configure"
title = "ddev: add badge and keys to Herdr config"
contexts = ["global"]
command = ["bin/herdr-ddev", "action", "configure"]

[[actions]]
id = "unconfigure"
title = "ddev: remove badge and keys from Herdr config"
contexts = ["global"]
command = ["bin/herdr-ddev", "action", "unconfigure"]

[[panes]]
id = "picker"
title = "ddev projects"
placement = "popup"
width = "80%"
height = "70%"
command = ["bin/herdr-ddev", "picker"]

[[panes]]
id = "configure"
title = "ddev configure"
placement = "popup"
width = "80%"
height = "60%"
command = ["bin/herdr-ddev", "configure"]

[[panes]]
id = "unconfigure"
title = "ddev unconfigure"
placement = "popup"
width = "80%"
height = "60%"
command = ["bin/herdr-ddev", "unconfigure"]

[[panes]]
id = "url"
title = "ddev site URL"
placement = "popup"
width = "70%"
height = 12
command = ["bin/herdr-ddev", "url"]
```

- [ ] **Step 2: Write the install script**

`scripts/install.sh`:

```sh
#!/bin/sh
# Build step for `herdr plugin install`: fetch the release binary for this platform, verify
# its checksum, or build from source when there is no matching release.
set -eu

repo="danjuls/herdr-ddev"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' herdr-plugin.toml | head -n 1)

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  Linux-x86_64) target=x86_64-unknown-linux-musl ;;
  Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-musl ;;
  *) target="" ;;
esac

mkdir -p bin

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  else
    shasum -a 256 "$1" | cut -d ' ' -f 1
  fi
}

if [ -n "$target" ] && command -v curl >/dev/null 2>&1; then
  archive="herdr-ddev-$target.tar.gz"
  base="https://github.com/$repo/releases/download/v$version"
  tmp=$(mktemp -d)
  if curl -fsSL "$base/$archive" -o "$tmp/$archive" \
    && curl -fsSL "$base/$archive.sha256" -o "$tmp/$archive.sha256"; then
    expected=$(cut -d ' ' -f 1 "$tmp/$archive.sha256")
    if [ "$expected" = "$(sha256 "$tmp/$archive")" ]; then
      tar -xzf "$tmp/$archive" -C bin herdr-ddev
      chmod +x bin/herdr-ddev
      rm -rf "$tmp"
      echo "herdr-ddev: installed release v$version ($target)"
      exit 0
    fi
    echo "herdr-ddev: checksum mismatch for $archive, building from source" >&2
  fi
  rm -rf "$tmp"
fi

if command -v cargo >/dev/null 2>&1; then
  cargo build --release --locked
  cp target/release/herdr-ddev bin/herdr-ddev
  echo "herdr-ddev: built from source"
  exit 0
fi

echo "herdr-ddev: no release binary for $(uname -s)-$(uname -m) v$version and no cargo." >&2
echo "Install Rust from https://rustup.rs and reinstall the plugin." >&2
exit 1
```

Run: `chmod +x scripts/install.sh`

- [ ] **Step 3: Run the install script locally**

Run: `sh scripts/install.sh && bin/herdr-ddev --version`
Expected: no release exists yet, so it prints `herdr-ddev: built from source`, then
`herdr-ddev 0.1.0`.

- [ ] **Step 4: Link into Herdr and check the manifest**

This step uses Daniel's real Herdr session: plugin registration is per user. Badges are
display-only tokens that expire within 20 seconds of the plugin stopping.

Run: `herdr plugin link "$PWD" && herdr plugin action list --plugin danjuls.ddev`
Expected: 8 actions (`toggle`, `start`, `stop`, `restart`, `open`, `picker`, `configure`,
`unconfigure`).

- [ ] **Step 5: Smoke-test the picker and the ticker**

Run: `herdr plugin action invoke danjuls.ddev.picker`
Expected: the picker popup opens in Herdr and lists ddev projects; `esc` closes it. If the
popup fails to start with a "not found" error for `bin/herdr-ddev`, change every `command` in
`herdr-plugin.toml` to the form
`["sh", "-c", "exec \"$HERDR_PLUGIN_ROOT/bin/herdr-ddev\" picker"]` (same arguments), re-link,
and retry.

Then, with a Herdr workspace open in a ddev project folder, wait 10 seconds and run:
`herdr workspace list | jq -c '.result.workspaces[] | {label, tokens}'`
Expected: that workspace shows `"tokens":{"ddev":"● ddev"}` (or `○`/`◐`). Report the output.

- [ ] **Step 6: Commit**

```bash
git add herdr-plugin.toml scripts/install.sh
git commit -m "feat: add plugin manifest and install script"
```

---

### Task 16: Release pipeline

**Files:**
- Create: `.github/workflows/release.yml`, `scripts/check-version.sh`
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: `herdr-plugin.toml`, `Cargo.toml` versions, `scripts/install.sh` asset names
  (`herdr-ddev-<target>.tar.gz` plus `.sha256`).
- Produces: tagged releases with binaries for 4 targets.

- [ ] **Step 1: Write the version check**

`scripts/check-version.sh`:

```sh
#!/bin/sh
# Fail unless herdr-plugin.toml and Cargo.toml both carry the given version.
set -eu

wanted="$1"
manifest=$(sed -n 's/^version = "\(.*\)"/\1/p' herdr-plugin.toml | head -n 1)
cargo=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)

if [ "$manifest" != "$wanted" ] || [ "$cargo" != "$wanted" ]; then
  echo "version mismatch: wanted=$wanted herdr-plugin.toml=$manifest Cargo.toml=$cargo" >&2
  exit 1
fi
echo "version $wanted ok"
```

Run: `chmod +x scripts/check-version.sh`

- [ ] **Step 2: Verify it catches a mismatch**

Run: `sh scripts/check-version.sh 0.1.0; echo "exit=$?"; sh scripts/check-version.sh 9.9.9; echo "exit=$?"`
Expected: `version 0.1.0 ok`, `exit=0`, then a `version mismatch` line and `exit=1`.

- [ ] **Step 3: Add the check to CI**

Append to the `steps` in `.github/workflows/ci.yml`:

```yaml
      - run: sh scripts/check-version.sh "$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
```

- [ ] **Step 4: Write the release workflow**

`.github/workflows/release.yml`:

```yaml
name: release
on:
  push:
    tags: ["v*"]
permissions:
  contents: write
jobs:
  check-version:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: sh scripts/check-version.sh "${GITHUB_REF_NAME#v}"
  build:
    needs: check-version
    strategy:
      matrix:
        include:
          - { target: aarch64-apple-darwin, os: macos-latest }
          - { target: x86_64-apple-darwin, os: macos-latest }
          - { target: x86_64-unknown-linux-musl, os: ubuntu-latest }
          - { target: aarch64-unknown-linux-musl, os: ubuntu-latest }
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      - name: Install cross
        if: runner.os == 'Linux'
        run: cargo install cross --locked
      - name: Build
        run: |
          if [ "$RUNNER_OS" = "Linux" ]; then
            cross build --release --locked --target ${{ matrix.target }}
          else
            cargo build --release --locked --target ${{ matrix.target }}
          fi
      - name: Package
        run: |
          archive="herdr-ddev-${{ matrix.target }}.tar.gz"
          tar -czf "$archive" -C "target/${{ matrix.target }}/release" herdr-ddev
          shasum -a 256 "$archive" > "$archive.sha256"
      - uses: softprops/action-gh-release@v2
        with:
          files: |
            herdr-ddev-${{ matrix.target }}.tar.gz
            herdr-ddev-${{ matrix.target }}.tar.gz.sha256
```

- [ ] **Step 5: Commit**

```bash
git add .github scripts/check-version.sh
git commit -m "ci: build release binaries on version tags"
```

---

### Task 17: README, manual test checklist, spec touch-up

**Files:**
- Create: `README.md`, `docs/manual-test.md`
- Modify: `docs/superpowers/specs/2026-09-27-herdr-ddev-design.md`

**Interfaces:**
- Consumes: everything above.
- Produces: user documentation.

- [ ] **Step 1: Write the README**

`README.md`:

````markdown
# herdr-ddev

ddev for [Herdr](https://herdr.dev): see which workspace's ddev project is running, start,
stop and open it from a key, and jump between projects from a picker.

## What it does

- **Sidebar badge** on every workspace whose folder is a ddev project:

  | Badge | Meaning |
  |-------|---------|
  | `● ddev` | running |
  | `◐ ddev` | paused (containers exist but the web container is down) |
  | `○ ddev` | stopped |
  | `◌ ddev…` | starting, stopping or restarting |

  Badges follow changes made outside Herdr too (a `ddev stop` in a shell, Docker restarting)
  within a few seconds. A small background process polls Docker every 5 seconds; each poll
  takes about 0.04 seconds.
- **Keys** for the project behind the focused pane: start/stop, open the site.
- **Picker** listing every ddev project with its status, type and URL. Enter jumps to the
  project's workspace, or opens a new one there.
- **Notifications** when an action finishes, with ddev's error when it fails.

## Requirements

- Herdr 0.9.1 or newer, on macOS or Linux
- ddev, and a Docker CLI that can reach your Docker (Docker Desktop, OrbStack and Colima work)
- Rust, only if there is no prebuilt binary for your machine

## Install

```sh
herdr plugin install danjuls/herdr-ddev
herdr plugin action invoke danjuls.ddev.configure
```

`configure` opens a popup that shows exactly what it will add to `~/.config/herdr/config.toml`
(the sidebar badge and three keys), asks before writing, keeps a backup, checks the result
with `herdr config check`, and reloads Herdr. Keys you already use are skipped and listed.
`herdr plugin action invoke danjuls.ddev.unconfigure` removes exactly what it added.

## Keys

| Key | Action |
|-----|--------|
| `prefix+shift+s` | Start or stop the current project |
| `prefix+shift+o` | Open the current project's site |
| `prefix+shift+e` | Project picker |

In the picker: type to filter, arrows or `ctrl+n`/`ctrl+p` to move, `enter` to jump,
`ctrl+s` start/stop, `ctrl+r` restart, `ctrl+o` open, `esc` to clear the filter and close.

Every action can also be bound by hand, for example restart:

```toml
[[keys.command]]
key = "prefix+alt+r"
type = "plugin_action"
command = "danjuls.ddev.restart"
description = "ddev: restart project"
```

## Configuration

Optional. Create `config.toml` in the folder printed by
`herdr plugin config-dir danjuls.ddev`:

```toml
ddev_command = ["ddev"]      # the command that runs ddev
docker_command = ["docker"]  # the command that runs docker
poll_interval_secs = 5
open_mode = "auto"           # "auto" | "browser" | "clipboard"
```

ddev and docker are found on your PATH or in the usual install folders, so most setups need
no file at all.

### ddev inside distrobox

If Herdr runs inside the same distrobox as ddev, nothing is needed. If Herdr runs on the host:

```toml
ddev_command = ["distrobox", "enter", "-n", "dev", "--", "ddev"]
docker_command = ["distrobox", "enter", "-n", "dev", "--", "docker"]
poll_interval_secs = 15
```

Each wrapped call pays distrobox's startup time, hence the slower poll.

## Opening sites over SSH

`open_mode = "auto"` opens your browser when Herdr runs on your own desktop. Over SSH, or
without a display, it copies the URL instead: a small popup puts it on your clipboard and
shows it, so you can drag-select it if your terminal blocks clipboard access.

## Troubleshooting

| Problem | What to do |
|---------|------------|
| No badges at all | Run `configure` so the sidebar shows `$ddev`, then check that `docker ps` works in a shell |
| "ddev not found" or "docker not found" | Set `ddev_command` or `docker_command` in the config file |
| Badges disappear | Docker is not reachable; they come back when it is |
| A start or stop failed | The notification shows ddev's first error line; the full output is in `worker.log` |
| "already starting" | Another start/stop/restart for that project is still running |

Logs live in `~/.local/state/herdr/plugins/danjuls.ddev/`: `ticker.log` for the badge poller
and `worker.log` for start/stop/restart.

## Uninstall

```sh
herdr plugin action invoke danjuls.ddev.unconfigure
herdr plugin uninstall danjuls.ddev
```

## Development

```sh
cargo test
cargo test -- --ignored   # also checks generated config with `herdr config check`
sh scripts/install.sh     # builds bin/herdr-ddev (no release for unreleased versions)
herdr plugin link "$PWD"  # `link` does not run the build step
```

## License

MIT
````

- [ ] **Step 2: Write the manual test checklist**

`docs/manual-test.md`:

```markdown
# Manual test checklist

Things only a person can confirm. Run against a linked checkout (`herdr plugin link "$PWD"`)
after `sh scripts/install.sh`. Note the Herdr version and machine for each run.

## Setup

- [ ] `herdr plugin action invoke danjuls.ddev.configure` shows the plan, asks y/N, writes a
      backup and reloads Herdr; the sidebar shows `$ddev` on the second Space row
- [ ] Running configure again says "Nothing to change"

## Badges

- [ ] A workspace in a running ddev project shows a green `● ddev`
- [ ] `ddev stop` typed in a shell turns it into a dim `○ ddev` within ~5 seconds
- [ ] `ddev pause` shows a yellow `◐ ddev`
- [ ] A workspace outside ddev projects shows no badge
- [ ] Quitting Docker makes badges disappear within ~20 seconds; starting it brings them back

## Actions

- [ ] `prefix+shift+s` on a stopped project shows `◌ ddev…`, then `● ddev` and a "started"
      notification with the done sound
- [ ] Pressing it twice quickly shows "already starting"
- [ ] `prefix+shift+s` outside a project shows "Not in a ddev project"
- [ ] `prefix+shift+o` opens the site in the browser (local desktop)

## Picker

- [ ] `prefix+shift+e` lists all projects, running first, with type and URL
- [ ] Typing filters; `esc` clears, then closes
- [ ] `enter` jumps to the project's workspace; on a project without one it creates one
- [ ] `ctrl+s`, `ctrl+r`, `ctrl+o` act on the selected project and the status updates in place
- [ ] With vim-herdr-navigation installed, `ctrl+j`/`ctrl+k` inside the picker do not move the
      selection (record whether they move Herdr focus instead: spec verify item 6)

## Over SSH and remote

- [ ] Over SSH with `open_mode = "auto"`, open shows the URL popup; record whether the URL
      reached the local clipboard automatically (spec verify item 1)
- [ ] With `herdr --remote`, record which machine needs the sidebar row and which the keys
      (spec verify item 2), and add the result to the README

## Distrobox (Bazzite)

- [ ] With the distrobox wrappers configured, badges and actions work at
      `poll_interval_secs = 15`

## Remove

- [ ] `unconfigure` removes only herdr-ddev's keys and badge; the backup and the rest of the
      config are untouched
```

- [ ] **Step 3: Record the worker-log deviation in the spec**

In `docs/superpowers/specs/2026-09-27-herdr-ddev-design.md`, replace

```text
the first line of ddev's stderr; the full
output goes to stderr, which Herdr keeps in `herdr plugin log`.
```

with

```text
the first line of ddev's stderr; the full
output goes to `worker.log` in the plugin state dir, because the worker runs detached from Herdr.
```

and in the Error handling table replace
`Notification with the first stderr line; full output in `herdr plugin log``
with `Notification with the first stderr line; full output in `worker.log``.

- [ ] **Step 4: Check the docs for dashes and line length**

Run: `grep -nE '—|–' README.md docs/manual-test.md; awk 'length > 100 && !/^\|/ {print FILENAME":"NR}' README.md docs/manual-test.md`
Expected: no output (tables are exempt from the line check).

- [ ] **Step 5: Commit**

```bash
git add README.md docs
git commit -m "docs: add README and manual test checklist"
```

---

## After the plan: releasing v0.1.0 (with Daniel)

Not part of implementation; each step is outward-facing and needs Daniel's go-ahead.

1. Daniel runs `docs/manual-test.md` and provides screenshots; add them to the README.
2. `gh repo edit danjuls/herdr-ddev --visibility public --accept-visibility-change-consequences`
3. `gh repo edit danjuls/herdr-ddev --add-topic herdr-plugin`
4. `git tag v0.1.0 && git push origin v0.1.0`, then confirm the release has 8 assets.
5. `herdr plugin unlink danjuls.ddev && herdr plugin install danjuls/herdr-ddev` and check the
   log says `installed release v0.1.0`.
6. Within 30 minutes the plugin appears on https://herdr.dev/plugins/.
