// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed config model. Every field has a default, so an empty config is valid.
//! Only what v0.2 applies is modelled; keys for later versions are accepted and dropped.

use std::collections::BTreeMap;

use tileide_core::layout::LayoutKind;
use tileide_core::tiles::{Axis, Direction};

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// TileIDE's own modifier; SUPER is left to the window manager (§4.5).
    pub modifier: String,
    pub appearance: Appearance,
    pub sidebar: Sidebar,
    pub background: Background,
    /// `tileide.background` was called: the config then wins over the picker's saved choice.
    pub background_set: bool,
    pub tiling: Tiling,
    /// The user's binds, in order; they replace defaults with the same keys.
    pub binds: Vec<Bind>,
    pub workspaces: BTreeMap<u8, Workspace>,
    /// `tileide.lsp(name, { … })`, by server name ("rust", "c", "python", "typescript", "java").
    pub lsp: BTreeMap<String, Lsp>,
}

/// Overrides for one built-in language server.
#[derive(Debug, Clone, PartialEq)]
pub struct Lsp {
    /// Program and arguments; `None` = the built-in candidates.
    pub cmd: Option<Vec<String>>,
    pub enabled: bool,
}

impl Default for Lsp {
    fn default() -> Self {
        Lsp { cmd: None, enabled: true }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Appearance {
    /// `None` = not set by the config, so `state.lua` (the tray's pick) may decide.
    pub theme: Option<String>,
    pub window_radius: u32,
    /// Smooth motion for tiles, workspace switches and overlays (the drawer always moves).
    pub animations: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sidebar {
    pub start_expanded: bool,
    pub show_launcher: bool,
    pub show_workspaces: bool,
    pub show_label: bool,
    pub show_clock: bool,
    pub show_files: bool,
    pub show_themes: bool,
    pub show_config: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Background {
    /// see-through | image | gradient | solid
    pub mode: String,
    /// image: a path; solid: `RRGGBB`; gradient: `RRGGBB,RRGGBB`.
    pub source: Option<String>,
    /// Percent.
    pub dim: u32,
    pub blur: u32,
    pub tile_opacity: f64,
    /// cover | contain | tile
    pub fit: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tiling {
    pub layout: LayoutKind,
    pub gaps_in: u32,
    pub gaps_out: u32,
    /// New columns' width as a fraction of the code region (columns layout).
    pub column_width: f64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Workspace {
    pub name: Option<String>,
    pub layout: Option<LayoutKind>,
    pub folder: Option<String>,
    pub theme: Option<String>,
    /// An image path; full-screen only, like the global background.
    pub background: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bind {
    /// Upper-case, sorted, deduplicated (`ALT`, `CTRL`, `SHIFT`, `SUPER`).
    pub mods: Vec<String>,
    /// A key name as GDK knows it (`B`, `1`, `Return`, `Escape`, `F5`).
    pub key: String,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Workspace(u8),
    ConfigWorkspace,
    Sidebar,
    /// `None` toggles between dwindle and columns.
    Layout(Option<LayoutKind>),
    /// The axis the next new tile splits along (in columns: vertical stacks into the column).
    Split(Axis),
    Focus(Direction),
    Move(Direction),
    Resize(Direction),
    Close,
    Terminal,
    /// Quick open into a new tile (an empty query offers a blank file).
    Editor,
    /// Quick open into the focused tile.
    Search,
    Wrap,
    Save,
    Quit,
    Overview,
    Themes,
    /// Folder search: sets the active workspace's folder.
    Folder,
    /// The drawer's Themes section (themes, background picker, VS Code import).
    ThemesSection,
    /// Removes the active workspace (asks first if it holds work).
    WorkspaceClose,
    /// Language server: hover text and problems at the cursor.
    Hover,
    /// Language server: go to the definition under the cursor.
    Definition,
    /// Language server: completion at the cursor.
    Complete,
    /// Valid, but its feature arrives in a later version; pressing it does nothing.
    Unavailable(String),
}

pub const MODIFIERS: &[&str] = &["ALT", "CTRL", "SHIFT", "SUPER"];
pub const BACKGROUND_MODES: &[&str] = &["see-through", "image", "gradient", "solid"];
pub const FITS: &[&str] = &["cover", "contain", "tile"];
/// Editor tiles never go below this opacity (design.md §4.2, legibility guard).
pub const MIN_TILE_OPACITY: f64 = 0.88;
/// Highest workspace number (`Alt+0` is 10).
pub const MAX_WORKSPACE: u8 = 10;

impl Default for Config {
    fn default() -> Self {
        Config {
            modifier: "ALT".into(),
            appearance: Appearance { theme: None, window_radius: 14, animations: true },
            sidebar: Sidebar {
                start_expanded: false,
                show_launcher: true,
                show_workspaces: true,
                show_label: true,
                show_clock: true,
                show_files: true,
                show_themes: true,
                show_config: true,
            },
            background: Background { mode: "see-through".into(), source: None, dim: 0, blur: 0, tile_opacity: 0.92, fit: "cover".into() },
            background_set: false,
            tiling: Tiling { layout: LayoutKind::Dwindle, gaps_in: 10, gaps_out: 16, column_width: 0.5 },
            binds: Vec::new(),
            workspaces: BTreeMap::new(),
            lsp: BTreeMap::new(),
        }
    }
}

impl Config {
    /// Built-in binds for `modifier`, overridden by the user's binds with the same keys.
    pub fn effective_binds(&self) -> Vec<Bind> {
        let mut binds = default_binds(&self.modifier);
        for user in &self.binds {
            binds.retain(|b| !(b.mods == user.mods && b.key.eq_ignore_ascii_case(&user.key)));
            binds.push(user.clone());
        }
        binds
    }
}

fn bind(mods: &[&str], key: &str, action: Action) -> Bind {
    Bind { mods: normalise_mods(mods.iter().map(|m| m.to_string())), key: key.into(), action }
}

pub fn normalise_mods(mods: impl Iterator<Item = String>) -> Vec<String> {
    let mut mods: Vec<String> = mods.map(|m| if m == "CONTROL" { "CTRL".into() } else { m }).collect();
    mods.sort();
    mods.dedup();
    mods
}

/// The keys of v0.1, plus v0.2's new actions.
pub fn default_binds(m: &str) -> Vec<Bind> {
    use Direction::*;
    let mut binds = Vec::new();
    for n in 1..=MAX_WORKSPACE {
        binds.push(bind(&[m], &(n % 10).to_string(), Action::Workspace(n)));
    }
    for (key, d) in [("H", Left), ("J", Down), ("K", Up), ("L", Right)] {
        binds.push(bind(&[m], key, Action::Focus(d)));
        binds.push(bind(&[m, "SHIFT"], key, Action::Resize(d)));
        binds.push(bind(&[m, "SHIFT", "CTRL"], key, Action::Move(d)));
    }
    binds.extend([
        bind(&[m], "Return", Action::Terminal),
        bind(&[m], "E", Action::Editor),
        bind(&[m], "F", Action::Search),
        bind(&[m], "W", Action::Close),
        bind(&[m], "C", Action::ConfigWorkspace),
        bind(&[m], "B", Action::Sidebar),
        bind(&[m], "Z", Action::Wrap),
        bind(&[m], "V", Action::Layout(None)),
        bind(&[m], "S", Action::Split(Axis::Column)),
        bind(&[m], "O", Action::Overview),
        bind(&[m], "T", Action::Themes),
        bind(&[m], "D", Action::Folder),
        bind(&[m, "SHIFT"], "T", Action::ThemesSection),
        bind(&[m, "SHIFT"], "W", Action::WorkspaceClose),
        bind(&[m], "I", Action::Hover),
        bind(&[], "F12", Action::Definition),
        bind(&["CTRL"], "space", Action::Complete),
        bind(&[m], "Escape", Action::Quit),
        bind(&["CTRL"], "S", Action::Save),
    ]);
    binds
}
