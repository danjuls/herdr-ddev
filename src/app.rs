//! What every command needs: how to run things, where state lives, which binaries to use.

use std::env;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::config::{self, Config, OpenMode};
use crate::herdr::Herdr;
use crate::runner::{Runner, SystemRunner};
use crate::{bins, busy, detach};

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
    /// Which Herdr session this is (from its socket), so each session gets its own ticker.
    pub session: String,
    /// The installed or linked plugin folder, when Herdr says (`HERDR_PLUGIN_ROOT`).
    pub plugin_root: Option<PathBuf>,
}

/// A short, stable, file-name-safe key for a Herdr session: FNV-1a of its socket path, or
/// `default` when Herdr did not say.
pub fn session_key(socket_path: Option<&str>) -> String {
    let Some(path) = socket_path.filter(|p| !p.is_empty()) else {
        return "default".to_string();
    };
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

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
    pub session: String,
    pub plugin_root: Option<PathBuf>,
}

impl App {
    /// Outside Herdr (development) state and config fall back to `~/.local/state/herdr-ddev`.
    pub fn from_env() -> Result<App> {
        let home = env::var("HOME").unwrap_or_default();
        let fallback = PathBuf::from(&home).join(".local/state/herdr-ddev");
        let dir_var = |name: &str| {
            env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
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
        let binaries = [
            ddev.as_deref().unwrap_or(&[]),
            docker.as_deref().unwrap_or(&[]),
        ];
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
            session: session_key(non_empty("HERDR_SOCKET_PATH").as_deref()),
            plugin_root: dir_var("HERDR_PLUGIN_ROOT"),
        })
    }

    /// Start `herdr-ddev <args>` in the background, logging to ticker.log or worker.log.
    pub fn spawn(&self, args: &[String]) -> Result<()> {
        let is_ticker = args.first().map(String::as_str) == Some("ticker");
        let log = self.state_dir.join(if is_ticker {
            "ticker.log"
        } else {
            "worker.log"
        });
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
            session: self.session.clone(),
            plugin_root: self.plugin_root.clone(),
        }
    }
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
            session: "test".to_string(),
            plugin_root: None,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn session_key_is_stable_file_safe_and_has_a_default() {
        let key = super::session_key(Some("/Users/x/.config/herdr/herdr.sock"));
        assert_eq!(
            key,
            super::session_key(Some("/Users/x/.config/herdr/herdr.sock"))
        );
        assert_ne!(key, super::session_key(Some("/tmp/other.sock")));
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()), "{key}");
        assert_eq!(super::session_key(None), "default");
        assert_eq!(super::session_key(Some("")), "default");
    }
}
