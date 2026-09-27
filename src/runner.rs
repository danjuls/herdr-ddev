//! The only place that spawns ddev, docker and herdr, so tests can script them.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

/// How long a docker, herdr or ddev lookup may take before it counts as hung.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

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
        Output {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    /// A failed run printing `stderr`.
    pub fn fail(stderr: &str) -> Output {
        Output {
            success: false,
            stdout: String::new(),
            stderr: stderr.to_string(),
        }
    }
}

pub trait Runner {
    /// Run `argv` (program first) in `cwd` and wait for it, within the runner's time limit.
    fn run(&self, argv: &[String], cwd: Option<&Path>) -> Result<Output>;

    /// Like `run`, without a time limit: for `ddev start`, which can take minutes.
    fn run_long(&self, argv: &[String], cwd: Option<&Path>) -> Result<Output> {
        self.run(argv, cwd)
    }

    /// Start a program without waiting for it or keeping its output (browser openers).
    fn spawn_detached(&self, argv: &[String]) -> Result<()>;
}

/// Runs real processes with a fixed PATH and, for `run`, a time limit.
pub struct SystemRunner {
    path: String,
    timeout: Option<Duration>,
}

impl SystemRunner {
    pub fn new(path: String) -> SystemRunner {
        SystemRunner::with_timeout(path, Some(DEFAULT_TIMEOUT))
    }

    pub fn with_timeout(path: String, timeout: Option<Duration>) -> SystemRunner {
        SystemRunner { path, timeout }
    }

    fn execute(
        &self,
        argv: &[String],
        cwd: Option<&Path>,
        limit: Option<Duration>,
    ) -> Result<Output> {
        let (program, args) = argv.split_first().context("empty command")?;
        let mut command = Command::new(program);
        command
            .args(args)
            .env("PATH", &self.path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = cwd {
            command.current_dir(dir);
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("could not run {program}"))?;
        let stdout = read_in_background(child.stdout.take());
        let stderr = read_in_background(child.stderr.take());
        let status = match limit {
            None => child.wait()?,
            Some(limit) => {
                let deadline = Instant::now() + limit;
                loop {
                    if let Some(status) = child.try_wait()? {
                        break status;
                    }
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        bail!("{program} timed out after {limit:?}");
                    }
                    thread::sleep(Duration::from_millis(10));
                }
            }
        };
        Ok(Output {
            success: status.success(),
            stdout: stdout.join().unwrap_or_default(),
            stderr: stderr.join().unwrap_or_default(),
        })
    }
}

/// Drain a pipe on its own thread so a chatty child cannot fill it and stall.
fn read_in_background(pipe: Option<impl Read + Send + 'static>) -> JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

impl Runner for SystemRunner {
    fn run(&self, argv: &[String], cwd: Option<&Path>) -> Result<Output> {
        self.execute(argv, cwd, self.timeout)
    }

    fn run_long(&self, argv: &[String], cwd: Option<&Path>) -> Result<Output> {
        self.execute(argv, cwd, None)
    }

    fn spawn_detached(&self, argv: &[String]) -> Result<()> {
        let (program, args) = argv.split_first().context("empty command")?;
        let mut child = Command::new(program)
            .args(args)
            .env("PATH", &self.path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .with_context(|| format!("could not run {program}"))?;
        thread::spawn(move || child.wait());
        Ok(())
    }
}

/// One call recorded by the test runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub detached: bool,
    /// Made through `run_long` (no time limit).
    pub long: bool,
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
            self.rules.borrow_mut().push(Rule {
                prefix,
                output,
                once,
            });
            self
        }

        pub fn calls(&self) -> Vec<Call> {
            self.calls.borrow().clone()
        }

        pub fn calls_starting_with(&self, prefix: &[&str]) -> Vec<Call> {
            self.calls()
                .into_iter()
                .filter(|call| starts_with(&call.argv, prefix))
                .collect()
        }
    }

    fn starts_with(argv: &[String], prefix: &[impl AsRef<str>]) -> bool {
        argv.len() >= prefix.len() && argv.iter().zip(prefix).all(|(a, p)| a == p.as_ref())
    }

    impl Runner for FakeRunner {
        fn run(&self, argv: &[String], cwd: Option<&Path>) -> Result<Output> {
            let call = Call {
                argv: argv.to_vec(),
                cwd: cwd.map(Path::to_path_buf),
                detached: false,
                long: false,
            };
            self.calls.borrow_mut().push(call);
            let mut rules = self.rules.borrow_mut();
            let Some(index) = rules
                .iter()
                .position(|rule| starts_with(argv, &rule.prefix))
            else {
                bail!("FakeRunner: no rule for {argv:?}");
            };
            let output = rules[index].output.clone();
            if rules[index].once {
                rules.remove(index);
            }
            Ok(output)
        }

        fn run_long(&self, argv: &[String], cwd: Option<&Path>) -> Result<Output> {
            let output = self.run(argv, cwd);
            if let Some(last) = self.calls.borrow_mut().last_mut() {
                last.long = true;
            }
            output
        }

        fn spawn_detached(&self, argv: &[String]) -> Result<()> {
            let call = Call {
                argv: argv.to_vec(),
                cwd: None,
                detached: true,
                long: false,
            };
            self.calls.borrow_mut().push(call);
            Ok(())
        }
    }
}

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
    fn system_runner_times_out_hung_commands() {
        let runner = SystemRunner::with_timeout(
            std::env::var("PATH").unwrap_or_default(),
            Some(std::time::Duration::from_millis(200)),
        );
        let started = std::time::Instant::now();
        let err = runner
            .run(&argv(&["sh", "-c", "sleep 5"]), None)
            .unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err}");
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn run_long_has_no_time_limit() {
        let runner = SystemRunner::with_timeout(
            std::env::var("PATH").unwrap_or_default(),
            Some(std::time::Duration::from_millis(100)),
        );
        let out = runner
            .run_long(&argv(&["sh", "-c", "sleep 0.3; echo done"]), None)
            .unwrap();
        assert_eq!(out.stdout, "done\n");
    }

    #[test]
    fn spawn_detached_does_not_wait() {
        let runner = SystemRunner::new(std::env::var("PATH").unwrap_or_default());
        let started = std::time::Instant::now();
        runner
            .spawn_detached(&argv(&["sh", "-c", "sleep 3"]))
            .unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn fake_runner_records_detached_spawns() {
        let fake = FakeRunner::new();
        fake.spawn_detached(&argv(&["open", "https://x"])).unwrap();
        let call = &fake.calls()[0];
        assert!(call.detached);
        assert_eq!(call.argv, ["open", "https://x"]);
    }

    #[test]
    fn fake_runner_matches_prefix_and_records_calls() {
        let fake = FakeRunner::new();
        fake.on(&["docker", "ps"], Output::ok("rows"));
        let out = fake
            .run(&argv(&["docker", "ps", "-a"]), Some(Path::new("/x")))
            .unwrap();
        assert_eq!(out.stdout, "rows");
        assert_eq!(fake.calls()[0].cwd.as_deref(), Some(Path::new("/x")));
        assert!(fake.run(&argv(&["ddev", "list"]), None).is_err());
        assert_eq!(fake.calls_starting_with(&["docker"]).len(), 1);
    }

    #[test]
    fn fake_runner_once_rules_are_used_up() {
        let fake = FakeRunner::new();
        fake.once(&["docker"], Output::ok("first"))
            .on(&["docker"], Output::ok("later"));
        assert_eq!(fake.run(&argv(&["docker"]), None).unwrap().stdout, "first");
        assert_eq!(fake.run(&argv(&["docker"]), None).unwrap().stdout, "later");
    }
}
