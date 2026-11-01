// SPDX-License-Identifier: GPL-3.0-or-later
//! The `tileide.*` API. Each call merges its options into the shared `Builder`,
//! so a later file (e.g. a rice pulled in with `require`) overrides earlier values.

use std::cell::RefCell;
use std::rc::Rc;

use mlua::{Lua, Result, Table, Value, Variadic};
use tileide_core::layout::LayoutKind;
use tileide_core::tiles::{Axis, Direction};

use crate::fields::{Fields, error};
use crate::model::*;

#[derive(Default)]
pub struct Builder {
    pub config: Config,
    pub warnings: Vec<String>,
}

pub type Shared = Rc<RefCell<Builder>>;

/// `file:line` of the Lua code that called the current Rust function.
pub fn caller(lua: &Lua) -> String {
    lua.inspect_stack(1, |debug| {
        let source = debug.source();
        let file = source.short_src.as_deref().unwrap_or("?").to_string();
        match debug.current_line() {
            Some(line) => format!("{file}:{line}"),
            None => file,
        }
    })
    .unwrap_or_else(|| "?".into())
}

fn options(lua: &Lua, section: &str, value: Value) -> Result<Fields> {
    let location = caller(lua);
    match value {
        Value::Table(t) => Fields::new(section, &location, t),
        Value::Nil => Fields::new(section, &location, lua.create_table()?),
        other => Err(error(&location, &format!("{section}: expected a table, got {}", other.type_name()))),
    }
}

fn layout_kind(f: &mut Fields, key: &str) -> Result<Option<LayoutKind>> {
    Ok(f.choice(key, &["dwindle", "columns"])?.map(|v| if v == "columns" { LayoutKind::Columns } else { LayoutKind::Dwindle }))
}

/// Builds the read-only `tileide` table: writes to it raise an error.
pub fn install(lua: &Lua, shared: &Shared) -> Result<Table> {
    let api = lua.create_table()?;

    let b = shared.clone();
    api.set("keys", lua.create_function(move |lua, value: Value| {
        let mut f = options(lua, "keys", value)?;
        let mut b = b.borrow_mut();
        if let Some(m) = f.choice("mod", MODIFIERS)? {
            if m == "SUPER" {
                b.warnings.push(format!("{}: warning: keys.mod = \"SUPER\" usually collides with your window manager", caller(lua)));
            }
            b.config.modifier = m;
        }
        f.finish(&mut b.warnings);
        Ok(())
    })?)?;

    let b = shared.clone();
    api.set("appearance", lua.create_function(move |lua, value: Value| {
        let mut f = options(lua, "appearance", value)?;
        let mut b = b.borrow_mut();
        let a = &mut b.config.appearance;
        if let Some(v) = f.string("theme")? { a.theme = Some(v) }
        if let Some(v) = f.uint("window_radius", 60)? { a.window_radius = v }
        if let Some(v) = f.boolean("animations")? { a.animations = v }
        f.accept(&["opacity", "blur_hint"]);
        f.finish(&mut b.warnings);
        Ok(())
    })?)?;

    let b = shared.clone();
    api.set("sidebar", lua.create_function(move |lua, value: Value| {
        let mut f = options(lua, "sidebar", value)?;
        let mut b = b.borrow_mut();
        let s = &mut b.config.sidebar;
        if let Some(v) = f.choice("start_state", &["collapsed", "expanded"])? { s.start_expanded = v == "expanded" }
        for (key, slot) in [
            ("show_launcher", &mut s.show_launcher),
            ("show_workspaces", &mut s.show_workspaces),
            ("show_label", &mut s.show_label),
            ("show_clock", &mut s.show_clock),
            ("show_files", &mut s.show_files),
            ("show_themes", &mut s.show_themes),
            ("show_config", &mut s.show_config),
        ] {
            if let Some(v) = f.boolean(key)? { *slot = v }
        }
        f.accept(&["drawer_fillet", "show_system", "show_extensions"]);
        f.finish(&mut b.warnings);
        Ok(())
    })?)?;

    let b = shared.clone();
    api.set("background", lua.create_function(move |lua, value: Value| {
        let mut f = options(lua, "background", value)?;
        let mut b = b.borrow_mut();
        b.config.background_set = true;
        read_background(&mut f, &mut b.config.background)?;
        f.finish(&mut b.warnings);
        Ok(())
    })?)?;

    let b = shared.clone();
    api.set("tiling", lua.create_function(move |lua, value: Value| {
        let mut f = options(lua, "tiling", value)?;
        let mut b = b.borrow_mut();
        let t = &mut b.config.tiling;
        if let Some(v) = layout_kind(&mut f, "layout")? { t.layout = v }
        if let Some(v) = f.uint("gaps_in", 100)? { t.gaps_in = v }
        if let Some(v) = f.uint("gaps_out", 100)? { t.gaps_out = v }
        if let Some(v) = f.number("column_width", 0.2, 1.0)? { t.column_width = v }
        f.finish(&mut b.warnings);
        Ok(())
    })?)?;

    let b = shared.clone();
    api.set("bind", lua.create_function(move |lua, args: Variadic<Value>| {
        let location = caller(lua);
        let text = |i: usize, what: &str| -> Result<String> {
            match args.get(i) {
                Some(Value::String(s)) => Ok(s.to_str()?.to_string()),
                other => Err(error(&location, &format!(
                    "bind: argument {} ({what}) must be a string, got {}",
                    i + 1,
                    other.map_or("nothing", |v| v.type_name())
                ))),
            }
        };
        let (mods, key, action) = (text(0, "mods")?, text(1, "key")?, text(2, "action")?);
        let mods = normalise_mods(mods.split_whitespace().map(str::to_uppercase));
        if let Some(bad) = mods.iter().find(|m| !MODIFIERS.contains(&m.as_str())) {
            return Err(error(&location, &format!("bind: unknown modifier \"{bad}\" (use {})", MODIFIERS.join(", "))));
        }
        if key.is_empty() {
            return Err(error(&location, "bind: the key is empty"));
        }
        let arg = match args.get(3) {
            None | Some(Value::Nil) => None,
            Some(Value::Integer(i)) => Some(i.to_string()),
            Some(Value::Number(n)) if n.fract() == 0.0 => Some((*n as i64).to_string()),
            Some(Value::String(s)) => Some(s.to_str()?.to_string()),
            Some(other) => return Err(error(&location, &format!("bind: argument 4 must be an integer or string, got {}", other.type_name()))),
        };
        let action = parse_action(&action, arg.as_deref()).map_err(|m| error(&location, &format!("bind: {m}")))?;
        let mut b = b.borrow_mut();
        if let Action::Unavailable(name) = &action {
            b.warnings.push(format!("{location}: warning: bind: \"{name}\" is not available in this version yet; the key does nothing"));
        }
        b.config.binds.push(Bind { mods, key, action });
        Ok(())
    })?)?;

    let b = shared.clone();
    api.set("workspace", lua.create_function(move |lua, (id, value): (Value, Value)| {
        let id = match id {
            Value::Integer(i) if (1..=MAX_WORKSPACE as i64).contains(&i) => i as u8,
            Value::Number(n) if n.fract() == 0.0 && (1.0..=MAX_WORKSPACE as f64).contains(&n) => n as u8,
            _ => return Err(error(&caller(lua), &format!("workspace: id must be an integer 1..{MAX_WORKSPACE}"))),
        };
        let mut f = options(lua, &format!("workspace({id})"), value)?;
        let mut b = b.borrow_mut();
        let w = b.config.workspaces.entry(id).or_default();
        if let Some(v) = f.string("name")? { w.name = Some(v) }
        if let Some(v) = layout_kind(&mut f, "layout")? { w.layout = Some(v) }
        if let Some(v) = f.string("folder")? { w.folder = Some(v) }
        if let Some(v) = f.string("theme")? { w.theme = Some(v) }
        if let Some(v) = f.string("background")? { w.background = Some(v) }
        f.accept(&["extensions"]);
        f.finish(&mut b.warnings);
        Ok(())
    })?)?;

    let b = shared.clone();
    api.set("lsp", lua.create_function(move |lua, (name, value): (Value, Value)| {
        let Value::String(name) = name else {
            return Err(error(&caller(lua), "lsp: argument 1 must be a server name, e.g. \"rust\""));
        };
        let name = name.to_str()?.to_string();
        let mut f = options(lua, &format!("lsp(\"{name}\")"), value)?;
        let mut b = b.borrow_mut();
        let entry = b.config.lsp.entry(name).or_default();
        if let Some(v) = f.command("cmd")? { entry.cmd = Some(v) }
        if let Some(v) = f.boolean("enabled")? { entry.enabled = v }
        f.finish(&mut b.warnings);
        Ok(())
    })?)?;

    // Later versions (v0.3 extensions, v0.4 toolchains): accepted so configs written
    // for them load today, and ignored.
    for func in ["extension", "toolchain"] {
        api.set(func, lua.create_function(|_, _: Variadic<Value>| Ok(()))?)?;
    }

    read_only(lua, api)
}

/// `tileide.background { … }` keys; also the picker's choice in `state.lua`.
pub fn read_background(f: &mut Fields, bg: &mut Background) -> Result<()> {
    if let Some(v) = f.choice("mode", BACKGROUND_MODES)? { bg.mode = v }
    if let Some(v) = f.string("source")? { bg.source = Some(v) }
    if let Some(v) = f.uint("dim", 100)? { bg.dim = v }
    if let Some(v) = f.uint("blur", 200)? { bg.blur = v }
    if let Some(v) = f.number("tile_opacity", MIN_TILE_OPACITY, 1.0)? { bg.tile_opacity = v }
    if let Some(v) = f.choice("fit", FITS)? { bg.fit = v }
    Ok(())
}

fn direction(arg: Option<&str>) -> std::result::Result<Direction, String> {
    match arg {
        Some("left") => Ok(Direction::Left),
        Some("right") => Ok(Direction::Right),
        Some("up") => Ok(Direction::Up),
        Some("down") => Ok(Direction::Down),
        other => Err(format!("expected left, right, up or down, got {}", other.unwrap_or("nothing"))),
    }
}

/// `action` plus its optional argument (numbers arrive as their decimal text).
pub fn parse_action(action: &str, arg: Option<&str>) -> std::result::Result<Action, String> {
    let with = |what: &str| format!("\"{action}\": {what}");
    Ok(match action {
        "workspace" => match arg {
            Some("config") => Action::ConfigWorkspace,
            Some(n) => match n.parse::<u8>() {
                Ok(n) if (1..=MAX_WORKSPACE).contains(&n) => Action::Workspace(n),
                _ => return Err(with(&format!("expected 1..{MAX_WORKSPACE} or \"config\", got {n}"))),
            },
            None => return Err(with("needs a workspace number or \"config\"")),
        },
        "sidebar" => Action::Sidebar,
        "layout" => match arg {
            None | Some("toggle") => Action::Layout(None),
            Some("dwindle") => Action::Layout(Some(LayoutKind::Dwindle)),
            Some("columns") => Action::Layout(Some(LayoutKind::Columns)),
            Some(other) => return Err(with(&format!("expected toggle, dwindle or columns, got {other}"))),
        },
        "split" => match arg {
            Some("vertical") => Action::Split(Axis::Column),
            Some("horizontal") => Action::Split(Axis::Row),
            other => return Err(with(&format!("expected vertical or horizontal, got {}", other.unwrap_or("nothing")))),
        },
        "focus" => Action::Focus(direction(arg).map_err(|e| with(&e))?),
        "move" | "swap" => Action::Move(direction(arg).map_err(|e| with(&e))?),
        "resize" => Action::Resize(direction(arg).map_err(|e| with(&e))?),
        "close" => Action::Close,
        "terminal" => Action::Terminal,
        "editor" => Action::Editor,
        "search" => Action::Search,
        "wrap" => Action::Wrap,
        "save" => Action::Save,
        "quit" => Action::Quit,
        "overview" => Action::Overview,
        "themes" => Action::Themes,
        "folder" => Action::Folder,
        "themes_section" => Action::ThemesSection,
        "workspace_close" => Action::WorkspaceClose,
        "hover" => Action::Hover,
        "definition" => Action::Definition,
        "complete" => Action::Complete,
        "moveto" | "palette" | "run" => Action::Unavailable(action.into()),
        other => return Err(format!("unknown action \"{other}\"")),
    })
}

/// Wraps `inner` in an empty proxy whose writes fail, so a config or mod
/// cannot replace API functions for code that runs after it.
pub fn read_only(lua: &Lua, inner: Table) -> Result<Table> {
    let proxy = lua.create_table()?;
    let meta = lua.create_table()?;
    meta.set("__index", inner)?;
    meta.set("__newindex", lua.create_function(|lua, _: Variadic<Value>| -> Result<()> {
        Err(error(&caller(lua), "the tileide table is read-only"))
    })?)?;
    meta.set("__metatable", false)?;
    proxy.set_metatable(Some(meta))?;
    Ok(proxy)
}
