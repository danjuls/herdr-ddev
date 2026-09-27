//! The optional plugin config file in Herdr's per-plugin config folder.

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
            Err(err) => (
                Config::default(),
                Some(format!("{}: {err}", path.display())),
            ),
        },
        Err(err) if err.kind() == ErrorKind::NotFound => (Config::default(), None),
        Err(err) => (
            Config::default(),
            Some(format!("{}: {err}", path.display())),
        ),
    }
}

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
        assert!(
            parse("poll_interval_secs = 0")
                .unwrap_err()
                .contains("at least 1")
        );
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
