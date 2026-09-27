//! Adding and removing the `$ddev` badge and keybindings in Herdr's `config.toml`.
//!
//! Configure appends its tables as one marked block at the end of the file, so every existing
//! comment stays under its own header. Only two edits happen in place, inside tables the user
//! already has: a `$ddev` row appended to custom sidebar rows, or `rows` added to an existing
//! `[ui.sidebar.spaces]`. Unconfigure removes exactly what `owned.json` lists and hands any
//! comments a removed table carried to whatever follows it.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value, value};

use crate::app::{Ctx, unix_ms};
use crate::project;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub key: &'static str,
    pub command: &'static str,
    pub description: &'static str,
}

pub const BINDINGS: [Binding; 3] = [
    Binding {
        key: "prefix+shift+s",
        command: "danjuls.ddev.toggle",
        description: "ddev: start/stop project",
    },
    Binding {
        key: "prefix+shift+o",
        command: "danjuls.ddev.open",
        description: "ddev: open site",
    },
    Binding {
        key: "prefix+shift+e",
        command: "danjuls.ddev.picker",
        description: "ddev: projects",
    },
];

pub const BLOCK_START: &str = "# >>> herdr-ddev: added by `configure`, removed by `unconfigure`";
pub const BLOCK_END: &str = "# <<< herdr-ddev";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SidebarChange {
    #[default]
    None,
    /// No `[ui.sidebar.spaces]` existed: one was appended with the defaults plus `$ddev`.
    CreatedRows,
    /// `[ui.sidebar.spaces]` existed without `rows`: the defaults plus `$ddev` went into it.
    InsertedRows,
    /// Custom rows existed: a row holding only `$ddev` was appended.
    AppendedRow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnedKey {
    pub key: String,
    pub command: String,
}

/// Exactly what configure added, so unconfigure removes nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Owned {
    /// The config file the changes went into. `None` in files from the first version.
    #[serde(default)]
    pub config_path: Option<PathBuf>,
    pub keys: Vec<OwnedKey>,
    pub sidebar: SidebarChange,
}

impl Owned {
    pub fn merge(&mut self, other: Owned) {
        for key in other.keys {
            if !self.keys.contains(&key) {
                self.keys.push(key);
            }
        }
        if self.sidebar == SidebarChange::None {
            self.sidebar = other.sidebar;
        }
        if self.config_path.is_none() {
            self.config_path = other.config_path;
        }
    }

    /// Refuse to change a config other than the one configure changed.
    pub fn check_for(&self, path: &Path) -> Result<(), String> {
        match &self.config_path {
            Some(owned) if project::canonical(owned) != project::canonical(path) => Err(format!(
                "herdr-ddev's changes are in {}, not {}. Run this with that config, or delete \
                 owned.json in the plugin's state folder if that file is gone.",
                owned.display(),
                path.display()
            )),
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub add: Vec<Binding>,
    pub skipped: Vec<String>,
    pub sidebar: SidebarChange,
}

/// The result of configure: the new file text and what it added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub text: String,
    pub owned: Owned,
}

fn norm(key: &str) -> String {
    key.trim().to_lowercase()
}

/// Every bound key and what it does: `[keys]` action values (string or list) and
/// `[[keys.command]]` entries.
pub fn used_keys(doc: &DocumentMut) -> Vec<(String, String)> {
    let mut used = Vec::new();
    let Some(keys) = doc.get("keys").and_then(Item::as_table_like) else {
        return used;
    };
    for (action, item) in keys.iter() {
        match item.as_value() {
            Some(Value::String(s)) => used.push((norm(s.value()), action.to_string())),
            Some(Value::Array(list)) => {
                for entry in list.iter().filter_map(Value::as_str) {
                    used.push((norm(entry), action.to_string()));
                }
            }
            _ => {}
        }
    }
    if let Some(commands) = keys.get("command").and_then(Item::as_array_of_tables) {
        for table in commands.iter() {
            if let Some(key) = table.get("key").and_then(Item::as_str) {
                let command = table
                    .get("command")
                    .and_then(Item::as_str)
                    .unwrap_or("a command");
                used.push((norm(key), command.to_string()));
            }
        }
    }
    used
}

/// Why new `[[keys.command]]` tables cannot be appended, if they cannot.
fn keys_blocked(doc: &DocumentMut) -> Option<String> {
    if let Some(item) = doc.get("keys") {
        if !item.is_table() {
            return Some("`keys` is written inline; add the bindings yourself".to_string());
        }
        if let Some(command) = item.get("command")
            && !command.is_array_of_tables()
        {
            return Some(
                "`keys.command` is not a list of tables; add the bindings yourself".into(),
            );
        }
    }
    None
}

enum Spaces<'a> {
    Missing,
    WithoutRows,
    Rows(&'a Array),
    Blocked(String),
}

fn spaces(doc: &DocumentMut) -> Spaces<'_> {
    let mut table = doc.as_table();
    for (name, path) in [
        ("ui", "ui"),
        ("sidebar", "ui.sidebar"),
        ("spaces", "ui.sidebar.spaces"),
    ] {
        match table.get(name) {
            None => return Spaces::Missing,
            Some(Item::Table(child)) => table = child,
            Some(_) => {
                return Spaces::Blocked(format!(
                    "`{path}` is written inline; add the $ddev badge to its rows yourself"
                ));
            }
        }
    }
    match table.get("rows").map(Item::as_array) {
        None => Spaces::WithoutRows,
        Some(Some(rows)) => Spaces::Rows(rows),
        Some(None) => Spaces::Blocked("`ui.sidebar.spaces.rows` is not a list".to_string()),
    }
}

fn spaces_table_mut(doc: &mut DocumentMut) -> Option<&mut Table> {
    doc.get_mut("ui")?
        .get_mut("sidebar")?
        .get_mut("spaces")?
        .as_table_mut()
}

fn rows_mut(doc: &mut DocumentMut) -> Option<&mut Array> {
    spaces_table_mut(doc)?.get_mut("rows")?.as_array_mut()
}

fn is_ddev_token(entry: &Value) -> bool {
    match entry {
        Value::String(s) => s.value() == "$ddev",
        Value::InlineTable(t) => t.get("token").and_then(Value::as_str) == Some("$ddev"),
        _ => false,
    }
}

fn has_ddev(rows: &Array) -> bool {
    rows.iter()
        .filter_map(Value::as_array)
        .any(|row| row.iter().any(is_ddev_token))
}

/// Whether `rows` is exactly Herdr's default Space layout.
fn is_default_rows(rows: &Array) -> bool {
    let wanted: [&[&str]; 2] = [&["state_icon", "workspace"], &["branch", "git_status"]];
    rows.len() == wanted.len()
        && rows.iter().zip(wanted).all(|(row, want)| {
            row.as_array().is_some_and(|row| {
                row.len() == want.len() && row.iter().zip(want).all(|(v, w)| v.as_str() == Some(*w))
            })
        })
}

pub fn plan(doc: &DocumentMut) -> Plan {
    let used = used_keys(doc);
    let mut add = Vec::new();
    let mut skipped = Vec::new();
    for binding in BINDINGS {
        match used.iter().find(|(key, _)| *key == norm(binding.key)) {
            Some((_, what)) if what == binding.command => {
                skipped.push(format!("{} already runs {}", binding.key, binding.command));
            }
            Some((_, what)) => skipped.push(format!(
                "{} is already bound to {what}; bind {} to another key yourself",
                binding.key, binding.command
            )),
            None => add.push(binding),
        }
    }
    if let Some(reason) = keys_blocked(doc).filter(|_| !add.is_empty()) {
        add.clear();
        skipped.push(reason);
    }
    let sidebar = match spaces(doc) {
        Spaces::Missing => SidebarChange::CreatedRows,
        Spaces::WithoutRows => SidebarChange::InsertedRows,
        Spaces::Rows(rows) if has_ddev(rows) => SidebarChange::None,
        Spaces::Rows(_) => SidebarChange::AppendedRow,
        Spaces::Blocked(reason) => {
            skipped.push(reason);
            SidebarChange::None
        }
    };
    Plan {
        add,
        skipped,
        sidebar,
    }
}

pub fn describe(plan: &Plan) -> Vec<String> {
    let mut lines: Vec<String> = plan
        .add
        .iter()
        .map(|b| format!("add key {} -> {}", b.key, b.command))
        .collect();
    lines.extend(plan.skipped.iter().map(|reason| format!("skip: {reason}")));
    lines.push(
        match plan.sidebar {
            SidebarChange::CreatedRows => "add the $ddev badge to the sidebar's second row",
            SidebarChange::InsertedRows => "add rows with the $ddev badge to [ui.sidebar.spaces]",
            SidebarChange::AppendedRow => "add the $ddev badge as a new sidebar row",
            SidebarChange::None => "leave the sidebar as it is",
        }
        .to_string(),
    );
    lines
}

/// The `$ddev` token with colour rules (Catppuccin green and yellow, dim when stopped).
fn ddev_token() -> Value {
    let mut rules = Array::new();
    for (prefix, colour) in [
        ("●", Some("#a6e3a1")),
        ("◐", Some("#f9e2af")),
        ("◌", Some("#f9e2af")),
        ("○", None),
    ] {
        let mut rule = InlineTable::new();
        rule.insert("starts_with", prefix.into());
        match colour {
            Some(colour) => rule.insert("fg", colour.into()),
            None => rule.insert("dim", true.into()),
        };
        rules.push(rule);
    }
    let mut token = InlineTable::new();
    token.insert("token", "$ddev".into());
    token.insert("rules", Value::Array(rules));
    Value::InlineTable(token)
}

/// Herdr's default Space rows with `$ddev` added to the second one, one row per line.
fn default_rows_with_ddev() -> Array {
    let mut first = Array::new();
    first.push("state_icon");
    first.push("workspace");
    let mut second = Array::new();
    second.push("branch");
    second.push("git_status");
    second.push(ddev_token());
    let mut rows = Array::new();
    rows.push(first);
    rows.push(second);
    for row in rows.iter_mut() {
        row.decor_mut().set_prefix("\n  ");
    }
    rows.set_trailing("\n");
    rows.set_trailing_comma(true);
    rows
}

fn implicit(table: Table) -> Item {
    let mut table = table;
    table.set_implicit(true);
    Item::Table(table)
}

/// The marked block of new tables to append, or an empty string when there are none.
fn block(plan: &Plan) -> String {
    let mut doc = DocumentMut::new();
    if !plan.add.is_empty() {
        let mut commands = ArrayOfTables::new();
        for binding in &plan.add {
            let mut table = Table::new();
            table["key"] = value(binding.key);
            table["type"] = value("plugin_action");
            table["command"] = value(binding.command);
            table["description"] = value(binding.description);
            commands.push(table);
        }
        let mut keys = Table::new();
        keys.insert("command", Item::ArrayOfTables(commands));
        doc.insert("keys", implicit(keys));
    }
    if plan.sidebar == SidebarChange::CreatedRows {
        let mut spaces = Table::new();
        spaces.insert("rows", value(default_rows_with_ddev()));
        let mut sidebar = Table::new();
        sidebar.insert("spaces", Item::Table(spaces));
        let mut ui = Table::new();
        ui.insert("sidebar", implicit(sidebar));
        doc.insert("ui", implicit(ui));
    }
    let body = doc.to_string();
    let body = body.trim();
    if body.is_empty() {
        String::new()
    } else {
        format!("{BLOCK_START}\n{body}\n{BLOCK_END}\n")
    }
}

/// Configure: the in-place edits, then the marked block appended after the original text.
pub fn apply_text(original: &str, plan: &Plan, config_path: &Path) -> Result<Applied> {
    let mut text = original.to_string();
    if matches!(
        plan.sidebar,
        SidebarChange::InsertedRows | SidebarChange::AppendedRow
    ) {
        let mut doc: DocumentMut = original.parse().context("config.toml is not valid TOML")?;
        if plan.sidebar == SidebarChange::InsertedRows {
            let spaces = spaces_table_mut(&mut doc).context("`ui.sidebar.spaces` disappeared")?;
            spaces.insert("rows", value(default_rows_with_ddev()));
        } else {
            let rows = rows_mut(&mut doc).context("sidebar rows disappeared")?;
            let mut row = Array::new();
            row.push(ddev_token());
            rows.push(row);
        }
        text = doc.to_string();
    }
    let block = block(plan);
    if !block.is_empty() {
        if !text.is_empty() {
            if !text.ends_with('\n') {
                text.push('\n');
            }
            text.push('\n');
        }
        text.push_str(&block);
    }
    let keys = plan
        .add
        .iter()
        .map(|b| OwnedKey {
            key: b.key.to_string(),
            command: b.command.to_string(),
        })
        .collect();
    let owned = Owned {
        config_path: Some(config_path.to_path_buf()),
        keys,
        sidebar: plan.sidebar,
    };
    Ok(Applied { text, owned })
}

/// Unconfigure: remove what `owned` lists, keep every other line and comment.
pub fn unapply_text(text: &str, owned: &Owned) -> Result<String> {
    let mut doc: DocumentMut = text.parse().context("config.toml is not valid TOML")?;
    remove_owned_keys(&mut doc, &owned.keys);
    remove_ddev_badge(&mut doc, owned.sidebar);
    let rendered = doc.to_string();
    let ending = if rendered.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let kept: String = rendered
        .split_inclusive('\n')
        .filter(|line| !matches!(line.trim(), l if l == BLOCK_START || l == BLOCK_END))
        .collect();
    let kept = kept.trim_end();
    Ok(if kept.is_empty() {
        String::new()
    } else {
        format!("{kept}{ending}")
    })
}

fn is_owned(table: &Table, owned: &[OwnedKey]) -> bool {
    owned.iter().any(|o| {
        table.get("key").and_then(Item::as_str) == Some(o.key.as_str())
            && table.get("command").and_then(Item::as_str) == Some(o.command.as_str())
    })
}

fn prefix_of(table: &Table) -> String {
    table
        .decor()
        .prefix()
        .and_then(|raw| raw.as_str())
        .unwrap_or("")
        .to_string()
}

fn remove_owned_keys(doc: &mut DocumentMut, owned: &[OwnedKey]) {
    while let Some(commands) = doc
        .get_mut("keys")
        .and_then(|keys| keys.get_mut("command"))
        .and_then(Item::as_array_of_tables_mut)
    {
        let Some(index) = commands.iter().position(|table| is_owned(table, owned)) else {
            break;
        };
        let (at, prefix) = commands
            .get(index)
            .map(|table| (table.position().map(|p| p as i64), prefix_of(table)))
            .unwrap_or((None, String::new()));
        commands.remove(index);
        let now_empty = commands.is_empty();
        if now_empty && let Some(keys) = doc.get_mut("keys").and_then(Item::as_table_mut) {
            keys.remove("command");
        }
        carry_forward(doc, at, prefix);
    }
    let empty_implicit = doc
        .get("keys")
        .and_then(Item::as_table)
        .is_some_and(|keys| keys.is_empty() && keys.is_implicit());
    if empty_implicit {
        doc.remove("keys");
    }
}

/// Take `$ddev` out of the sidebar rows. Rows configure created are removed only when what
/// is left is exactly Herdr's default, so rows the user edited since stay.
fn remove_ddev_badge(doc: &mut DocumentMut, change: SidebarChange) {
    let Some(rows) = rows_mut(doc) else {
        return;
    };
    for row in rows.iter_mut() {
        if let Some(row) = row.as_array_mut() {
            row.retain(|entry| !is_ddev_token(entry));
        }
    }
    rows.retain(|row| !row.as_array().is_some_and(Array::is_empty));
    let created = matches!(
        change,
        SidebarChange::CreatedRows | SidebarChange::InsertedRows
    );
    if !created || !is_default_rows(rows) {
        return;
    }
    let Some(spaces) = spaces_table_mut(doc) else {
        return;
    };
    spaces.remove("rows");
    if change == SidebarChange::CreatedRows && spaces.is_empty() && !spaces.is_implicit() {
        let at = spaces.position().map(|p| p as i64);
        let prefix = prefix_of(spaces);
        if let Some(sidebar) = doc
            .get_mut("ui")
            .and_then(|ui| ui.get_mut("sidebar"))
            .and_then(Item::as_table_mut)
        {
            sidebar.remove("spaces");
        }
        carry_forward(doc, at, prefix);
    }
    for path in [["ui", "sidebar"].as_slice(), ["ui"].as_slice()] {
        remove_if_empty_implicit(doc, path);
    }
}

fn remove_if_empty_implicit(doc: &mut DocumentMut, path: &[&str]) {
    let (last, parents) = path.split_last().expect("non-empty path");
    let mut table = doc.as_table_mut();
    for name in parents {
        let Some(child) = table.get_mut(name).and_then(Item::as_table_mut) else {
            return;
        };
        table = child;
    }
    let empty_implicit = table
        .get(last)
        .and_then(Item::as_table)
        .is_some_and(|t| t.is_empty() && t.is_implicit());
    if empty_implicit {
        table.remove(last);
    }
}

#[derive(Clone)]
enum Seg {
    Key(String),
    Index(usize),
}

/// The header table that comes first after document position `after`, as a path.
fn find_next(table: &Table, after: i64, path: &mut Vec<Seg>, best: &mut Option<(i64, Vec<Seg>)>) {
    let consider = |t: &Table, path: &Vec<Seg>, best: &mut Option<(i64, Vec<Seg>)>| {
        if t.is_implicit() || t.is_dotted() {
            return;
        }
        if let Some(pos) = t.position().map(|p| p as i64)
            && pos > after
            && best.as_ref().is_none_or(|(b, _)| pos < *b)
        {
            *best = Some((pos, path.clone()));
        }
    };
    for (key, item) in table.iter() {
        match item {
            Item::Table(child) => {
                path.push(Seg::Key(key.to_string()));
                consider(child, path, best);
                find_next(child, after, path, best);
                path.pop();
            }
            Item::ArrayOfTables(array) => {
                for (index, child) in array.iter().enumerate() {
                    path.push(Seg::Key(key.to_string()));
                    path.push(Seg::Index(index));
                    consider(child, path, best);
                    find_next(child, after, path, best);
                    path.pop();
                    path.pop();
                }
            }
            _ => {}
        }
    }
}

fn table_at<'a>(root: &'a mut Table, path: &[Seg]) -> Option<&'a mut Table> {
    let mut current = root;
    let mut i = 0;
    while i < path.len() {
        let Seg::Key(key) = &path[i] else {
            return None;
        };
        match current.get_mut(key)? {
            Item::Table(child) => {
                current = child;
                i += 1;
            }
            Item::ArrayOfTables(array) => {
                let Some(Seg::Index(index)) = path.get(i + 1) else {
                    return None;
                };
                current = array.get_mut(*index)?;
                i += 2;
            }
            _ => return None,
        }
    }
    Some(current)
}

/// Comments sit in the prefix of the header that follows them. When a table is removed, move
/// its prefix to the next header, or to the end of the file, so no user comment is lost.
fn carry_forward(doc: &mut DocumentMut, after: Option<i64>, text: String) {
    if text.is_empty() {
        return;
    }
    let mut best = None;
    find_next(
        doc.as_table(),
        after.unwrap_or(-1),
        &mut Vec::new(),
        &mut best,
    );
    if let Some(table) = best.and_then(|(_, path)| table_at(doc.as_table_mut(), &path)) {
        let existing = prefix_of(table);
        table.decor_mut().set_prefix(format!("{text}{existing}"));
    } else {
        let existing = doc.trailing().as_str().unwrap_or("").to_string();
        doc.set_trailing(format!("{text}{existing}"));
    }
}

/// Herdr's config file: `HERDR_CONFIG_PATH`, else `~/.config/herdr/config.toml`.
pub fn herdr_config_path() -> PathBuf {
    std::env::var_os("HERDR_CONFIG_PATH")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default())
                .join(".config/herdr/config.toml")
        })
}

fn owned_path(state_dir: &Path) -> PathBuf {
    state_dir.join("owned.json")
}

pub fn load_owned(state_dir: &Path) -> Owned {
    fs::read_to_string(owned_path(state_dir))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save_owned(state_dir: &Path, owned: &Owned) -> Result<()> {
    fs::create_dir_all(state_dir)?;
    fs::write(owned_path(state_dir), serde_json::to_string_pretty(owned)?)?;
    Ok(())
}

fn read_config(path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(err).with_context(|| format!("cannot read {}", path.display())),
    }
}

fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

fn pause() -> Result<()> {
    print!("\nPress enter to close.");
    std::io::stdout().flush()?;
    std::io::stdin().read_line(&mut String::new())?;
    Ok(())
}

/// Back up, write, and run `herdr config check`; restore the old text if the check fails.
fn write_checked(ctx: &Ctx, path: &Path, original: &str, updated: &str) -> Result<bool> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let backup = path.with_extension(format!("toml.bak-{}", unix_ms() / 1000));
    fs::write(&backup, original)?;
    fs::write(path, updated)?;
    if ctx.herdr.config_check()? {
        println!("Backup: {}", backup.display());
        return Ok(true);
    }
    fs::write(path, original)?;
    println!(
        "herdr config check failed, so your config was restored. Backup: {}",
        backup.display()
    );
    Ok(false)
}

pub fn run_configure(ctx: &Ctx) -> Result<()> {
    let path = herdr_config_path();
    let original = read_config(&path)?;
    let doc: DocumentMut = original
        .parse()
        .with_context(|| format!("{} is not valid TOML", path.display()))?;
    let mut owned = load_owned(&ctx.state_dir);
    if let Err(reason) = owned.check_for(&path) {
        println!("{reason}");
        return pause();
    }
    let plan = plan(&doc);
    println!("herdr-ddev configure - {}\n", path.display());
    for line in describe(&plan) {
        println!("  {line}");
    }
    if plan.add.is_empty() && plan.sidebar == SidebarChange::None {
        println!("\nNothing to change.");
        return pause();
    }
    if !confirm("\nApply these changes? [y/N] ")? {
        return Ok(());
    }
    let applied = apply_text(&original, &plan, &path)?;
    if write_checked(ctx, &path, &original, &applied.text)? {
        owned.merge(applied.owned);
        save_owned(&ctx.state_dir, &owned)?;
        let _ = ctx.herdr.reload_config();
        println!("Done. Herdr reloaded its config.");
    }
    pause()
}

pub fn run_unconfigure(ctx: &Ctx) -> Result<()> {
    let owned = load_owned(&ctx.state_dir);
    if owned.keys.is_empty() && owned.sidebar == SidebarChange::None {
        println!("herdr-ddev has not added anything to your Herdr config.");
        return pause();
    }
    let path = herdr_config_path();
    if let Err(reason) = owned.check_for(&path) {
        println!("{reason}");
        return pause();
    }
    let original = read_config(&path)?;
    println!("herdr-ddev unconfigure - {}\n", path.display());
    for key in &owned.keys {
        println!("  remove key {} -> {}", key.key, key.command);
    }
    if owned.sidebar != SidebarChange::None {
        println!("  remove the $ddev badge from the sidebar");
    }
    if !confirm("\nRemove these? [y/N] ")? {
        return Ok(());
    }
    let updated = unapply_text(&original, &owned)?;
    if write_checked(ctx, &path, &original, &updated)? {
        let _ = fs::remove_file(owned_path(&ctx.state_dir));
        let _ = ctx.herdr.reload_config();
        println!("Done. Herdr reloaded its config.");
    }
    pause()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/herdr-config.toml");
    const DEFAULT_TEMPLATE: &str = include_str!("../tests/fixtures/herdr-default-config.toml");
    const LEGACY: &str = include_str!("../tests/fixtures/herdr-config-legacy.toml");

    fn doc(text: &str) -> DocumentMut {
        text.parse().unwrap()
    }

    fn settings(text: &str) -> toml::Table {
        toml::from_str(text).unwrap()
    }

    fn config_path() -> PathBuf {
        PathBuf::from("/home/u/.config/herdr/config.toml")
    }

    fn configure(text: &str) -> Applied {
        apply_text(text, &plan(&doc(text)), &config_path()).unwrap()
    }

    /// What the first version of configure recorded: no config path.
    fn legacy_owned() -> Owned {
        Owned {
            config_path: None,
            keys: BINDINGS
                .iter()
                .map(|b| OwnedKey {
                    key: b.key.into(),
                    command: b.command.into(),
                })
                .collect(),
            sidebar: SidebarChange::CreatedRows,
        }
    }

    #[test]
    fn empty_config_gets_all_keys_and_default_rows() {
        let plan = plan(&doc(""));
        assert_eq!(plan.add.len(), 3);
        assert!(plan.skipped.is_empty());
        assert_eq!(plan.sidebar, SidebarChange::CreatedRows);
    }

    #[test]
    fn keys_in_use_are_skipped_and_explained() {
        let text = concat!(
            "[keys]\nsettings = [\"prefix+s\", \"prefix+shift+s\"]\n",
            "[[keys.command]]\nkey = \"prefix+shift+o\"\ntype = \"shell\"\ncommand = \"x\"\n"
        );
        let plan = plan(&doc(text));
        assert_eq!(
            plan.add.iter().map(|b| b.key).collect::<Vec<_>>(),
            ["prefix+shift+e"]
        );
        assert!(plan.skipped[0].contains("prefix+shift+s is already bound to settings"));
        assert!(plan.skipped[1].contains("prefix+shift+o is already bound to x"));
    }

    #[test]
    fn existing_ddev_bindings_are_recognized() {
        let applied = configure("");
        let again = plan(&doc(&applied.text));
        assert!(again.add.is_empty());
        assert_eq!(again.sidebar, SidebarChange::None);
        assert!(again.skipped[0].contains("already runs danjuls.ddev.toggle"));
    }

    #[test]
    fn custom_rows_get_an_appended_ddev_row() {
        let text = "[ui.sidebar.spaces]\nrows = [[\"workspace\"]]\n";
        assert_eq!(plan(&doc(text)).sidebar, SidebarChange::AppendedRow);
    }

    #[test]
    fn rows_that_already_show_ddev_are_left_alone() {
        let text = "[ui.sidebar.spaces]\nrows = [[\"workspace\", \"$ddev\"]]\n";
        assert_eq!(plan(&doc(text)).sidebar, SidebarChange::None);
    }

    #[test]
    fn a_spaces_table_without_rows_gets_rows_inside_it() {
        let text = "[ui.sidebar.spaces]\nrow_gap = 1\n";
        assert_eq!(plan(&doc(text)).sidebar, SidebarChange::InsertedRows);
        let parsed = settings(&configure(text).text);
        assert_eq!(
            parsed["ui"]["sidebar"]["spaces"]["row_gap"].as_integer(),
            Some(1)
        );
        assert_eq!(
            parsed["ui"]["sidebar"]["spaces"]["rows"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn inline_tables_are_skipped_instead_of_breaking_the_file() {
        let text = "keys = { prefix = \"ctrl+a\" }\nui = { toast = { delivery = \"system\" } }\n";
        let plan = plan(&doc(text));
        assert!(plan.add.is_empty());
        assert_eq!(plan.sidebar, SidebarChange::None);
        assert!(
            plan.skipped.iter().any(|s| s.contains("`keys`")),
            "{:?}",
            plan.skipped
        );
        assert!(
            plan.skipped.iter().any(|s| s.contains("`ui`")),
            "{:?}",
            plan.skipped
        );
    }

    #[test]
    fn apply_writes_bindings_a_styled_token_and_the_config_path() {
        let applied = configure("");
        let parsed = settings(&applied.text);
        let commands = parsed["keys"]["command"].as_array().unwrap();
        assert_eq!(commands.len(), 3);
        assert_eq!(commands[0]["type"].as_str(), Some("plugin_action"));
        assert_eq!(commands[0]["command"].as_str(), Some("danjuls.ddev.toggle"));
        let rows = parsed["ui"]["sidebar"]["spaces"]["rows"]
            .as_array()
            .unwrap();
        let token = rows[1].as_array().unwrap()[2].as_table().unwrap();
        assert_eq!(token["token"].as_str(), Some("$ddev"));
        assert_eq!(token["rules"].as_array().unwrap().len(), 4);
        assert_eq!(applied.owned.keys.len(), 3);
        assert_eq!(applied.owned.sidebar, SidebarChange::CreatedRows);
        assert_eq!(
            applied.owned.config_path.as_deref(),
            Some(config_path().as_path())
        );
    }

    #[test]
    fn default_template_keeps_every_comment_under_its_own_header() {
        let applied = configure(DEFAULT_TEMPLATE);
        assert!(
            applied.text.starts_with(DEFAULT_TEMPLATE),
            "configure must only append"
        );
        let added = &applied.text[DEFAULT_TEMPLATE.len()..];
        assert!(
            added.contains(BLOCK_START) && added.contains(BLOCK_END),
            "{added}"
        );
        settings(&applied.text);
    }

    #[test]
    fn unconfigure_restores_the_default_template_exactly() {
        let applied = configure(DEFAULT_TEMPLATE);
        assert_eq!(
            unapply_text(&applied.text, &applied.owned).unwrap(),
            DEFAULT_TEMPLATE
        );
    }

    #[test]
    fn unconfigure_restores_small_configs_exactly() {
        for original in [
            FIXTURE,
            "",
            "onboarding = false\n",
            "[ui.sidebar.spaces]\nrow_gap = 1\n",
        ] {
            let applied = configure(original);
            assert_eq!(
                unapply_text(&applied.text, &applied.owned).unwrap(),
                original
            );
        }
    }

    #[test]
    fn unconfigure_keeps_rows_the_user_edited_after_configure() {
        let applied = configure("");
        let edited = applied.text.replace(
            r#"["state_icon", "workspace"]"#,
            r#"["state_icon", "workspace", "$jj_status"]"#,
        );
        assert_ne!(edited, applied.text);
        let after = unapply_text(&edited, &applied.owned).unwrap();
        let parsed = settings(&after);
        let rows = parsed["ui"]["sidebar"]["spaces"]["rows"]
            .as_array()
            .unwrap();
        assert_eq!(rows[0].as_array().unwrap().len(), 3);
        assert!(!after.contains("$ddev"));
        assert!(!after.contains("herdr-ddev"));
        assert!(parsed.get("keys").is_none());
    }

    #[test]
    fn unconfigure_removes_only_the_appended_row() {
        let text = "[ui.sidebar.spaces]\nrows = [[\"workspace\"]]\n";
        let applied = configure(text);
        let after = unapply_text(&applied.text, &applied.owned).unwrap();
        assert_eq!(settings(&after), settings(text));
    }

    #[test]
    fn unconfigure_cleans_up_what_the_first_version_wrote() {
        let after = unapply_text(LEGACY, &legacy_owned()).unwrap();
        assert_eq!(settings(&after), settings(FIXTURE));
        assert!(after.contains("# reviewr") && after.contains("# vim-herdr-navigation"));
    }

    #[test]
    fn owned_changes_for_another_config_are_refused() {
        let mut owned = legacy_owned();
        assert!(owned.check_for(&config_path()).is_ok());
        owned.config_path = Some(PathBuf::from("/tmp/scratch/config.toml"));
        let err = owned.check_for(&config_path()).unwrap_err();
        assert!(err.contains("/tmp/scratch/config.toml"), "{err}");
        owned.config_path = Some(config_path());
        assert!(owned.check_for(&config_path()).is_ok());
    }

    #[test]
    fn owned_merge_keeps_unique_keys_the_first_sidebar_change_and_the_path() {
        let key = |k: &str| OwnedKey {
            key: k.into(),
            command: "c".into(),
        };
        let mut owned = Owned {
            config_path: None,
            keys: vec![key("a")],
            sidebar: SidebarChange::CreatedRows,
        };
        owned.merge(Owned {
            config_path: Some(config_path()),
            keys: vec![key("a"), key("b")],
            sidebar: SidebarChange::AppendedRow,
        });
        assert_eq!(owned.keys, [key("a"), key("b")]);
        assert_eq!(owned.sidebar, SidebarChange::CreatedRows);
        assert_eq!(owned.config_path, Some(config_path()));
    }

    #[test]
    fn owned_round_trips_through_the_state_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_owned(dir.path()), Owned::default());
        let owned = configure("").owned;
        save_owned(dir.path(), &owned).unwrap();
        assert_eq!(load_owned(dir.path()), owned);
    }

    #[test]
    fn describe_lists_every_change() {
        let lines = describe(&plan(&doc("")));
        assert_eq!(lines.len(), 4);
        assert!(lines[0].starts_with("add key prefix+shift+s"));
        assert!(lines[3].contains("$ddev"));
    }

    #[test]
    #[ignore = "needs herdr on PATH; run with cargo test -- --ignored"]
    fn applied_config_passes_herdr_config_check() {
        for original in [FIXTURE, "", DEFAULT_TEMPLATE] {
            let applied = configure(original);
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            std::fs::write(&path, &applied.text).unwrap();
            let out = std::process::Command::new("herdr")
                .args(["config", "check"])
                .env("HERDR_CONFIG_PATH", &path)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stdout)
            );
        }
    }
}
