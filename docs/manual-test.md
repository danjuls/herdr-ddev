# Manual test checklist

Things only a person can confirm. Run against a linked checkout (`herdr plugin link "$PWD"`)
after `sh scripts/install.sh`. Note the Herdr version and machine for each run.

## Setup

- [ ] `herdr plugin action invoke danjuls.ddev.configure` shows the plan, asks y/N, writes a
      backup and reloads Herdr; the sidebar shows `$ddev` on the second Space row
- [ ] Running configure again says "Nothing to change"

## Badges

- [ ] A workspace in a running ddev project shows a green `● ddev`
- [ ] `ddev stop` typed in a shell turns it into a dim `○ ddev` within ~5 seconds
- [ ] Stopping only the web container (`docker stop ddev-<project>-web`) shows a yellow `◐ ddev`
      (ddev 1.25 has no `ddev pause`)
- [ ] A workspace outside ddev projects shows no badge
- [ ] Quitting Docker makes badges disappear within ~20 seconds; starting it brings them back

## Actions

- [ ] `prefix+shift+s` on a stopped project shows `◌ ddev…`, then `● ddev` and a "started"
      notification with the done sound
- [ ] Pressing it twice quickly shows "already starting"
- [ ] `prefix+shift+s` outside a project shows "Not in a ddev project"
- [ ] `prefix+shift+o` opens the site in the browser (local desktop)

## Picker

- [ ] `prefix+shift+e` lists all projects, running first, with type and URL
- [ ] Typing filters; `esc` clears, then closes
- [ ] `enter` jumps to the project's workspace; on a project without one it creates one
- [ ] `ctrl+s`, `ctrl+r`, `ctrl+o` act on the selected project and the status updates in place
- [ ] With vim-herdr-navigation installed, `ctrl+j`/`ctrl+k` inside the picker do not move the
      selection (record whether they move Herdr focus instead: spec verify item 6)

## Over SSH and remote

- [ ] Over SSH with `open_mode = "auto"`, open shows the URL popup; record whether the URL
      reached the local clipboard automatically (spec verify item 1)
- [ ] With `herdr --remote`, record which machine needs the sidebar row and which the keys
      (spec verify item 2), and add the result to the README

## Distrobox (Bazzite)

- [ ] With the distrobox wrappers configured, badges and actions work at
      `poll_interval_secs = 15`

## Remove

- [ ] `unconfigure` removes only herdr-ddev's keys and badge; the backup and the rest of the
      config are untouched
