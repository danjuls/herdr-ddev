//! Starting the ticker and workers so they outlive the Herdr hook or action that started them.

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
