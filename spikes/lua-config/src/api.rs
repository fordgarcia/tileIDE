//! The `tileide.*` API. Each call merges its options into the shared `Builder`,
//! so a later file (e.g. a rice pulled in with `require`) overrides earlier values.

use std::cell::RefCell;
use std::rc::Rc;

use mlua::{Lua, Result, Table, Value, Variadic};

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

fn name_arg(lua: &Lua, section: &str, value: Value) -> Result<String> {
    match value {
        Value::String(s) => Ok(s.to_str()?.to_string()),
        other => Err(error(&caller(lua), &format!("{section}: expected a name string, got {}", other.type_name()))),
    }
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
            b.config.keys.modifier = m;
        }
        f.finish(&mut b.warnings);
        Ok(())
    })?)?;

    let b = shared.clone();
    api.set("appearance", lua.create_function(move |lua, value: Value| {
        let mut f = options(lua, "appearance", value)?;
        let mut b = b.borrow_mut();
        let a = &mut b.config.appearance;
        if let Some(v) = f.string("theme")? { a.theme = v }
        if let Some(v) = f.number("opacity", 0.0, 1.0)? { a.opacity = v }
        if let Some(v) = f.boolean("blur_hint")? { a.blur_hint = v }
        if let Some(v) = f.uint("window_radius", 200)? { a.window_radius = v }
        if let Some(v) = f.uint("gap_rail", 500)? { a.gap_rail = v }
        if let Some(v) = f.uint("gap_outer", 500)? { a.gap_outer = v }
        f.finish(&mut b.warnings);
        Ok(())
    })?)?;

    let b = shared.clone();
    api.set("sidebar", lua.create_function(move |lua, value: Value| {
        let mut f = options(lua, "sidebar", value)?;
        let mut b = b.borrow_mut();
        let s = &mut b.config.sidebar;
        if let Some(v) = f.choice("start_state", START_STATES)? { s.start_state = v }
        if let Some(v) = f.uint("drawer_fillet", 200)? { s.drawer_fillet = v }
        for (key, slot) in [
            ("show_launcher", &mut s.show_launcher),
            ("show_workspaces", &mut s.show_workspaces),
            ("show_label", &mut s.show_label),
            ("show_clock", &mut s.show_clock),
            ("show_system", &mut s.show_system),
            ("show_files", &mut s.show_files),
            ("show_extensions", &mut s.show_extensions),
            ("show_themes", &mut s.show_themes),
            ("show_config", &mut s.show_config),
        ] {
            if let Some(v) = f.boolean(key)? { *slot = v }
        }
        f.finish(&mut b.warnings);
        Ok(())
    })?)?;

    let b = shared.clone();
    api.set("background", lua.create_function(move |lua, value: Value| {
        let mut f = options(lua, "background", value)?;
        let mut b = b.borrow_mut();
        let bg = &mut b.config.background;
        if let Some(v) = f.choice("mode", BACKGROUND_MODES)? { bg.mode = v }
        if let Some(v) = f.string("source")? { bg.source = Some(v) }
        if let Some(v) = f.uint("dim", 100)? { bg.dim = v }
        if let Some(v) = f.uint("blur", 200)? { bg.blur = v }
        if let Some(v) = f.number("tile_opacity", MIN_TILE_OPACITY, 1.0)? { bg.tile_opacity = v }
        f.finish(&mut b.warnings);
        Ok(())
    })?)?;

    let b = shared.clone();
    api.set("tiling", lua.create_function(move |lua, value: Value| {
        let mut f = options(lua, "tiling", value)?;
        let mut b = b.borrow_mut();
        let t = &mut b.config.tiling;
        if let Some(v) = f.choice("layout", LAYOUTS)? { t.layout = v }
        if let Some(v) = f.uint("gaps_in", 200)? { t.gaps_in = v }
        if let Some(v) = f.uint("gaps_out", 200)? { t.gaps_out = v }
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
        let mods: Vec<String> = mods.split_whitespace().map(str::to_uppercase).collect();
        if let Some(bad) = mods.iter().find(|m| !MODIFIERS.contains(&m.as_str())) {
            return Err(error(&location, &format!("bind: unknown modifier \"{bad}\" (use {})", MODIFIERS.join(", "))));
        }
        let arg = match args.get(3) {
            None | Some(Value::Nil) => None,
            Some(Value::Integer(i)) => Some(BindArg::Int(*i)),
            Some(Value::Number(n)) if n.fract() == 0.0 => Some(BindArg::Int(*n as i64)),
            Some(Value::String(s)) => Some(BindArg::Str(s.to_str()?.to_string())),
            Some(other) => return Err(error(&location, &format!("bind: argument 4 must be an integer or string, got {}", other.type_name()))),
        };
        b.borrow_mut().config.binds.push(Bind { mods, key, action, arg });
        Ok(())
    })?)?;

    let b = shared.clone();
    api.set("workspace", lua.create_function(move |lua, (id, value): (Value, Value)| {
        let id = match id {
            Value::Integer(i) if (1..=9).contains(&i) => i as u8,
            Value::Number(n) if n.fract() == 0.0 && (1.0..=9.0).contains(&n) => n as u8,
            _ => return Err(error(&caller(lua), "workspace: id must be an integer 1..9")),
        };
        let mut f = options(lua, &format!("workspace({id})"), value)?;
        let mut b = b.borrow_mut();
        let w = b.config.workspaces.entry(id).or_default();
        if let Some(v) = f.string("name")? { w.name = Some(v) }
        if let Some(v) = f.choice("layout", LAYOUTS)? { w.layout = Some(v) }
        if let Some(v) = f.string("folder")? { w.folder = Some(v) }
        if let Some(v) = f.string("extensions")? { w.extensions = Some(v) }
        if let Some(v) = f.string("background")? { w.background = Some(v) }
        f.finish(&mut b.warnings);
        Ok(())
    })?)?;

    // extension / lsp / toolchain all take (name, { one_key = ... }).
    for (func, key, allowed) in [
        ("extension", "mode", Some(EXTENSION_MODES)),
        ("lsp", "cmd", None),
        ("toolchain", "path", None),
    ] {
        let b = shared.clone();
        api.set(func, lua.create_function(move |lua, (name, value): (Value, Value)| {
            let name = name_arg(lua, func, name)?;
            let mut f = options(lua, &format!("{func}(\"{name}\")"), value)?;
            let v = match allowed {
                Some(allowed) => f.choice(key, allowed)?,
                None => f.string(key)?,
            };
            let Some(v) = v else {
                return Err(error(&caller(lua), &format!("{func}(\"{name}\"): missing \"{key}\"")));
            };
            let mut b = b.borrow_mut();
            let map = match func {
                "extension" => &mut b.config.extensions,
                "lsp" => &mut b.config.lsp,
                _ => &mut b.config.toolchains,
            };
            map.insert(name, v);
            f.finish(&mut b.warnings);
            Ok(())
        })?)?;
    }

    read_only(lua, api)
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
