//! Adding and removing the `$ddev` badge and keybindings in Herdr's `config.toml`.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value, value};

use crate::app::{Ctx, unix_ms};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SidebarChange {
    #[default]
    None,
    /// No `rows` existed: the defaults were written with `$ddev` added to the second row.
    CreatedRows,
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
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub add: Vec<Binding>,
    pub skipped: Vec<String>,
    pub sidebar: SidebarChange,
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

fn rows(doc: &DocumentMut) -> Option<&Array> {
    doc.get("ui")?
        .get("sidebar")?
        .get("spaces")?
        .get("rows")?
        .as_array()
}

fn rows_mut(doc: &mut DocumentMut) -> Option<&mut Array> {
    doc.get_mut("ui")?
        .get_mut("sidebar")?
        .get_mut("spaces")?
        .get_mut("rows")?
        .as_array_mut()
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
    let sidebar = match rows(doc) {
        None => SidebarChange::CreatedRows,
        Some(rows) if has_ddev(rows) => SidebarChange::None,
        Some(_) => SidebarChange::AppendedRow,
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
            SidebarChange::AppendedRow => "add the $ddev badge as a new sidebar row",
            SidebarChange::None => "the sidebar already shows $ddev",
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

fn implicit_table() -> Item {
    let mut table = Table::new();
    table.set_implicit(true);
    Item::Table(table)
}

fn command_array(doc: &mut DocumentMut) -> Result<&mut ArrayOfTables> {
    let keys = doc
        .entry("keys")
        .or_insert(implicit_table())
        .as_table_mut()
        .context("`keys` in config.toml is not a table")?;
    keys.entry("command")
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()))
        .as_array_of_tables_mut()
        .context("`keys.command` in config.toml is not a list of tables")
}

fn spaces_table(doc: &mut DocumentMut) -> Result<&mut Table> {
    let ui = doc
        .entry("ui")
        .or_insert(implicit_table())
        .as_table_mut()
        .context("`ui` is not a table")?;
    let sidebar = ui
        .entry("sidebar")
        .or_insert(implicit_table())
        .as_table_mut()
        .context("`ui.sidebar` is not a table")?;
    sidebar
        .entry("spaces")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .context("`ui.sidebar.spaces` is not a table")
}

pub fn apply(doc: &mut DocumentMut, plan: &Plan) -> Result<Owned> {
    let mut owned = Owned {
        keys: Vec::new(),
        sidebar: plan.sidebar,
    };
    if !plan.add.is_empty() {
        let commands = command_array(doc)?;
        for binding in &plan.add {
            let mut table = Table::new();
            table["key"] = value(binding.key);
            table["type"] = value("plugin_action");
            table["command"] = value(binding.command);
            table["description"] = value(binding.description);
            commands.push(table);
            owned.keys.push(OwnedKey {
                key: binding.key.to_string(),
                command: binding.command.to_string(),
            });
        }
    }
    match plan.sidebar {
        SidebarChange::None => {}
        SidebarChange::CreatedRows => {
            spaces_table(doc)?.insert("rows", value(default_rows_with_ddev()));
        }
        SidebarChange::AppendedRow => {
            let rows = rows_mut(doc).context("sidebar rows disappeared")?;
            let mut row = Array::new();
            row.push(ddev_token());
            rows.push(row);
        }
    }
    Ok(owned)
}

pub fn unapply(doc: &mut DocumentMut, owned: &Owned) {
    remove_owned_keys(doc, &owned.keys);
    match owned.sidebar {
        SidebarChange::None => {}
        SidebarChange::CreatedRows => remove_created_rows(doc),
        SidebarChange::AppendedRow => {
            if let Some(rows) = rows_mut(doc) {
                rows.retain(|row| {
                    !row.as_array()
                        .is_some_and(|r| r.len() == 1 && r.iter().all(is_ddev_token))
                });
            }
        }
    }
}

fn remove_owned_keys(doc: &mut DocumentMut, owned: &[OwnedKey]) {
    let Some(keys) = doc.get_mut("keys").and_then(Item::as_table_mut) else {
        return;
    };
    if let Some(commands) = keys
        .get_mut("command")
        .and_then(Item::as_array_of_tables_mut)
    {
        let is_owned = |table: &Table| {
            owned.iter().any(|o| {
                table.get("key").and_then(Item::as_str) == Some(o.key.as_str())
                    && table.get("command").and_then(Item::as_str) == Some(o.command.as_str())
            })
        };
        let mut index = 0;
        while index < commands.len() {
            if commands.get(index).is_some_and(is_owned) {
                commands.remove(index);
            } else {
                index += 1;
            }
        }
        if commands.is_empty() {
            keys.remove("command");
        }
    }
    if keys.is_empty() {
        doc.remove("keys");
    }
}

fn remove_created_rows(doc: &mut DocumentMut) {
    let Some(ui) = doc.get_mut("ui").and_then(Item::as_table_mut) else {
        return;
    };
    if let Some(sidebar) = ui.get_mut("sidebar").and_then(Item::as_table_mut) {
        if let Some(spaces) = sidebar.get_mut("spaces").and_then(Item::as_table_mut) {
            spaces.remove("rows");
            if spaces.is_empty() {
                sidebar.remove("spaces");
            }
        }
        if sidebar.is_empty() {
            ui.remove("sidebar");
        }
    }
    if ui.is_empty() {
        doc.remove("ui");
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
    let mut doc: DocumentMut = original
        .parse()
        .with_context(|| format!("{} is not valid TOML", path.display()))?;
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
    let added = apply(&mut doc, &plan)?;
    if write_checked(ctx, &path, &original, &doc.to_string())? {
        let mut owned = load_owned(&ctx.state_dir);
        owned.merge(added);
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
    let original = read_config(&path)?;
    let mut doc: DocumentMut = original
        .parse()
        .with_context(|| format!("{} is not valid TOML", path.display()))?;
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
    unapply(&mut doc, &owned);
    if write_checked(ctx, &path, &original, &doc.to_string())? {
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

    fn doc(text: &str) -> DocumentMut {
        text.parse().unwrap()
    }

    fn settings(text: &str) -> toml::Table {
        toml::from_str(text).unwrap()
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
        let mut first = doc("");
        let plan_one = plan(&first);
        apply(&mut first, &plan_one).unwrap();
        let again = plan(&first);
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
    fn apply_writes_bindings_and_a_styled_token() {
        let mut document = doc("");
        let plan = plan(&document);
        let owned = apply(&mut document, &plan).unwrap();
        let text = document.to_string();
        let parsed = settings(&text);
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
        assert_eq!(owned.keys.len(), 3);
        assert_eq!(owned.sidebar, SidebarChange::CreatedRows);
    }

    #[test]
    fn apply_keeps_comments_and_other_bindings() {
        let mut document = doc(FIXTURE);
        let plan = plan(&document);
        apply(&mut document, &plan).unwrap();
        let text = document.to_string();
        assert!(text.contains("# vim-herdr-navigation"));
        assert!(text.contains("# reviewr"));
        let keys: Vec<String> = used_keys(&document).into_iter().map(|(k, _)| k).collect();
        assert!(keys.contains(&"prefix+d".to_string()));
        assert!(keys.contains(&"prefix+shift+e".to_string()));
    }

    #[test]
    fn unapply_returns_the_same_settings() {
        for original in [FIXTURE, ""] {
            let mut document = doc(original);
            let plan = plan(&document);
            let owned = apply(&mut document, &plan).unwrap();
            unapply(&mut document, &owned);
            let after = document.to_string();
            assert_eq!(settings(&after), settings(original));
            assert!(after.contains("# reviewr") || original.is_empty());
        }
    }

    #[test]
    fn unapply_removes_only_the_appended_row() {
        let text = "[ui.sidebar.spaces]\nrows = [[\"workspace\"]]\n";
        let mut document = doc(text);
        let plan = plan(&document);
        let owned = apply(&mut document, &plan).unwrap();
        unapply(&mut document, &owned);
        assert_eq!(settings(&document.to_string()), settings(text));
    }

    #[test]
    fn owned_merge_keeps_unique_keys_and_the_first_sidebar_change() {
        let key = |k: &str| OwnedKey {
            key: k.into(),
            command: "c".into(),
        };
        let mut owned = Owned {
            keys: vec![key("a")],
            sidebar: SidebarChange::CreatedRows,
        };
        owned.merge(Owned {
            keys: vec![key("a"), key("b")],
            sidebar: SidebarChange::AppendedRow,
        });
        assert_eq!(owned.keys, [key("a"), key("b")]);
        assert_eq!(owned.sidebar, SidebarChange::CreatedRows);
    }

    #[test]
    fn owned_round_trips_through_the_state_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_owned(dir.path()), Owned::default());
        let owned = Owned {
            keys: vec![OwnedKey {
                key: "prefix+shift+s".into(),
                command: "danjuls.ddev.toggle".into(),
            }],
            sidebar: SidebarChange::AppendedRow,
        };
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
        for original in [FIXTURE, ""] {
            let mut document = doc(original);
            let plan = plan(&document);
            apply(&mut document, &plan).unwrap();
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.toml");
            std::fs::write(&path, document.to_string()).unwrap();
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
