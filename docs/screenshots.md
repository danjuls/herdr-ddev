# Screenshots for the README

Shot list for v0.1. Save PNGs in `docs/images/` with the names below; the README links them
once they exist.

## Setup (keeps client names out)

1. `sh scripts/demo.sh up` - creates `acme-shop` (running), `northwind` (paused) and
   `blue-harbor` (stopped) in `~/herdr-ddev-demo`.
2. `herdr --session demo` in a fresh Ghostty window. Run the next commands in a pane **inside**
   that session, so they reach the demo session and not your normal one:

   ```sh
   herdr workspace create --cwd ~/herdr-ddev-demo/acme-shop --label "Acme Shop"
   herdr workspace create --cwd ~/herdr-ddev-demo/northwind --label "Northwind"
   herdr workspace create --cwd ~/herdr-ddev-demo/blue-harbor --label "Blue Harbor"
   ```

3. Press any ddev key once (for example `prefix+shift+e`, then `esc`) so the badge poller
   starts; badges show within ~5 seconds.

Before each shot, check nothing else is visible: other Herdr sessions, client Claude panes,
macOS notification banners from other apps, and your username in paths (crop if needed).
The demo folders are not git repos, so no branch names show in the sidebar.

## Shots

| # | File | What to capture | How |
|---|------|-----------------|-----|
| 1 | `badges.png` | The sidebar with all three states: green `●`, yellow `◐`, dim `○` | The hero image. Sidebar plus a slice of the pane, so it reads as Herdr |
| 2 | `picker.png` | The picker popup with the three demo rows, type and URL, footer help visible | `prefix+shift+e`, type `demo` so only demo projects show, arrow to `acme-shop` |
| 3 | `starting.png` | A workspace mid-start showing `◌ ddev…` | Focus Blue Harbor, `prefix+shift+s`, capture within the first seconds |
| 4 | `notification.png` | The "blue-harbor started" notification | Right after shot 3 finishes |
| 5 | `configure.png` | Configure listing its planned changes, before answering | Your real config already has the changes, so use a scratch copy in a demo pane (see below); press `n` |

For shot 5, in a pane of the demo session (nothing is written when you answer `n`):

```sh
herdr --default-config > /tmp/herdr-demo-config.toml
HERDR_CONFIG_PATH=/tmp/herdr-demo-config.toml ~/Work/herdr-ddev/bin/herdr-ddev configure
```

Optional: a short GIF of picker -> `enter` jumps to the workspace -> `prefix+shift+s` -> badge
flips from `○` to `◌` to `●`. Kap or CleanShot can record it; keep it under ~5 MB.

## Afterwards

1. Detach with `prefix+q`, then `herdr session stop demo` and `herdr session delete demo`.
2. `sh scripts/demo.sh down`
3. Put the PNGs in `docs/images/` and ask Claude to add them to the README.
