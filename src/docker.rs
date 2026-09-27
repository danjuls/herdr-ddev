//! ddev projects as Docker sees them: one `docker ps` per tick (~0.04s against ~1.3s for
//! `ddev list`).

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
    for arg in [
        "ps",
        "-a",
        "--filter",
        "label=com.ddev.site-name",
        "--format",
        PS_FORMAT,
    ] {
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
        let entry = projects
            .entry(name.to_string())
            .or_insert_with(|| (PathBuf::from(root), false));
        if service == "web" && state == "running" {
            entry.1 = true;
        }
    }
    projects
        .into_iter()
        .map(|(name, (root, web_running))| Project {
            name,
            root,
            state: if web_running {
                State::Running
            } else {
                State::Paused
            },
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
        assert_eq!(
            parse_ps(PS)[0].root,
            PathBuf::from("/home/dev/My Sites/blog")
        );
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
        fake.on(
            &["docker"],
            Output::fail("Cannot connect to the Docker daemon"),
        );
        let err = query(&fake, &["docker".to_string()]).unwrap_err();
        assert!(err.to_string().contains("Cannot connect"));
    }

    #[test]
    fn wrapped_docker_command_keeps_the_wrapper_first() {
        let wrapper: Vec<String> = ["distrobox", "enter", "--", "docker"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let argv = ps_argv(&wrapper);
        assert_eq!(&argv[..5], ["distrobox", "enter", "--", "docker", "ps"]);
    }
}
