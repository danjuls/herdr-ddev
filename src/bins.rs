//! Finding ddev and docker when Herdr's server started with a bare PATH.

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
    let fixed = candidates
        .iter()
        .map(|candidate| expand_home(candidate, home));
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
    let inherited_dirs = std::env::split_paths(inherited).filter(|dir| !dir.as_os_str().is_empty());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn exists_in(paths: &'static [&'static str]) -> impl Fn(&Path) -> bool {
        move |path| paths.iter().any(|p| Path::new(p) == path)
    }

    #[test]
    fn configured_command_wins() {
        let configured: Vec<String> = ["distrobox", "enter", "--", "ddev"]
            .iter()
            .map(|s| s.to_string())
            .collect();
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
        let found = resolve(
            Some(empty.as_slice()),
            "ddev",
            DDEV_CANDIDATES,
            "/p",
            "/home/u",
            &is_exec,
        );
        assert_eq!(found, Some(vec!["/p/ddev".to_string()]));
    }

    #[test]
    fn path_is_searched_before_candidates() {
        let is_exec = exists_in(&["/custom/bin/ddev", "/usr/bin/ddev"]);
        let found = resolve(
            None,
            "ddev",
            DDEV_CANDIDATES,
            "/nothing:/custom/bin",
            "/home/u",
            &is_exec,
        );
        assert_eq!(found, Some(vec!["/custom/bin/ddev".to_string()]));
    }

    #[test]
    fn candidates_expand_home() {
        let is_exec = exists_in(&["/home/u/.orbstack/bin/docker"]);
        let found = resolve(None, "docker", DOCKER_CANDIDATES, "", "/home/u", &is_exec);
        assert_eq!(
            found,
            Some(vec!["/home/u/.orbstack/bin/docker".to_string()])
        );
    }

    #[test]
    fn nothing_found_gives_none() {
        assert_eq!(
            resolve(None, "ddev", DDEV_CANDIDATES, "/x", "/home/u", &|_| false),
            None
        );
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
