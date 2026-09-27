//! The project picker popup.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use crate::actions::{toggle_verb, worker_args};
use crate::app::{Ctx, unix_ms};
use crate::busy::{self, Verb};
use crate::ddev::{self, Listed};
use crate::docker::{self, Project, State};
use crate::herdr::Pane;
use crate::open::{self, Opener};
use crate::{badge, project, ticker};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub root: PathBuf,
    pub state: State,
    pub kind: String,
    pub url: String,
    pub workspace: Option<String>,
    pub busy: Option<Verb>,
}

fn state_from_status(status: &str) -> State {
    match status {
        "running" => State::Running,
        "paused" => State::Paused,
        _ => State::Stopped,
    }
}

fn rank(state: State) -> u8 {
    match state {
        State::Running => 0,
        State::Paused => 1,
        State::Stopped => 2,
    }
}

/// Rows from `ddev list` (every known project, with type and URL) updated with Docker's
/// state (`running` must be canonical), each linked to the first workspace with a pane inside
/// it. Running first, then paused, then stopped, then by name.
pub fn build_rows(listed: &[Listed], running: &[Project], panes: &[Pane]) -> Vec<Row> {
    let mut rows: Vec<Row> = listed
        .iter()
        .filter(|l| !l.approot.is_empty())
        .map(|l| Row {
            name: l.name.clone(),
            root: project::canonical(Path::new(&l.approot)),
            state: state_from_status(&l.status),
            kind: l.kind.clone(),
            url: l.primary_url.clone(),
            workspace: None,
            busy: None,
        })
        .collect();
    for found in running {
        match rows.iter_mut().find(|row| row.root == found.root) {
            Some(row) => row.state = found.state,
            None => rows.push(Row {
                name: found.name.clone(),
                root: found.root.clone(),
                state: found.state,
                kind: String::new(),
                url: String::new(),
                workspace: None,
                busy: None,
            }),
        }
    }
    for pane in panes {
        let Some(dir) = pane.dir() else { continue };
        let dir = project::canonical(Path::new(dir));
        let owner = rows
            .iter_mut()
            .filter(|row| dir.starts_with(&row.root))
            .max_by_key(|row| row.root.components().count());
        if let Some(row) = owner.filter(|row| row.workspace.is_none()) {
            row.workspace = Some(pane.workspace_id.clone());
        }
    }
    rows.sort_by(|a, b| {
        rank(a.state)
            .cmp(&rank(b.state))
            .then_with(|| a.name.cmp(&b.name))
    });
    rows
}

pub fn row_line(row: &Row) -> String {
    let status = match (row.busy, row.state) {
        (Some(verb), _) => verb.progressive(),
        (None, State::Running) => "running",
        (None, State::Paused) => "paused",
        (None, State::Stopped) => "stopped",
    };
    let mut parts = vec![format!(
        "{} {}",
        badge::symbol(row.state, row.busy),
        row.name
    )];
    parts.push(status.to_string());
    parts.extend(
        [&row.kind, &row.url]
            .into_iter()
            .filter(|s| !s.is_empty())
            .cloned(),
    );
    parts.join("  ·  ")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    None,
    Quit,
    Jump(usize),
    Change(usize, Verb),
    Open(usize),
}

pub struct Picker {
    pub rows: Vec<Row>,
    pub filter: String,
    pub selected: usize,
    pub status: String,
}

impl Picker {
    pub fn new(rows: Vec<Row>) -> Picker {
        Picker {
            rows,
            filter: String::new(),
            selected: 0,
            status: String::new(),
        }
    }

    /// Indices of rows whose name or folder contains the filter, ignoring case.
    pub fn visible(&self) -> Vec<usize> {
        let needle = self.filter.to_lowercase();
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                needle.is_empty()
                    || row.name.to_lowercase().contains(&needle)
                    || row.root.to_string_lossy().to_lowercase().contains(&needle)
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn selected_row(&self) -> Option<usize> {
        self.visible().get(self.selected).copied()
    }

    fn move_by(&mut self, delta: isize) {
        let count = self.visible().len();
        self.selected = if count == 0 {
            0
        } else {
            self.selected.saturating_add_signed(delta).min(count - 1)
        };
    }

    fn type_char(&mut self, c: Option<char>) -> Command {
        match c {
            Some(c) => self.filter.push(c),
            None => {
                self.filter.pop();
            }
        }
        self.selected = 0;
        Command::None
    }

    pub fn handle(&mut self, key: KeyEvent) -> Command {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc if !self.filter.is_empty() => self.clear_filter(),
            KeyCode::Esc => Command::Quit,
            KeyCode::Char('c') if ctrl => Command::Quit,
            KeyCode::Enter => self.selected_row().map_or(Command::None, Command::Jump),
            KeyCode::Up => self.moved(-1),
            KeyCode::Char('p') if ctrl => self.moved(-1),
            KeyCode::Down => self.moved(1),
            KeyCode::Char('n') if ctrl => self.moved(1),
            KeyCode::Char('s') if ctrl => self.selected_row().map_or(Command::None, |index| {
                Command::Change(index, toggle_verb(self.rows[index].state))
            }),
            KeyCode::Char('r') if ctrl => self
                .selected_row()
                .map_or(Command::None, |index| Command::Change(index, Verb::Restart)),
            KeyCode::Char('o') if ctrl => self.selected_row().map_or(Command::None, Command::Open),
            KeyCode::Backspace => self.type_char(None),
            KeyCode::Char(c) if !ctrl && !alt => self.type_char(Some(c)),
            _ => Command::None,
        }
    }

    fn moved(&mut self, delta: isize) -> Command {
        self.move_by(delta);
        Command::None
    }

    fn clear_filter(&mut self) -> Command {
        self.filter.clear();
        self.selected = 0;
        Command::None
    }
}

/// The popup: list projects, filter, act, jump.
pub fn run(ctx: &Ctx) -> Result<()> {
    let _ = ticker::ensure_running(ctx);
    let mut picker = Picker::new(load_rows(ctx));
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut picker, ctx);
    ratatui::restore();
    result
}

fn current_running(ctx: &Ctx) -> Vec<Project> {
    ctx.docker
        .as_ref()
        .and_then(|docker| docker::query(ctx.runner, docker).ok())
        .map(project::canonical_projects)
        .unwrap_or_default()
}

fn mark_busy(ctx: &Ctx, rows: &mut [Row]) {
    let busy = busy::live_all(&ctx.state_dir, unix_ms() / 1000, ctx.pid_alive);
    for row in rows {
        row.busy = busy.get(&row.name).copied();
    }
}

fn load_rows(ctx: &Ctx) -> Vec<Row> {
    let listed = ctx
        .ddev
        .as_ref()
        .and_then(|d| ddev::list(ctx.runner, d).ok())
        .unwrap_or_default();
    let panes = ctx.herdr.panes().unwrap_or_default();
    let mut rows = build_rows(&listed, &current_running(ctx), &panes);
    mark_busy(ctx, &mut rows);
    rows
}

/// Re-read Docker and busy markers so changes show without reopening. Rows keep their order.
fn refresh(ctx: &Ctx, picker: &mut Picker) {
    let running = current_running(ctx);
    for row in &mut picker.rows {
        row.state = running
            .iter()
            .find(|p| p.root == row.root)
            .map_or(State::Stopped, |p| p.state);
    }
    mark_busy(ctx, &mut picker.rows);
}

fn event_loop(terminal: &mut DefaultTerminal, picker: &mut Picker, ctx: &Ctx) -> Result<()> {
    let mut last_refresh = Instant::now();
    loop {
        terminal.draw(|frame| draw(frame, picker))?;
        if event::poll(Duration::from_millis(500))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match picker.handle(key) {
                Command::None => {}
                Command::Quit => return Ok(()),
                Command::Jump(index) => return jump(ctx, &picker.rows[index]),
                Command::Change(index, verb) => change(ctx, picker, index, verb),
                Command::Open(index) => picker.status = open_row(ctx, &picker.rows[index]),
            }
        }
        if last_refresh.elapsed() >= Duration::from_secs(2) {
            refresh(ctx, picker);
            last_refresh = Instant::now();
        }
    }
}

fn change(ctx: &Ctx, picker: &mut Picker, index: usize, verb: Verb) {
    let row = &mut picker.rows[index];
    match (ctx.spawn)(&worker_args(verb, &row.root, &row.name)) {
        Ok(()) => {
            row.busy = Some(verb);
            picker.status = format!("{} {}…", row.name, verb.progressive());
        }
        Err(err) => picker.status = format!("could not start the worker: {err:#}"),
    }
}

fn jump(ctx: &Ctx, row: &Row) -> Result<()> {
    match &row.workspace {
        Some(workspace) => ctx.herdr.focus_workspace(workspace),
        None => ctx.herdr.create_workspace(&row.root, &row.name),
    }
}

/// Open the row's site. Only one Herdr popup can be open, so clipboard mode copies from the
/// picker itself and shows the URL in the status line.
fn open_row(ctx: &Ctx, row: &Row) -> String {
    let url = if row.url.is_empty() {
        match ctx
            .ddev
            .as_ref()
            .map(|d| ddev::site_url(ctx.runner, d, &row.root))
        {
            Some(Ok(url)) => url,
            Some(Err(err)) => return format!("{}: {err:#}", row.name),
            None => return "ddev not found".to_string(),
        }
    } else {
        row.url.clone()
    };
    match open::decide(ctx.open_mode, std::env::consts::OS, &|key| {
        std::env::var(key).ok()
    }) {
        Opener::Browser(program) => match ctx.runner.run(&[program.to_string(), url.clone()], None)
        {
            Ok(out) if out.success => format!("opened {url}"),
            _ => format!("could not open {url}"),
        },
        Opener::Clipboard => {
            print!("{}", open::osc52(&url));
            let _ = std::io::Write::flush(&mut std::io::stdout());
            format!("Copied {url} - or select it here")
        }
    }
}

fn draw(frame: &mut Frame, picker: &Picker) {
    let [header, list_area, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    frame.render_widget(Paragraph::new(format!(" ddev > {}", picker.filter)), header);
    let visible = picker.visible();
    let items: Vec<ListItem> = visible
        .iter()
        .map(|&index| ListItem::new(row_line(&picker.rows[index])))
        .collect();
    let mut state =
        ListState::default().with_selected((!visible.is_empty()).then_some(picker.selected));
    let list = List::new(items).highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    frame.render_stateful_widget(list, list_area, &mut state);
    let help = if picker.status.is_empty() {
        " enter jump · ctrl+s start/stop · ctrl+r restart · ctrl+o open · esc close".to_string()
    } else {
        format!(" {}", picker.status)
    };
    frame.render_widget(
        Paragraph::new(help).style(Style::new().add_modifier(Modifier::DIM)),
        footer,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(name: &str, root: &str, status: &str) -> Listed {
        Listed {
            name: name.into(),
            status: status.into(),
            approot: root.into(),
            kind: "drupal11".into(),
            primary_url: format!("https://{name}.ddev.site"),
        }
    }

    fn pane(workspace: &str, dir: &str) -> Pane {
        Pane {
            pane_id: format!("{workspace}:p1"),
            workspace_id: workspace.into(),
            focused: false,
            cwd: Some(dir.into()),
            foreground_cwd: None,
        }
    }

    fn rows() -> Vec<Row> {
        let listed = [
            listed("zeta", "/w/zeta", "stopped"),
            listed("blog", "/w/blog", "stopped"),
            listed("shop", "/w/shop", "stopped"),
            listed("orphan", "", "stopped"),
        ];
        let running = [
            Project {
                name: "shop".into(),
                root: "/w/shop".into(),
                state: State::Running,
            },
            Project {
                name: "extra".into(),
                root: "/w/extra".into(),
                state: State::Paused,
            },
        ];
        let panes = [
            pane("w1", "/w/shop/web"),
            pane("w2", "/w/blog"),
            pane("w3", "/w/shop"),
        ];
        build_rows(&listed, &running, &panes)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn rows_merge_ddev_list_and_docker_and_sort_by_state_then_name() {
        let rows = rows();
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["shop", "extra", "blog", "zeta"]);
        assert_eq!(rows[1].state, State::Paused);
        assert_eq!(rows[1].kind, "");
    }

    #[test]
    fn rows_link_the_first_workspace_inside_the_project() {
        let rows = rows();
        assert_eq!(rows[0].workspace.as_deref(), Some("w1"));
        assert_eq!(rows[2].workspace.as_deref(), Some("w2"));
        assert_eq!(rows[3].workspace, None);
    }

    #[test]
    fn typing_filters_by_name_or_folder_case_insensitively() {
        let mut picker = Picker::new(rows());
        for c in "BLO".chars() {
            picker.handle(key(KeyCode::Char(c)));
        }
        assert_eq!(picker.visible(), [2]);
        picker.handle(key(KeyCode::Backspace));
        picker.handle(key(KeyCode::Backspace));
        picker.handle(key(KeyCode::Backspace));
        for c in "/w/z".chars() {
            picker.handle(key(KeyCode::Char(c)));
        }
        assert_eq!(picker.visible(), [3]);
    }

    #[test]
    fn plain_letters_type_instead_of_acting() {
        let mut picker = Picker::new(rows());
        assert_eq!(picker.handle(key(KeyCode::Char('s'))), Command::None);
        assert_eq!(picker.filter, "s");
    }

    #[test]
    fn escape_clears_the_filter_then_quits() {
        let mut picker = Picker::new(rows());
        picker.handle(key(KeyCode::Char('x')));
        assert_eq!(picker.handle(key(KeyCode::Esc)), Command::None);
        assert_eq!(picker.filter, "");
        assert_eq!(picker.handle(key(KeyCode::Esc)), Command::Quit);
    }

    #[test]
    fn movement_stays_in_bounds() {
        let mut picker = Picker::new(rows());
        picker.handle(key(KeyCode::Up));
        assert_eq!(picker.selected, 0);
        for _ in 0..10 {
            picker.handle(key(KeyCode::Down));
        }
        assert_eq!(picker.selected, 3);
        picker.handle(ctrl('p'));
        assert_eq!(picker.selected, 2);
        picker.handle(ctrl('n'));
        assert_eq!(picker.selected, 3);
    }

    #[test]
    fn control_keys_act_on_the_selected_row() {
        let mut picker = Picker::new(rows());
        assert_eq!(picker.handle(ctrl('s')), Command::Change(0, Verb::Stop));
        assert_eq!(picker.handle(ctrl('r')), Command::Change(0, Verb::Restart));
        assert_eq!(picker.handle(ctrl('o')), Command::Open(0));
        picker.handle(key(KeyCode::Down));
        picker.handle(key(KeyCode::Down));
        assert_eq!(picker.handle(ctrl('s')), Command::Change(2, Verb::Start));
        assert_eq!(picker.handle(key(KeyCode::Enter)), Command::Jump(2));
    }

    #[test]
    fn no_match_means_no_action() {
        let mut picker = Picker::new(rows());
        for c in "nothing".chars() {
            picker.handle(key(KeyCode::Char(c)));
        }
        assert_eq!(picker.handle(key(KeyCode::Enter)), Command::None);
        assert_eq!(picker.handle(ctrl('s')), Command::None);
    }

    #[test]
    fn row_line_shows_symbol_status_type_and_url() {
        let mut row = rows().remove(0);
        assert_eq!(
            row_line(&row),
            "● shop  ·  running  ·  drupal11  ·  https://shop.ddev.site"
        );
        row.busy = Some(Verb::Stop);
        assert!(row_line(&row).starts_with("◌ shop  ·  stopping"));
    }
}
