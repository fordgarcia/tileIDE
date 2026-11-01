// SPDX-License-Identifier: GPL-3.0-or-later
//! TileIDE's config engine (spec §6.3). `tileide.lua` runs in a fresh, sandboxed Luau
//! state on every load, and its `tileide.*` calls fill a typed `Config`. Theme files
//! (`themes/*.lua`) and the state file (`state.lua`, Q15) are data: each returns one
//! table and gets no `tileide` API. Nothing here touches GTK.

pub mod api;
pub mod fields;
pub mod model;
pub mod sandbox;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use mlua::{Lua, Value};
use tileide_core::theme::{Glass, Theme};

use fields::Fields;
pub use model::{Action, Bind, Config};
pub use sandbox::Limits;

#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    pub warnings: Vec<String>,
    pub elapsed: Duration,
}

/// Runs the config at `path`. On error the caller keeps its last good `Config`.
pub fn load(path: &Path, limits: Limits) -> Result<Loaded, String> {
    let started = Instant::now();
    let shared = Rc::new(RefCell::new(api::Builder::default()));
    run(path, limits, &shared).map_err(|e| describe(&e))?;
    let builder = Rc::try_unwrap(shared).map_err(|_| "internal: config state still referenced".to_string())?.into_inner();
    // A bind inside a loop warns once per pass; one copy is enough.
    let mut warnings = builder.warnings;
    let mut seen = std::collections::HashSet::new();
    warnings.retain(|w| seen.insert(w.clone()));
    Ok(Loaded { config: builder.config, warnings, elapsed: started.elapsed() })
}

fn run(path: &Path, limits: Limits, shared: &api::Shared) -> mlua::Result<()> {
    let lua = sandbox::new_state(limits)?;
    sandbox::install_require(&lua, dir_of(path))?;
    let api = api::install(&lua, shared)?;
    lua.globals().set("tileide", api)?;
    sandbox::seal(&lua)?;
    sandbox::exec_file(&lua, path, &file_name(path))?;
    Ok(())
}

fn dir_of(path: &Path) -> &Path {
    path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// Runs a data file (no API, no `require`) and reads the table it returns.
fn data_table<T>(path: &Path, section: &str, read: impl FnOnce(&mut Fields) -> mlua::Result<T>) -> Result<(T, Vec<String>), String> {
    let run = || -> mlua::Result<(T, Vec<String>)> {
        let lua: Lua = sandbox::new_state(Limits::default())?;
        sandbox::seal(&lua)?;
        let name = file_name(path);
        let Value::Table(table) = sandbox::exec_file(&lua, path, &name)? else {
            return Err(mlua::Error::runtime(format!("{name}: must return a table")));
        };
        let mut f = Fields::new(section, &name, table)?;
        let value = read(&mut f)?;
        let mut warnings = Vec::new();
        f.finish(&mut warnings);
        Ok((value, warnings))
    };
    run().map_err(|e| describe(&e))
}

/// `themes/<name>.lua`: colours only (`themes/theme-design.md`). Layout keys are
/// refused with a warning, since layout belongs to rices.
pub fn load_theme(path: &Path) -> Result<(Theme, Vec<String>), String> {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut glass_warnings = Vec::new();
    let (theme, mut warnings) = data_table(path, "theme", |f| {
        let need = |f: &mut Fields, key: &str| f.colour(key)?.ok_or_else(|| mlua::Error::runtime(format!("{}: theme.{key}: missing", file_name(path))));
        let mode = f.choice("mode", &["dark", "light"])?.ok_or_else(|| mlua::Error::runtime(format!("{}: theme.mode: missing (dark or light)", file_name(path))))?;
        let surface = need(f, "surface")?;
        let tile = need(f, "tile")?;
        let text = need(f, "text")?;
        let muted = need(f, "muted")?;
        let accent = need(f, "accent")?;
        let on_accent = f.colour("on_accent")?.unwrap_or(surface);
        let error = f.colour("error")?.unwrap_or(Theme::builtin().error);
        let mid = f.colour("strip_mid")?.unwrap_or(muted);
        let glass = f.table("glass")?.map(|g| read_glass(g, &mut glass_warnings)).transpose()?;
        Ok(Theme { name: f.string("name")?.unwrap_or(stem), dark: mode == "dark", surface, tile, text, muted, accent, on_accent, error, strip: [surface, mid, accent], glass })
    })?;
    warnings.append(&mut glass_warnings);
    Ok((theme, warnings))
}

/// `glass = { … }` in a theme file; every key is optional (defaults: theme-design.md §4).
fn read_glass(mut f: Fields, warnings: &mut Vec<String>) -> mlua::Result<Glass> {
    let d = Glass::default();
    let glass = Glass {
        opacity: f.number("opacity", 0.0, 1.0)?.unwrap_or(d.opacity),
        bottom: f.colour("bottom")?.unwrap_or(d.bottom),
        bottom_opacity: f.number("bottom_opacity", 0.0, 1.0)?.unwrap_or(d.bottom_opacity),
        edge_opacity: f.number("edge_opacity", 0.0, 1.0)?.unwrap_or(d.edge_opacity),
        focus_opacity: f.number("focus_opacity", 0.0, 1.0)?.unwrap_or(d.focus_opacity),
        blur: f.number("blur", 0.0, 100.0)?.unwrap_or(d.blur),
        saturate: f.number("saturate", 0.0, 3.0)?.unwrap_or(d.saturate),
        min_tint: f.number("min_tint", 0.0, 1.0)?.unwrap_or(d.min_tint),
    };
    f.finish(warnings);
    Ok(glass)
}

/// Theme files by name (file stem). Earlier directories win, so a user's
/// `~/.config/tileide/themes/x.lua` replaces a packaged one with the same name.
pub fn find_themes(dirs: &[PathBuf]) -> BTreeMap<String, PathBuf> {
    let mut found = BTreeMap::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "lua")
                && let Some(stem) = path.file_stem()
            {
                found.entry(stem.to_string_lossy().into_owned()).or_insert(path);
            }
        }
    }
    found
}

/// What TileIDE remembers from GUI choices (Q15). Written only by TileIDE; the
/// config wins over it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub theme: Option<String>,
}

/// A missing file is an empty state; a broken one is an error the caller shows and ignores.
pub fn load_state(path: &Path) -> Result<State, String> {
    if !path.exists() {
        return Ok(State::default());
    }
    data_table(path, "state", |f| Ok(State { theme: f.string("theme")? })).map(|(s, _)| s)
}

pub fn save_state(path: &Path, state: &State) -> std::io::Result<()> {
    let mut text = String::from("-- Written by TileIDE (GUI choices). Settings in tileide.lua win over these.\nreturn {\n");
    if let Some(theme) = &state.theme {
        text += &format!("  theme = {},\n", lua_string(theme));
    }
    text += "}\n";
    // Written next to the target and renamed, so a crash never leaves half a file.
    let tmp = path.with_extension("lua.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, path)
}

fn lua_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            '\n' => out += "\\n",
            c if c.is_control() => out += &format!("\\{}", c as u32),
            c => out.push(c),
        }
    }
    out + "\""
}

/// The innermost, human-readable message (already `file:line:` prefixed where Lua knows it).
fn describe(error: &mlua::Error) -> String {
    match error {
        mlua::Error::CallbackError { cause, .. } => describe(cause),
        mlua::Error::RuntimeError(message) => message.clone(),
        mlua::Error::SyntaxError { message, .. } => message.clone(),
        mlua::Error::MemoryError(_) => "config exceeded the memory limit".into(),
        other => other.to_string(),
    }
}
