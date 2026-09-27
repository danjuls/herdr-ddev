//! Busy markers: one file per project while a start, stop or restart runs.

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
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = if safe.is_empty() || safe.starts_with('.') {
        format!("_{safe}")
    } else {
        safe
    };
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
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        std::process::id(),
        marker.started_unix
    ));
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
        Marker {
            project: project.to_string(),
            verb,
            pid: 42,
            started_unix,
        }
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
