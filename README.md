# herdr-ddev

ddev for [Herdr](https://herdr.dev): see which workspace's ddev project is running, start,
stop and open it from a key, and jump between projects from a picker.

## What it does

- **Sidebar badge** on every workspace whose folder is a ddev project:

  | Badge | Meaning |
  |-------|---------|
  | `● ddev` | running |
  | `◐ ddev` | paused (containers exist but the web container is down) |
  | `○ ddev` | stopped |
  | `◌ ddev…` | starting, stopping or restarting |

  Badges follow changes made outside Herdr too (a `ddev stop` in a shell, Docker restarting)
  within a few seconds. A small background process polls Docker every 5 seconds; each poll
  takes about 0.04 seconds.
- **Keys** for the project behind the focused pane: start/stop, open the site.
- **Picker** listing every ddev project with its status, type and URL. Enter jumps to the
  project's workspace, or opens a new one there.
- **Notifications** when an action finishes, with ddev's error when it fails.

## Requirements

- Herdr 0.9.1 or newer, on macOS or Linux
- ddev, and a Docker CLI that can reach your Docker (Docker Desktop, OrbStack and Colima work)
- Rust, only if there is no prebuilt binary for your machine

## Install

```sh
herdr plugin install danjuls/herdr-ddev
herdr plugin action invoke danjuls.ddev.configure
```

`configure` opens a popup that shows exactly what it will add to `~/.config/herdr/config.toml`
(the sidebar badge and three keys), asks before writing, keeps a backup, checks the result
with `herdr config check`, and reloads Herdr. Keys you already use are skipped and listed.
Its additions go in one marked block at the end of the file, so your comments stay where they
are.
`herdr plugin action invoke danjuls.ddev.unconfigure` removes exactly what it added.

## Keys

| Key | Action |
|-----|--------|
| `prefix+shift+s` | Start or stop the current project |
| `prefix+shift+o` | Open the current project's site |
| `prefix+shift+e` | Project picker |

In the picker: type to filter, arrows or `ctrl+n`/`ctrl+p` to move, `enter` to jump,
`ctrl+s` start/stop, `ctrl+r` restart, `ctrl+o` open, `esc` to clear the filter and close.

Every action can also be bound by hand, for example restart:

```toml
[[keys.command]]
key = "prefix+alt+r"
type = "plugin_action"
command = "danjuls.ddev.restart"
description = "ddev: restart project"
```

## Configuration

Optional. Create `config.toml` in the folder printed by
`herdr plugin config-dir danjuls.ddev`:

```toml
ddev_command = ["ddev"]      # the command that runs ddev
docker_command = ["docker"]  # the command that runs docker
poll_interval_secs = 5
open_mode = "auto"           # "auto" | "browser" | "clipboard"
```

ddev and docker are found on your PATH or in the usual install folders, so most setups need
no file at all.

### ddev inside distrobox

If Herdr runs inside the same distrobox as ddev, nothing is needed. If Herdr runs on the host:

```toml
ddev_command = ["distrobox", "enter", "-n", "dev", "--", "ddev"]
docker_command = ["distrobox", "enter", "-n", "dev", "--", "docker"]
poll_interval_secs = 15
```

Each wrapped call pays distrobox's startup time, hence the slower poll.

## Opening sites over SSH

`open_mode = "auto"` opens your browser when Herdr runs on your own desktop. Over SSH, or
without a display, it copies the URL instead: a small popup puts it on your clipboard and
shows it, so you can drag-select it if your terminal blocks clipboard access.

## Troubleshooting

| Problem | What to do |
|---------|------------|
| No badges at all | Run `configure` so the sidebar shows `$ddev`, then check that `docker ps` works in a shell |
| "ddev not found" or "docker not found" | Set `ddev_command` or `docker_command` in the config file |
| Badges disappear | Docker is not reachable; they come back when it is |
| A start or stop failed | The notification shows ddev's first error line; the full output is in `worker.log` |
| "already starting" | Another start/stop/restart for that project is still running |

Logs live in `~/.local/state/herdr/plugins/danjuls.ddev/`: `ticker.log` for the badge poller
and `worker.log` for start/stop/restart.

## Uninstall

```sh
herdr plugin action invoke danjuls.ddev.unconfigure
herdr plugin uninstall danjuls.ddev
```

## Development

```sh
cargo test
cargo test -- --ignored   # also checks generated config with `herdr config check`
sh scripts/install.sh     # builds bin/herdr-ddev (no release for unreleased versions)
herdr plugin link "$PWD"  # `link` does not run the build step
```

## License

MIT
