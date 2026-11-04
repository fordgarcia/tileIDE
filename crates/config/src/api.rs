// SPDX-License-Identifier: GPL-3.0-or-later
//! The `tileide.*` API. Each call merges its options into the shared `Builder`,
//! so a later file (e.g. a rice pulled in with `require`) overrides earlier values.

use std::cell::RefCell;
use std::rc::Rc;

use mlua::{Lua, Result, Table, Value, Variadic};
use tileide_core::layout::LayoutKind;
use tileide_core::motion::{Curve, MAX_SPEED, MIN_SPEED};
use tileide_core::theme::Edge;
use tileide_core::token::{BORDER_MAX, Rice};
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
    Ok(f.choice(key, &["dwindle", "columns", "stack"])?.map(|v| match v.as_str() {
        "columns" => LayoutKind::Columns,
        "stack" => LayoutKind::Stack,
        _ => LayoutKind::Dwindle,
    }))
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
        let mut set = Vec::new();
        let a = &mut b.config.appearance;
        if let Some(v) = f.string("theme")? { a.theme = Some(v) }
        if let Some(v) = f.uint("window_radius", 60)? { a.window_radius = v; set.push("window_radius") }
        if let Some(v) = f.boolean("animations")? { a.animations = v; set.push("animations") }
        if let Some(v) = f.choice("motion", &["fluid", "snappy"])? { a.motion = Curve::from_key(&v).unwrap_or_default(); set.push("motion") }
        if let Some(v) = f.number("motion_speed", MIN_SPEED, MAX_SPEED)? { a.motion_speed = v; set.push("motion_speed") }
        if let Some(v) = f.string("ui_font")? { a.ui_font = v; set.push("ui_font") }
        if let Some(v) = f.number("ui_scale", crate::settings::UI_SCALE.0, crate::settings::UI_SCALE.1)? { a.ui_scale = v; set.push("ui_scale") }
        if let Some(v) = f.string("code_font")? { a.code_font = v; set.push("code_font") }
        if let Some(v) = f.number("code_size", crate::settings::CODE_SIZE.0, crate::settings::CODE_SIZE.1)? { a.code_size = v; set.push("code_size") }
        if let Some(v) = f.number("tile_transparency", 0.0, crate::settings::TRANSPARENCY_MAX)? { a.tile_transparency = Some(v); set.push("tile_transparency") }
        if let Some(v) = f.number("tile_blur", 0.0, crate::settings::BLUR_MAX)? { a.tile_blur = Some(v); set.push("tile_blur") }
        if let Some(v) = f.number("tile_border", 0.0, BORDER_MAX)? { a.tile_border = v; set.push("tile_border") }
        if let Some(v) = f.choice("tile_border_color", &Edge::KEYS)? { a.tile_border_color = Edge::from_key(&v).unwrap_or_default(); set.push("tile_border_color") }
        if let Some(v) = f.number("frame_border", 0.0, BORDER_MAX)? { a.frame_border = v; set.push("frame_border") }
        if let Some(v) = f.choice("frame_border_color", &Edge::KEYS)? { a.frame_border_color = Edge::from_key(&v).unwrap_or_default(); set.push("frame_border_color") }
        if let Some(v) = f.choice("rice", &Rice::KEYS)? { a.rice = Rice::from_key(&v).unwrap_or_default(); set.push("rice") }
        f.accept(&["opacity", "blur_hint"]);
        b.config.explicit.extend(set.into_iter().map(|k| format!("appearance.{k}")));
        f.finish(&mut b.warnings);
        Ok(())
    })?)?;

    let b = shared.clone();
    api.set("sidebar", lua.create_function(move |lua, value: Value| {
        let mut f = options(lua, "sidebar", value)?;
        let mut b = b.borrow_mut();
        let mut set = Vec::new();
        let s = &mut b.config.sidebar;
        if let Some(v) = f.choice("start_state", &["collapsed", "expanded"])? { s.start_expanded = v == "expanded"; set.push("start_expanded") }
        for (key, slot) in [
            ("show_launcher", &mut s.show_launcher),
            ("show_workspaces", &mut s.show_workspaces),
            ("show_label", &mut s.show_label),
            ("show_clock", &mut s.show_clock),
            ("show_files", &mut s.show_files),
            ("show_themes", &mut s.show_themes),
            ("show_config", &mut s.show_config),
            ("show_visualizer", &mut s.show_visualizer),
            ("icon_backgrounds", &mut s.icon_backgrounds),
            ("pills", &mut s.pills),
        ] {
            if let Some(v) = f.boolean(key)? { *slot = v; set.push(key) }
        }
        if let Some(v) = f.number("scale", crate::settings::RAIL_SCALE.0, crate::settings::RAIL_SCALE.1)? { s.scale = v; set.push("scale") }
        if let Some(v) = f.number("icon_scale", crate::settings::ICON_SCALE.0, crate::settings::ICON_SCALE.1)? { s.icon_scale = v; set.push("icon_scale") }
        if let Some(v) = f.number("transparency", 0.0, crate::settings::TRANSPARENCY_MAX)? { s.transparency = Some(v); set.push("transparency") }
        if let Some(v) = f.number("blur", 0.0, crate::settings::BLUR_MAX)? { s.blur = Some(v); set.push("blur") }
        f.accept(&["drawer_fillet", "show_system", "show_extensions"]);
        b.config.explicit.extend(set.into_iter().map(|k| format!("sidebar.{k}")));
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
        let mut set = Vec::new();
        let t = &mut b.config.tiling;
        if let Some(v) = layout_kind(&mut f, "layout")? { t.layout = v }
        if let Some(v) = f.uint("gaps_in", 100)? { t.gaps_in = v; set.push("gaps_in") }
        if let Some(v) = f.uint("gaps_out", 100)? { t.gaps_out = v; set.push("gaps_out") }
        if let Some(v) = f.number("column_width", 0.2, 1.0)? { t.column_width = v }
        if let Some(v) = f.uint("max_tiles", 64)? { t.max_tiles = v.max(1); set.push("max_tiles") }
        if let Some(v) = f.number("peek", 0.0, tileide_core::token::PEEK_MAX)? { t.peek = v; set.push("peek") }
        b.config.explicit.extend(set.into_iter().map(|k| format!("tiling.{k}")));
        f.finish(&mut b.warnings);
        Ok(())
    })?)?;

    let b = shared.clone();
    api.set("alerts", lua.create_function(move |lua, value: Value| {
        let mut f = options(lua, "alerts", value)?;
        let mut b = b.borrow_mut();
        if let Some(v) = f.boolean("missing_tools")? {
            b.config.alerts.missing_tools = v;
            b.config.explicit.insert("alerts.missing_tools".into());
        }
        if let Some(v) = f.boolean("tile_limit")? {
            b.config.alerts.tile_limit = v;
            b.config.explicit.insert("alerts.tile_limit".into());
        }
        f.finish(&mut b.warnings);
        Ok(())
    })?)?;

    let b = shared.clone();
    api.set("editor", lua.create_function(move |lua, value: Value| {
        let mut f = options(lua, "editor", value)?;
        let mut b = b.borrow_mut();
        let mut set = Vec::new();
        let e = &mut b.config.editor;
        for (key, slot) in [("auto_close", &mut e.auto_close), ("match_brackets", &mut e.match_brackets), ("format_on_save", &mut e.format_on_save), ("inlay_hints", &mut e.inlay_hints)] {
            if let Some(v) = f.boolean(key)? { *slot = v; set.push(key) }
        }
        b.config.explicit.extend(set.into_iter().map(|k| format!("editor.{k}")));
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

    let b = shared.clone();
    api.set("runner", lua.create_function(move |lua, (ext, cmd): (Value, Value)| {
        let location = caller(lua);
        let (Value::String(ext), Value::String(cmd)) = (ext, cmd) else {
            return Err(error(&location, "runner: expects an extension and a command, e.g. runner(\".py\", \"python3 -u {file}\")"));
        };
        let (ext, cmd) = (ext.to_str()?.to_lowercase(), cmd.to_str()?.to_string());
        if !ext.starts_with('.') || ext.len() < 2 {
            return Err(error(&location, &format!("runner: the extension must look like \".py\", got \"{ext}\"")));
        }
        b.borrow_mut().config.runners.insert(ext, cmd);
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
            Some("stack") => Action::Layout(Some(LayoutKind::Stack)),
            Some(other) => return Err(with(&format!("expected toggle, dwindle, columns or stack, got {other}"))),
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
        "format" => Action::Format,
        "rename" => Action::Rename,
        "references" => Action::References,
        "code_action" => Action::CodeAction,
        "outline" => Action::Outline,
        "extensions_section" => Action::ExtensionsSection,
        "settings" => Action::Settings,
        "keybinds" => Action::Keybinds,
        "font_size" => match arg {
            Some("up") => Action::FontSize(1),
            Some("down") => Action::FontSize(-1),
            other => return Err(with(&format!("expected up or down, got {}", other.unwrap_or("nothing")))),
        },
        "extension" => match arg {
            Some(id) if !id.is_empty() => Action::Extension(id.into()),
            _ => return Err(with("needs a command id, e.g. \"word-count.count\"")),
        },
        "run" => Action::Run,
        "debug" => Action::Debug,
        "debug_stop" => Action::DebugStop,
        "breakpoint" => Action::Breakpoint,
        "step_over" => Action::StepOver,
        "step_into" => Action::StepInto,
        "step_out" => Action::StepOut,
        "toolchains" => Action::Toolchains,
        "interpreter" => Action::Interpreter,
        "test" => Action::Test,
        "next_failure" => Action::NextFailure,
        "moveto" | "palette" => Action::Unavailable(action.into()),
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
