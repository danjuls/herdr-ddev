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
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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
