//! Which ddev project a folder belongs to.

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
            return if rest.as_os_str().is_empty() {
                real
            } else {
                real.join(rest)
            };
        }
    }
    clean
}

/// Docker's projects with canonical roots, ready for matching.
pub fn canonical_projects(projects: Vec<Project>) -> Vec<Project> {
    projects
        .into_iter()
        .map(|p| Project {
            root: canonical(&p.root),
            ..p
        })
        .collect()
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
    dir.ancestors()
        .find(|d| d.join(".ddev").join("config.yaml").is_file())
        .map(Path::to_path_buf)
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
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("config.") && name.ends_with(".yaml") && name != "config.yaml"
                })
        })
        .collect();
    overrides.sort();
    let mut name = None;
    for file in std::iter::once(ddev.join("config.yaml")).chain(overrides) {
        if let Some(found) = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| name_line(&t))
        {
            name = Some(found);
        }
    }
    name.unwrap_or_else(|| {
        root.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
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
        .rfind(|value| !value.is_empty())
}

/// Stopped projects found by walking up to `.ddev/config.yaml`, cached per folder.
pub struct StoppedCache {
    ttl: Duration,
    entries: HashMap<PathBuf, (Instant, Option<Project>)>,
}

impl StoppedCache {
    pub fn new(ttl: Duration) -> StoppedCache {
        StoppedCache {
            ttl,
            entries: HashMap::new(),
        }
    }

    /// The stopped project owning `dir` (canonical), from cache while younger than the TTL.
    pub fn lookup(&mut self, dir: &Path, now: Instant) -> Option<Project> {
        if let Some((at, found)) = self.entries.get(dir)
            && now.duration_since(*at) < self.ttl
        {
            return found.clone();
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
        (Some(d), Some(c)) if c.root.components().count() > d.root.components().count() => Some(c),
        (Some(d), _) => Some(d.clone()),
        (None, c) => c,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_project(root: &Path, config: &str) {
        fs::create_dir_all(root.join(".ddev")).unwrap();
        fs::write(root.join(".ddev/config.yaml"), config).unwrap();
    }

    fn running(name: &str, root: &Path) -> Project {
        Project {
            name: name.to_string(),
            root: canonical(root),
            state: State::Running,
        }
    }

    fn cache() -> StoppedCache {
        StoppedCache::new(Duration::from_secs(60))
    }

    #[test]
    fn longest_root_wins_for_nested_projects() {
        let parent = Project {
            name: "parent".into(),
            root: "/w/site".into(),
            state: State::Running,
        };
        let child = Project {
            name: "child".into(),
            root: "/w/site/sub".into(),
            state: State::Running,
        };
        let projects = [parent, child];
        assert_eq!(
            best_match(Path::new("/w/site/sub/web"), &projects)
                .unwrap()
                .name,
            "child"
        );
        assert_eq!(
            best_match(Path::new("/w/site/other"), &projects)
                .unwrap()
                .name,
            "parent"
        );
        assert!(best_match(Path::new("/w/sitex"), &projects).is_none());
    }

    #[test]
    fn canonical_strips_trailing_slash_for_missing_paths() {
        assert_eq!(
            canonical(Path::new("/no/such/dir/")),
            PathBuf::from("/no/such/dir")
        );
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
        fs::write(
            tmp.path().join(".ddev/config.local.yaml"),
            "name: \"shop-local\" # mine\n",
        )
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
        let found = resolve(
            &tmp.path().join("web"),
            &docker,
            &mut cache(),
            Instant::now(),
        );
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
        assert_eq!(
            (found.name.as_str(), found.state),
            ("child", State::Stopped)
        );
    }

    #[test]
    fn folder_outside_any_project_resolves_to_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(resolve(tmp.path(), &[], &mut cache(), Instant::now()).is_none());
    }

    #[test]
    fn canonical_projects_canonicalizes_roots() {
        let tmp = tempfile::tempdir().unwrap();
        let raw = Project {
            name: "shop".into(),
            root: tmp.path().join("./"),
            state: State::Running,
        };
        assert_eq!(canonical_projects(vec![raw])[0].root, canonical(tmp.path()));
    }
}
