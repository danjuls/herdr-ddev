//! The only place that spawns ddev, docker and herdr, so tests can script them.

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
        let output = command
            .output()
            .with_context(|| format!("could not run {program}"))?;
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
