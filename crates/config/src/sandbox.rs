// SPDX-License-Identifier: GPL-3.0-or-later
//! A fresh, restricted Luau state per config run (user-config tier, §6.3):
//! no io/os/package/debug, no loading code from strings or bytecode,
//! `require` confined to the config directory, plus time and memory limits.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use mlua::chunk::ChunkMode;
use mlua::{Lua, LuaOptions, Result, StdLib, Table, Value};

use crate::api::caller;
use crate::fields::error;

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub time: Duration,
    pub memory: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { time: Duration::from_millis(250), memory: 32 * 1024 * 1024 }
    }
}

/// Base-library entries that can load or run code outside our control.
const REMOVED_GLOBALS: &[&str] = &["dofile", "loadfile", "load", "loadstring", "collectgarbage", "require", "module", "newproxy", "gcinfo"];

pub fn new_state(limits: Limits) -> Result<Lua> {
    let libs = StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8;
    let lua = Lua::new_with(libs, LuaOptions::new())?;
    lua.set_memory_limit(limits.memory)?;

    let globals = lua.globals();
    for name in REMOVED_GLOBALS {
        globals.raw_set(*name, Value::Nil)?;
    }
    let string: Table = globals.get("string")?;
    string.raw_set("dump", Value::Nil)?;

    globals.set("print", lua.create_function(|lua, args: mlua::Variadic<Value>| {
        let text: Vec<String> = args.iter().map(|v| v.to_string().unwrap_or_else(|_| v.type_name().into())).collect();
        eprintln!("{}: print: {}", caller(lua), text.join("\t"));
        Ok(())
    })?)?;

    let deadline = Instant::now() + limits.time;
    guard_protected_calls(&lua, deadline)?;
    let over_time = move || -> Result<mlua::VmState> {
        if Instant::now() > deadline {
            Err(mlua::Error::runtime(format!("config exceeded the {} ms time limit", limits.time.as_millis())))
        } else {
            Ok(mlua::VmState::Continue)
        }
    };
    // Luau checks the interrupt inside string matching too, so no single call escapes it.
    lua.set_interrupt(move |_| over_time());

    Ok(lua)
}

/// Without this, `while true do pcall(function() while true do end end) end` runs
/// forever: the hook always fires inside the inner loop and `pcall` swallows the
/// time-limit error. Past the deadline, protected calls re-raise instead.
const GUARD: &str = r#"
return function(over)
  local raw_pcall, raw_xpcall, error = pcall, xpcall, error
  local function check(ok, ...)
    if not ok and over() then error((...), 0) end
    return ok, ...
  end
  pcall = function(f, ...) return check(raw_pcall(f, ...)) end
  xpcall = function(f, handler, ...) return check(raw_xpcall(f, handler, ...)) end
end
"#;

fn guard_protected_calls(lua: &Lua, deadline: Instant) -> Result<()> {
    let install: mlua::Function = lua.load(GUARD).set_name("=tileide-sandbox").eval()?;
    install.call::<()>(lua.create_function(move |_, ()| Ok(Instant::now() > deadline))?)
}

/// Freezes the environment after the API is installed (Luau's built-in sandbox mode).
pub fn seal(lua: &Lua) -> Result<()> {
    lua.sandbox(true)
}

/// Loads a file as text (never bytecode). `name` is relative to the config dir:
/// Lua truncates long chunk names, and `rices/nord.lua:2:` is what users need.
pub fn exec_file(lua: &Lua, path: &Path, name: &str) -> Result<Value> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| mlua::Error::runtime(format!("{}: cannot read: {e}", path.display())))?;
    lua.load(source)
        .set_name(format!("@{name}"))
        .set_mode(ChunkMode::Text)
        .call(())
}

/// Module name → its result, or `None` while it is still loading (cycle detection).
type Modules = HashMap<String, Option<Value>>;

/// `require("rices.nord")` → `<config dir>/rices/nord.lua`; nothing outside the dir.
pub fn install_require(lua: &Lua, config_dir: &Path) -> Result<()> {
    let root = config_dir.to_path_buf();
    let canonical_root = root
        .canonicalize()
        .map_err(|e| mlua::Error::runtime(format!("{}: {e}", root.display())))?;
    let loaded: Rc<RefCell<Modules>> = Rc::default();

    let require = lua.create_function(move |lua, name: Value| {
        let location = caller(lua);
        let Value::String(name) = name else {
            return Err(error(&location, "require: expected a module name string"));
        };
        let name = name.to_str()?.to_string();
        let path = module_path(&root, &name).ok_or_else(|| {
            error(&location, &format!("require: \"{name}\" is not a module name inside the config directory"))
        })?;
        let canonical = path
            .canonicalize()
            .map_err(|_| error(&location, &format!("require: module \"{name}\" not found ({})", path.display())))?;
        if !canonical.starts_with(&canonical_root) {
            return Err(error(&location, &format!("require: \"{name}\" resolves outside the config directory")));
        }

        if let Some(state) = loaded.borrow().get(&name) {
            return match state {
                Some(value) => Ok(value.clone()),
                None => Err(error(&location, &format!("require: \"{name}\" requires itself (cycle)"))),
            };
        }
        loaded.borrow_mut().insert(name.clone(), None);
        let display = format!("{}.lua", name.replace('.', "/"));
        let value = match exec_file(lua, &path, &display)? {
            Value::Nil => Value::Boolean(true),
            v => v,
        };
        loaded.borrow_mut().insert(name, Some(value.clone()));
        Ok(value)
    })?;
    lua.globals().set("require", require)?;
    Ok(())
}

/// Accepts only dotted identifiers (`rices.nord`), so `..`, `/` and absolute paths are impossible.
fn module_path(root: &Path, name: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = name.split('.').collect();
    let valid = |p: &&str| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !parts.iter().all(valid) {
        return None;
    }
    let mut path = root.to_path_buf();
    path.extend(&parts);
    path.set_extension("lua");
    Some(path)
}
