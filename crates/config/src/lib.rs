// SPDX-License-Identifier: GPL-3.0-or-later
//! TileIDE's config engine (spec §6.3). `tileide.lua` runs in a fresh, sandboxed Luau
//! state on every load, and its `tileide.*` calls fill a typed `Config`. Theme files
//! (`themes/*.lua`) and the state file (`state.lua`, Q15) are data: each returns one
//! table and gets no `tileide` API. Nothing here touches GTK.

pub mod api;
pub mod fields;
pub mod model;
pub mod sandbox;
pub mod settings;
pub mod vscode;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use mlua::{Lua, Value};
use tileide_core::extension::Mode;
use tileide_core::theme::{ANSI_KEYS, Glass, SYNTAX_KEYS, Theme};

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
        let mut code = [None; 8];
        if let Some(mut s) = f.table("syntax")? {
            for (slot, key) in code.iter_mut().zip(SYNTAX_KEYS) {
                *slot = s.colour(key)?;
            }
            s.finish(&mut glass_warnings);
        }
        let mut ansi = [None; 16];
        if let Some(mut t) = f.table("terminal")? {
            for (slot, key) in ansi.iter_mut().zip(ANSI_KEYS) {
                *slot = t.colour(key)?;
            }
            t.finish(&mut glass_warnings);
        }
        Ok(Theme { name: f.string("name")?.unwrap_or(stem), dark: mode == "dark", surface, tile, text, muted, accent, on_accent, error, strip: [surface, mid, accent], glass, ansi, code, match_syntax: false })
    })?;
    warnings.append(&mut glass_warnings);
    Ok((theme, warnings))
}

/// `theme` as a theme file that `load_theme` reads back unchanged (the theme editor's
/// Save and Export). `header` is the comment line on top.
pub fn theme_lua(theme: &Theme, header: &str) -> String {
    let hex = vscode::hex;
    let mut lua = format!("-- {header}\nreturn {{\n  name      = {},\n  mode      = \"{}\",\n", lua_string(&theme.name), if theme.dark { "dark" } else { "light" });
    for (key, c) in [("surface", theme.surface), ("tile", theme.tile), ("text", theme.text), ("muted", theme.muted), ("accent", theme.accent), ("on_accent", theme.on_accent), ("error", theme.error), ("strip_mid", theme.strip[1])] {
        lua += &format!("  {key:<9} = \"{}\",\n", hex(c));
    }
    if let Some(g) = &theme.glass {
        lua += &format!(
            "  glass = {{\n    opacity        = {},\n    bottom         = \"{}\",\n    bottom_opacity = {},\n    edge_opacity   = {},\n    focus_opacity  = {},\n    blur           = {},\n    saturate       = {},\n    min_tint       = {},\n  }},\n",
            g.opacity,
            hex(g.bottom),
            g.bottom_opacity,
            g.edge_opacity,
            g.focus_opacity,
            g.blur,
            g.saturate,
            g.min_tint
        );
    }
    for (table, keys, colours) in [("syntax", &SYNTAX_KEYS[..], &theme.code[..]), ("terminal", &ANSI_KEYS[..], &theme.ansi[..])] {
        let set: Vec<String> = keys.iter().zip(colours).filter_map(|(key, c)| c.map(|c| format!("    {key} = \"{}\",\n", hex(c)))).collect();
        if !set.is_empty() {
            lua += &format!("  {table} = {{\n{}  }},\n", set.concat());
        }
    }
    lua + "}\n"
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
    /// Folders picked with the folder search, newest first (at most `RECENT_FOLDERS`).
    pub recent_folders: Vec<String>,
    /// The background picker's choice for every workspace.
    pub background: Option<model::Background>,
    /// `tileide.background` when that choice was made: once the config's differs (it
    /// was edited since), the config wins again.
    pub background_over: Option<model::Background>,
    /// Themes section picks for one workspace, by number.
    pub workspaces: BTreeMap<u8, WorkspacePick>,
    /// Extensions section choices, by extension id.
    pub extensions: BTreeMap<String, ExtensionPick>,
    /// Settings changed in the GUI (`settings::KEYS`).
    pub settings: BTreeMap<String, settings::Value>,
}

/// An extension's switch and its per-workspace modes (absent = on demand).
#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionPick {
    pub enabled: bool,
    pub workspaces: BTreeMap<u8, Mode>,
}

impl Default for ExtensionPick {
    fn default() -> Self {
        ExtensionPick { enabled: true, workspaces: BTreeMap::new() }
    }
}

impl ExtensionPick {
    pub fn mode(&self, workspace: Option<u8>) -> Mode {
        workspace.and_then(|n| self.workspaces.get(&n).copied()).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkspacePick {
    pub theme: Option<String>,
    pub background: Option<model::Background>,
}

pub const RECENT_FOLDERS: usize = 10;

impl State {
    /// Moves `folder` to the front of the recent list.
    pub fn remember_folder(&mut self, folder: &str) {
        self.recent_folders.retain(|f| f != folder);
        self.recent_folders.insert(0, folder.to_string());
        self.recent_folders.truncate(RECENT_FOLDERS);
    }
}

/// A missing file is an empty state; a broken one is an error the caller shows and ignores.
pub fn load_state(path: &Path) -> Result<State, String> {
    if !path.exists() {
        return Ok(State::default());
    }
    data_table(path, "state", |f| {
        let mut state = State { theme: f.string("theme")?, recent_folders: f.strings("recent_folders")?.unwrap_or_default(), ..State::default() };
        state.background = f.table("background")?.map(|mut b| state_background(&mut b)).transpose()?;
        state.background_over = f.table("background_over")?.map(|mut b| state_background(&mut b)).transpose()?;
        if let Some(mut list) = f.table("workspaces")? {
            for key in list.keys() {
                let Ok(n) = key.parse::<u8>() else { continue };
                let Some(mut w) = list.table(&key)? else { continue };
                let pick = WorkspacePick { theme: w.string("theme")?, background: w.table("background")?.map(|mut b| state_background(&mut b)).transpose()? };
                state.workspaces.insert(n, pick);
            }
        }
        if let Some(mut list) = f.table("settings")? {
            for key in list.keys() {
                let value = match settings::kind(&key) {
                    Some(settings::Kind::Bool) => list.boolean(&key)?.map(settings::Value::Bool),
                    Some(settings::Kind::Number(min, max)) => list.number(&key, min, max)?.map(settings::Value::Number),
                    Some(settings::Kind::Text | settings::Kind::Choice(_)) => list.string(&key)?.map(settings::Value::Text),
                    None => None,
                };
                if let Some(v) = value.and_then(|v| settings::checked(&key, v)) {
                    state.settings.insert(key, v);
                }
            }
        }
        if let Some(mut list) = f.table("extensions")? {
            for id in list.keys() {
                let Some(mut e) = list.table(&id)? else { continue };
                let mut pick = ExtensionPick { enabled: e.boolean("enabled")?.unwrap_or(true), ..ExtensionPick::default() };
                if let Some(mut modes) = e.table("workspaces")? {
                    for key in modes.keys() {
                        let (Ok(n), Some(mode)) = (key.parse::<u8>(), modes.string(&key)?.as_deref().and_then(Mode::from_key)) else { continue };
                        pick.workspaces.insert(n, mode);
                    }
                }
                state.extensions.insert(id, pick);
            }
        }
        Ok(state)
    })
    .map(|(s, _)| s)
}

fn state_background(f: &mut Fields) -> mlua::Result<model::Background> {
    let mut bg = Config::default().background;
    api::read_background(f, &mut bg)?;
    Ok(bg)
}

fn background_lua(bg: &model::Background) -> String {
    let source = bg.source.as_deref().map_or_else(String::new, |s| format!(" source = {},", lua_string(s)));
    format!("{{ mode = {},{source} dim = {}, blur = {}, tile_opacity = {}, fit = {} }}", lua_string(&bg.mode), bg.dim, bg.blur, bg.tile_opacity, lua_string(&bg.fit))
}

pub fn save_state(path: &Path, state: &State) -> std::io::Result<()> {
    let mut text = String::from("-- Written by TileIDE (GUI choices). Settings in tileide.lua win over these.\nreturn {\n");
    if let Some(theme) = &state.theme {
        text += &format!("  theme = {},\n", lua_string(theme));
    }
    if let Some(bg) = &state.background {
        text += &format!("  background = {},\n", background_lua(bg));
    }
    if let Some(bg) = &state.background_over {
        text += &format!("  background_over = {},\n", background_lua(bg));
    }
    let picks: Vec<String> = state
        .workspaces
        .iter()
        .filter(|(_, w)| w.theme.is_some() || w.background.is_some())
        .map(|(n, w)| {
            let theme = w.theme.as_deref().map_or_else(String::new, |t| format!(" theme = {},", lua_string(t)));
            let bg = w.background.as_ref().map_or_else(String::new, |b| format!(" background = {},", background_lua(b)));
            format!("    [\"{n}\"] = {{{theme}{bg} }},\n")
        })
        .collect();
    if !picks.is_empty() {
        text += &format!("  workspaces = {{\n{}  }},\n", picks.concat());
    }
    let extensions: Vec<String> = state
        .extensions
        .iter()
        .filter(|(_, e)| **e != ExtensionPick::default())
        .map(|(id, e)| {
            let modes: Vec<String> = e.workspaces.iter().filter(|(_, m)| **m != Mode::OnDemand).map(|(n, m)| format!("[\"{n}\"] = {}", lua_string(m.key()))).collect();
            format!("    [{}] = {{ enabled = {}, workspaces = {{ {} }} }},\n", lua_string(id), e.enabled, modes.join(", "))
        })
        .collect();
    if !extensions.is_empty() {
        text += &format!("  extensions = {{\n{}  }},\n", extensions.concat());
    }
    let settings: Vec<String> = state
        .settings
        .iter()
        .map(|(key, v)| {
            let value = match v {
                settings::Value::Bool(b) => b.to_string(),
                settings::Value::Number(n) => format!("{n}"),
                settings::Value::Text(t) => lua_string(t),
            };
            format!("    [{}] = {value},\n", lua_string(key))
        })
        .collect();
    if !settings.is_empty() {
        text += &format!("  settings = {{\n{}  }},\n", settings.concat());
    }
    if !state.recent_folders.is_empty() {
        let list: Vec<String> = state.recent_folders.iter().map(|f| lua_string(f)).collect();
        text += &format!("  recent_folders = {{ {} }},\n", list.join(", "));
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
