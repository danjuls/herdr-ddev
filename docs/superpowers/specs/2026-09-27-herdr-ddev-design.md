# herdr-ddev design

Date: 2026-09-27 · Status: design approved in brainstorming, spec awaiting review

## Intent

**Outcome.** A Herdr plugin that brings ddev into Herdr the way norns-companion does for tmux:
see which workspace's ddev project is running, start/stop/open it from a key, and jump between
projects from a picker.

**Who.** Public-first: any Herdr user who runs ddev. Daniel's two machines (macOS, and a
Bazzite box where ddev runs inside distrobox) are the first test cases, not the target.
When this started, none of the 1,291 marketplace plugins handled ddev.

**Success criteria.**

- A stranger can `herdr plugin install danjuls/herdr-ddev`, run the configure popup, and get
  working badges and keys with no config file on a standard Docker Desktop, OrbStack or Colima
  setup.
- Daniel can drop norns-companion's ddev features after moving to Herdr.
- Status badges stay correct when ddev is changed outside the plugin (a `ddev stop` typed in a
  shell, `ddev poweroff`, Docker restarting), within one poll interval.
- Polling cost is negligible on a laptop.

**Decisions made in brainstorming.**

| Topic | Decision |
|-------|----------|
| Audience | Public-first |
| v1 scope | Parity with norns-companion only |
| Language | Rust, single binary |
| Freshness | Light polling ticker that queries Docker container labels |
| Plugin id | `danjuls.ddev` (actions become `danjuls.ddev.<action>`) |
| Default keys | `prefix+shift+s` start/stop, `prefix+shift+o` open, `prefix+shift+e` picker |
| License | MIT |
| Repo | `danjuls/herdr-ddev`, private until v0.1, then public with topic `herdr-plugin` |

## Scope

**In v1** (norns-companion parity):

- Per-workspace ddev status badge in the Herdr sidebar.
- Start/stop toggle, restart, open site for the project behind the focused pane.
- Project picker popup: list, filter, jump to or create the project's workspace, act on it.
- Configure/unconfigure popups that add and remove the badge and keybindings.

**Out of scope** (v0.2 candidates, decided by real use and issues): `ddev logs` pane,
shell in the web container, drush shortcuts, auto-start on workspace open, push-based updates
(`docker events`), Windows.

## Architecture

```text
herdr-ddev/
  herdr-plugin.toml        manifest: build, startup, actions, panes
  Cargo.toml               one binary: herdr-ddev
  scripts/install.sh       build step: download release binary, else cargo build
  src/
    main.rs                subcommand dispatch
    runner.rs              Runner trait: the only place that spawns processes
    docker.rs              container label query + parsing
    project.rs             folder -> project matching, .ddev/config*.yaml name lookup
    herdr.rs               typed wrappers over herdr CLI calls (pane list, report-metadata, ...)
    ticker.rs              polling loop, lock, backoff
    badge.rs               project state -> badge text
    actions.rs             toggle/start/stop/restart/open, detached worker, busy markers
    picker.rs              picker popup (TUI)
    configure.rs           configure/unconfigure popups, config.toml editing
    config.rs              plugin config file
    open.rs                open_mode decision, browser/clipboard
  docs/manual-test.md      checks only a person can confirm
  .github/workflows/       ci.yml (fmt, clippy, test), release.yml (tagged builds)
```

**Manifest sketch** (field names per Herdr 0.9.1 plugin docs):

```toml
id = "danjuls.ddev"
name = "ddev"
version = "0.1.0"
min_herdr_version = "0.9.1"
description = "ddev status badges, start/stop/open from a key, and a project picker"
platforms = ["macos", "linux"]

[[build]]
command = ["sh", "scripts/install.sh"]

[[startup]]
command = ["bin/herdr-ddev", "ticker", "--detach"]

[[actions]]
id = "toggle"
title = "ddev: start/stop project"
contexts = ["global"]
command = ["bin/herdr-ddev", "action", "toggle"]
# also: start, stop, restart, open, picker (opens the picker pane)

[[panes]]
id = "picker"
title = "ddev projects"
placement = "popup"
width = "80%"
height = "70%"
command = ["bin/herdr-ddev", "picker"]
# also: configure, unconfigure, url (see Open site)
```

**Process boundary.** Every call to `ddev`, `docker` or `herdr` goes through the `Runner`
trait (`run(argv, cwd) -> Output`). Production uses `std::process::Command` with the widened
PATH; tests use a scripted fake. No other module spawns processes.

## Project detection

**Running and paused projects** come from one Docker call per tick (~0.04s, measured against
~1.3s for `ddev list`):

```sh
docker ps -a --filter label=com.ddev.site-name \
  --format '{{.Label "com.ddev.site-name"}}\t{{.Label "com.ddev.approot"}}\t{{.State}}\t{{.Label "com.docker.compose.service"}}'
```

Per project: `running` if its `web` container is running, `paused` if containers exist but none
is running. Rows without a site name (router, ssh-agent) are ignored.

**Stopped projects** have no containers. A folder belongs to a stopped project when walking up
from it finds `.ddev/config.yaml`. The project name is the last `name:` found in `config.yaml`
then `config.*.yaml` in lexical order (ddev's merge order), else the project folder's
basename (ddev's default). Results are cached per folder for 60 seconds.

**Paths** are canonicalized before comparing (macOS `/tmp` is `/private/tmp`). A folder matches
a project when it equals the project root or sits inside it. The longest root wins, so a nested
project beats its parent (same rule as norns-companion `matchProject()`).

**Workspace to project.** Each pane contributes `foreground_cwd`, falling back to `cwd`, from
`herdr pane list`. The workspace's project is the one matched by the most panes; ties go to the
focused pane's match, then the first pane. A workspace with no match gets no badge.

## Status ticker

**Lifecycle.** Started by the `[[startup]]` hook with `--detach`: it re-launches itself in the
background (own process group, no terminal, stdio to `$HERDR_PLUGIN_STATE_DIR/ticker.log`,
truncated at 1 MB) and the hook exits.
Because startup hooks do not run on `plugin link` or `plugin enable`, every action and popup also
starts the ticker if it is not running. One ticker per machine: it holds an exclusive lock on
`$HERDR_PLUGIN_STATE_DIR/ticker.lock` (`std::fs::File::try_lock`), which the OS releases if the
process dies, so there are no stale locks.

**Each tick** (default every 5s):

1. Query Docker (above) and `herdr pane list`.
2. Compute each workspace's project and state; busy markers (see Actions) override the state.
3. Report changed badges, and re-report unchanged ones once half their TTL has passed:
   `herdr workspace report-metadata <ws> --source danjuls.ddev --token ddev=<badge> --seq <ms>
   --ttl-ms <ttl>`. TTL is `max(4 x interval, 20s)`, so badges disappear on their own if the
   ticker dies. `--seq` is Unix time in milliseconds, so a restarted ticker is never treated as
   stale.
4. Clear the token (`--clear-token ddev`) on workspaces that no longer match a project.

**Backoff.** When Docker is unreachable, the interval grows 5s, 10s, 20s, 30s and resets on the
first success. The ticker exits after 3 consecutive failed `herdr` calls (server gone).

## Badge

| State | Token value | Suggested style |
|-------|-------------|-----------------|
| running | `● ddev` | green |
| paused | `◐ ddev` | yellow |
| stopped | `○ ddev` | dim |
| starting, stopping, restarting | `◌ ddev…` | yellow |

Herdr renders only tokens the user adds to `[ui.sidebar.spaces] rows`. Configure adds the styled
token to the second default row, or appends it as its own row when the rows are customized:

```toml
[ui.sidebar.spaces]
rows = [
  ["state_icon", "workspace"],
  ["branch", "git_status", { token = "$ddev", rules = [
    { starts_with = "●", fg = "#a6e3a1" },
    { starts_with = "◐", fg = "#f9e2af" },
    { starts_with = "◌", fg = "#f9e2af" },
    { starts_with = "○", dim = true },
  ] }],
]
```

## Actions

All actions resolve the project from the focused pane's folder (from
`HERDR_PLUGIN_CONTEXT_JSON`, falling back to `herdr pane current`). Outside a ddev project they
show the notification "Not in a ddev project" and stop.

| Action | Runs (in the project root) |
|--------|----------------------------|
| `toggle` | `ddev stop` when running, else `ddev start` |
| `start` / `stop` / `restart` | `ddev start` / `ddev stop` / `ddev restart` |
| `open` | `ddev describe -j`, then opens `raw.primary_url` (see Open site) |
| `picker` | `herdr plugin pane open --plugin danjuls.ddev --entrypoint picker` |
| `configure` / `unconfigure` | Opens the matching popup (see Configure and unconfigure) |

**Detached worker.** `start`/`stop`/`restart`/`toggle` write a busy marker, re-launch the binary
in the background to run the ddev call, and return at once, so Herdr never waits on a slow
first `ddev start`. The worker removes the marker, triggers an immediate badge refresh and shows
a notification.

**Busy markers** live in `$HERDR_PLUGIN_STATE_DIR/busy/<project>.json` (verb, pid, start time).
A second action on a busy project is refused with "already starting". A marker whose process
is gone, or that is older than 15 minutes, is ignored and removed.

**Notifications** use `herdr notification show "ddev" --body "<project> started"
--sound done`. Failures use `--sound request` and the first line of ddev's stderr; the full
output goes to `worker.log` in the plugin state dir, because the worker runs detached from Herdr.

## Open site

`open_mode = "auto"` opens a browser when there is a local desktop session and no
`SSH_CONNECTION` (macOS `open`, Linux `xdg-open` with `DISPLAY`/`WAYLAND_DISPLAY`), otherwise
uses clipboard mode.

Herdr 0.9.1 has no clipboard API. Clipboard mode opens the small `url` popup, which emits the URL
as an OSC 52 clipboard escape and shows it in large text with "Copied - or drag to select". If
Herdr forwards OSC 52 from pane output to the outer terminal, the copy is automatic; if not,
drag-select in the popup copies through Herdr's own clipboard support. Any key closes it.

## Picker popup

A full-screen TUI in the `picker` popup:

- Rows: `name · status · type · URL`, running first, then paused, then stopped, then by name.
  Built from the Docker query plus `ddev list -j` (once, when the popup opens, for type and URL).
- Typing filters by substring on name and folder. `↑`/`↓` (and `ctrl+n`/`ctrl+p`) move.
  `ctrl+j`/`ctrl+k` are not used: navigation plugins commonly bind them globally in Herdr.
- `enter` focuses the workspace whose project matches (`herdr workspace focus`), or creates one
  (`herdr workspace create --cwd <root> --label <name> --focus`), then closes.
- `ctrl+s` start/stop, `ctrl+r` restart, `ctrl+o` open: same code paths as the actions, status
  updates in place. Control chords because plain letters type into the filter; they match the
  `ctrl-s`/`ctrl-o` of Daniel's tmux session switcher.
- In clipboard mode, `ctrl+o` shows the URL in the picker's status line and emits OSC 52 from the
  picker itself, because Herdr allows only one popup at a time.
- `esc` clears the filter, then closes.

## Configure and unconfigure

Both are popups, so they work without adding the binary to `PATH`.

**Configure** reads `~/.config/herdr/config.toml`, plans its changes (badge token, three
keybindings), shows the plan and asks `y/N`. Any key already bound, in `[keys]` defaults or
`[[keys.command]]`, is skipped and listed. On yes it:

1. Backs up the file to `config.toml.bak-<timestamp>`.
2. Edits with `toml_edit`, so comments and formatting survive.
3. Records exactly what it added in `$HERDR_PLUGIN_STATE_DIR/owned.json`.
4. Runs `herdr config check`, restoring the backup if it fails.
5. Runs `herdr server reload-config`.

**Unconfigure** removes only what `owned.json` lists, with the same backup, check and reload.

## Plugin config

Optional `$HERDR_PLUGIN_CONFIG_DIR/config.toml`; every key has a default and no file is needed:

```toml
ddev_command = ["ddev"]      # e.g. ["distrobox", "enter", "-n", "dev", "--", "ddev"]
docker_command = ["docker"]  # same wrapper idea
poll_interval_secs = 5
open_mode = "auto"           # "auto" | "browser" | "clipboard"
```

An invalid file is reported in a notification once, and defaults are used.

## Portability

- **Finding binaries.** Unless configured, `ddev` and `docker` are resolved from PATH first, then
  `/opt/homebrew/bin`, `/usr/local/bin`, `/usr/bin`, `/home/linuxbrew/.linuxbrew/bin`, and
  `~/.orbstack/bin` for docker (ported from norns-companion `util.rs`). Spawned commands get a
  PATH widened with the binary's own folder, because Herdr's server can start with a bare PATH
  and ddev needs `mkcert` and `docker` next to it.
- **Docker CLI rather than its socket**, so Docker Desktop, OrbStack and Colima all work through
  the CLI's own context resolution.
- **Distrobox.** With Herdr inside the same distrobox as ddev, no config is needed. With Herdr on
  the host, set both commands to the `distrobox enter` wrapper; the README suggests
  `poll_interval_secs = 15` because each wrapped call pays distrobox startup. Folders match either
  way because distrobox shares `$HOME`.

## Error handling

| Situation | Behaviour |
|-----------|-----------|
| ddev or docker not found | No badges; actions notify with the config key to set |
| Docker not running | Badges expire via TTL; ticker backs off 5s to 30s, resets on success |
| ddev command fails | Notification with the first stderr line; full output in `worker.log` |
| Action on a busy project | Refused with "already starting" (or stopping, restarting) |
| Badge during an action | Busy marker shows `◌ ddev…`; the ticker never overwrites it |
| Second ticker | Fails to take the lock and exits quietly |
| Herdr server gone | Ticker exits after 3 consecutive failed `herdr` calls |
| Invalid plugin config | One notification, then defaults |
| Configure breaks config.toml | `herdr config check` fails, backup is restored |

## Testing

- **Unit tests** for pure logic: Docker output parsing, folder matching (nested projects,
  trailing slashes, canonicalized paths), config name resolution, workspace majority vote, badge
  text, TTL/seq values, backoff, busy-marker expiry, `open_mode` decision, config parsing,
  configure planning (conflict skipping, `owned.json` round trip).
- **Scripted fakes.** Scenario tests drive the ticker and actions through a fake `Runner` that
  returns canned `docker`/`herdr`/`ddev` output and records every call, then assert the exact
  `herdr` calls (for example `report-metadata w1 --token ddev=● ddev`).
- **Manual checklist** (`docs/manual-test.md`) for badge colours, popups, notifications, OSC 52
  copy over SSH, and distrobox, run against a throwaway session with `herdr plugin link`, never
  the user's default session or real `config.toml`.
- **CI**: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` on macOS and Linux.

## Release and install

- A `vX.Y.Z` tag runs `release.yml`: builds `aarch64-apple-darwin`, `x86_64-apple-darwin`,
  `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, uploads `.tar.gz` archives with
  SHA-256 files.
- CI fails when the manifest `version`, `Cargo.toml` version and tag disagree.
- `scripts/install.sh` (the manifest build step, run in the plugin folder) detects the platform,
  downloads the archive for the manifest's version, verifies the checksum and installs
  `bin/herdr-ddev`. With no matching asset or no network, it runs `cargo build --release` and
  copies the binary, or fails with a message naming both options.

## Publishing

- README: what it does, screenshots (Daniel provides), install, configure popup, keys, config
  (including distrobox), troubleshooting from the error table, uninstall.
- MIT license. `min_herdr_version = "0.9.1"`, the version tested; lower it only after testing.
- Going live: repo public, topic `herdr-plugin`, first tagged release. The marketplace indexes
  within 30 minutes.

## Dependencies (proposed)

| Crate | Why | Alternative considered |
|-------|-----|------------------------|
| `serde`, `serde_json` | Parse herdr/ddev JSON | Hand parsing: fragile |
| `toml` | Read the plugin config | Same crate family as below |
| `toml_edit` | Edit Herdr's `config.toml` keeping comments | Rewrite the file: loses comments |
| `ratatui`, `crossterm` | Picker and configure popups | Raw ANSI by hand: much more code |
| `anyhow` | Error context in the binary | `thiserror`: not needed for a binary |
| `tempfile` (dev only) | Test fixtures | Manual temp dirs |

Minimum Rust 1.89 (for `File::try_lock`). No async runtime.

## To verify during implementation

1. Whether Herdr forwards OSC 52 from pane or popup output to the outer terminal (decides whether
   clipboard mode copies automatically).
2. With `herdr --remote`, sidebar layout comes from the client's config and custom commands from
   the server's. Confirm configure's split: the badge row on the machine you view from, the
   plugin, keys and ticker on the machine where panes run. Document the result.
3. That `starts_with` rules style the `$ddev` token as shown in the Badge section.
4. That action commands run without blocking the Herdr UI. The detached worker makes this safe
   either way.
5. `herdr pane list` JSON field names (`workspace_id`, `cwd`, `foreground_cwd`, `focused`), seen
   on 0.9.1; pin them in a parsing test from real output.
6. Whether global Herdr keybindings (such as a user's `ctrl+h/j/k/l`) still fire while a popup
   is open, which decides which chords the picker can safely use.
