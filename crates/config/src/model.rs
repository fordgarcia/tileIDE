// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed config model. Every field has a default, so an empty config is valid.
//! Only what v0.2 applies is modelled; keys for later versions are accepted and dropped.

use std::collections::{BTreeMap, BTreeSet};

use tileide_core::layout::LayoutKind;
use tileide_core::motion::Curve;
use tileide_core::theme::Edge;
use tileide_core::token::Rice;
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
    pub alerts: Alerts,
    pub editor: Editor,
    /// The user's binds, in order; they replace defaults with the same keys.
    pub binds: Vec<Bind>,
    pub workspaces: BTreeMap<u8, Workspace>,
    /// `tileide.lsp(name, { … })`, by server name ("rust", "c", "python", "typescript", "java").
    pub lsp: BTreeMap<String, Lsp>,
    /// Setting keys (`settings::KEYS`) that `tileide.lua` set: Settings shows them locked
    /// and `state.lua` can't override them.
    pub explicit: BTreeSet<String>,
    /// `tileide.runner(".ext", "command {file}")`: Alt+R commands by extension.
    pub runners: BTreeMap<String, String>,
    /// `tileide.launch { name, program, args, env, cwd }`: launch configs for Alt+R and F5.
    pub launches: Vec<tileide_core::launch::Launch>,
}

/// Which messages pop up on the banner.
#[derive(Debug, Clone, PartialEq)]
pub struct Alerts {
    /// A tool, language server, debugger, Jupyter or Python module is missing. Off, the
    /// message still shows where the action ran (run card, debug tile, notebook cell).
    pub missing_tools: bool,
    /// A new tile was refused: the workspace holds `tiling.max_tiles` already.
    pub tile_limit: bool,
}

/// Typing aids in editors and notebook cells.
#[derive(Debug, Clone, PartialEq)]
pub struct Editor {
    /// Typing `(`, `[`, `{` or a quote adds its closing partner; typing that partner
    /// steps over it.
    pub auto_close: bool,
    /// Highlights the bracket matching the one at the cursor.
    pub match_brackets: bool,
    /// Saving organizes imports and formats the file first (when its language server can).
    pub format_on_save: bool,
    /// Inferred types and parameter names at the end of each line (servers that offer
    /// them: basedpyright, clangd, rust-analyzer, TypeScript).
    pub inlay_hints: bool,
    /// Code colours made from the theme's colours instead of the terminal's hues.
    pub theme_syntax: bool,
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
    pub motion: Curve,
    /// Divides every animation's duration.
    pub motion_speed: f64,
    /// Font of TileIDE's own interface (drawer, cards, rail).
    pub ui_font: String,
    /// Interface text size, 1.0 = as designed.
    pub ui_scale: f64,
    /// Editors and terminals.
    pub code_font: String,
    /// Points.
    pub code_size: f64,
    /// Editors' and terminals' see-through (percent) and backdrop blur (px); `None` =
    /// the theme's look.
    pub tile_transparency: Option<f64>,
    pub tile_blur: Option<f64>,
    /// Tile outline (px, 0 = none) and its colour; the focused tile's is the accent.
    pub tile_border: f64,
    pub tile_border_color: Edge,
    /// The line around the frame's hole, where the tiles sit (px, 0 = none).
    pub frame_border: f64,
    pub frame_border_color: Edge,
    /// The overall arrangement: the rail (default) or the Wikipedia layout.
    pub rice: Rice,
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
    /// The audio visualizer (only while something plays).
    pub show_visualizer: bool,
    /// Size of the whole rail (its width and everything on it), 1.0 = as designed.
    pub scale: f64,
    /// Size of the rail's icons and dots within it.
    pub icon_scale: f64,
    /// The discs and squares behind the rail's icons.
    pub icon_backgrounds: bool,
    /// The two rounded pills the rail's buttons sit on.
    pub pills: bool,
    /// The frame's (rail, border, drawer) see-through (percent) and backdrop blur (px);
    /// `None` = as designed.
    pub transparency: Option<f64>,
    pub blur: Option<f64>,
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
    /// Most tiles one workspace may hold; new-tile commands beyond it show a notice.
    pub max_tiles: u32,
    /// Columns layout: px of the neighbouring columns kept in sight (0 = none).
    pub peek: f64,
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
    /// Language server: format the file (Ruff for Python).
    Format,
    /// Language server: rename the symbol at the cursor everywhere.
    Rename,
    /// Language server: list every use of the symbol at the cursor.
    References,
    /// Language server: quick fixes for the cursor's line.
    CodeAction,
    /// Language server: the file's names (the notebook's, across cells), to go to one.
    Outline,
    /// The drawer's Extensions section (installed extensions, weight meter).
    ExtensionsSection,
    /// Runs an extension command by id (e.g. "word-count.count").
    Extension(String),
    /// The Settings card.
    Settings,
    /// The keybinds card.
    Keybinds,
    /// Code font size of every tile: +1 or -1 point.
    FontSize(i8),
    /// Runs the focused file (or the Cargo project) and shows its output.
    Run,
    /// Debugging: start (or continue), stop, breakpoint at the cursor, steps.
    Debug,
    DebugStop,
    Breakpoint,
    StepOver,
    StepInto,
    StepOut,
    /// Settings on its Toolchains page.
    Toolchains,
    /// The Python a file's project uses, or a notebook's kernel (v0.5 phase 7).
    Interpreter,
    /// pytest: the test at the cursor, else the file, else every test (v0.5 phase 9).
    Test,
    /// Picks the launch config Alt+R and F5 use in this workspace (v0.6 phase 3).
    Launch,
    /// The next failed test's failure.
    NextFailure,
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
            appearance: Appearance {
                theme: None,
                window_radius: 14,
                animations: true,
                motion: Curve::Fluid,
                motion_speed: 1.0,
                ui_font: tileide_core::token::UI_FONT.into(),
                ui_scale: 1.0,
                code_font: "monospace".into(),
                code_size: 11.0,
                tile_transparency: None,
                tile_blur: None,
                tile_border: tileide_core::token::TILE_BORDER,
                tile_border_color: Edge::Subtle,
                frame_border: 0.0,
                frame_border_color: Edge::Accent,
                rice: Rice::Default,
            },
            sidebar: Sidebar {
                start_expanded: false,
                show_launcher: true,
                show_workspaces: true,
                show_label: true,
                show_clock: true,
                show_files: true,
                show_themes: true,
                show_config: true,
                show_visualizer: true,
                scale: 1.0,
                icon_scale: 1.0,
                icon_backgrounds: true,
                pills: true,
                transparency: None,
                blur: None,
            },
            background: Background { mode: "see-through".into(), source: None, dim: 0, blur: 0, tile_opacity: 0.92, fit: "cover".into() },
            background_set: false,
            tiling: Tiling { layout: LayoutKind::Dwindle, gaps_in: 10, gaps_out: 16, column_width: 0.5, max_tiles: tileide_core::token::MAX_TILES, peek: tileide_core::token::PEEK },
            alerts: Alerts { missing_tools: true, tile_limit: true },
            editor: Editor { auto_close: true, match_brackets: false, format_on_save: false, inlay_hints: false, theme_syntax: true },
            binds: Vec::new(),
            workspaces: BTreeMap::new(),
            lsp: BTreeMap::new(),
            explicit: BTreeSet::new(),
            runners: BTreeMap::new(),
            launches: Vec::new(),
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
    // Arrows, so Alt+K stays free for the keybinds card (user choice, 2026-11-01).
    for (key, d) in [("Left", Left), ("Down", Down), ("Up", Up), ("Right", Right)] {
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
        bind(&[m, "SHIFT"], "X", Action::ExtensionsSection),
        bind(&[m], "K", Action::Keybinds),
        bind(&[m], "comma", Action::Settings),
        bind(&[m], "equal", Action::FontSize(1)),
        bind(&[m], "minus", Action::FontSize(-1)),
        bind(&[m], "R", Action::Run),
        bind(&[m], "P", Action::Interpreter),
        bind(&[m], "L", Action::Launch),
        bind(&[m, "SHIFT"], "R", Action::Test),
        bind(&[], "F8", Action::NextFailure),
        bind(&[], "F5", Action::Debug),
        bind(&["SHIFT"], "F5", Action::DebugStop),
        bind(&[], "F9", Action::Breakpoint),
        bind(&[], "F10", Action::StepOver),
        bind(&[], "F11", Action::StepInto),
        bind(&["SHIFT"], "F11", Action::StepOut),
        bind(&[m], "I", Action::Hover),
        bind(&[], "F12", Action::Definition),
        bind(&["CTRL"], "space", Action::Complete),
        // Many desktops take Alt+Tab for window switching; Tab in the editor does the same.
        bind(&[m], "Tab", Action::Complete),
        bind(&["CTRL", "SHIFT"], "I", Action::Format),
        bind(&[], "F2", Action::Rename),
        bind(&["SHIFT"], "F12", Action::References),
        bind(&["CTRL"], "period", Action::CodeAction),
        bind(&["CTRL", "SHIFT"], "O", Action::Outline),
        bind(&[m], "Escape", Action::Quit),
        bind(&["CTRL"], "S", Action::Save),
    ]);
    binds
}
