//! Typed config model. Every field has a default, so an empty config is valid.
//! Values mirror `project_contexts/project_context.md` §6.3 and `design.md` §4.1.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub keys: Keys,
    pub appearance: Appearance,
    pub sidebar: Sidebar,
    pub background: Background,
    pub tiling: Tiling,
    pub binds: Vec<Bind>,
    pub workspaces: BTreeMap<u8, Workspace>,
    pub extensions: BTreeMap<String, String>,
    pub lsp: BTreeMap<String, String>,
    pub toolchains: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Keys {
    /// TileIDE's own modifier; SUPER is left to the window manager (§4.5).
    pub modifier: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Appearance {
    pub theme: String,
    pub opacity: f64,
    pub blur_hint: bool,
    pub window_radius: u32,
    pub gap_rail: u32,
    pub gap_outer: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sidebar {
    pub start_state: String,
    pub drawer_fillet: u32,
    pub show_launcher: bool,
    pub show_workspaces: bool,
    pub show_label: bool,
    pub show_clock: bool,
    pub show_system: bool,
    pub show_files: bool,
    pub show_extensions: bool,
    pub show_themes: bool,
    pub show_config: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Background {
    pub mode: String,
    pub source: Option<String>,
    pub dim: u32,
    pub blur: u32,
    pub tile_opacity: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tiling {
    pub layout: String,
    pub gaps_in: u32,
    pub gaps_out: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bind {
    pub mods: Vec<String>,
    pub key: String,
    pub action: String,
    pub arg: Option<BindArg>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BindArg {
    Int(i64),
    Str(String),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Workspace {
    pub name: Option<String>,
    pub layout: Option<String>,
    pub folder: Option<String>,
    pub extensions: Option<String>,
    pub background: Option<String>,
}

pub const MODIFIERS: &[&str] = &["SUPER", "ALT", "CTRL", "SHIFT"];
pub const START_STATES: &[&str] = &["collapsed", "expanded"];
pub const BACKGROUND_MODES: &[&str] = &["see-through", "desktop", "image", "gradient", "solid"];
pub const LAYOUTS: &[&str] = &["dwindle", "master"];
pub const EXTENSION_MODES: &[&str] = &["on", "off", "on-demand"];
/// Editor tiles never go below this opacity (design.md §4.1, legibility guard).
pub const MIN_TILE_OPACITY: f64 = 0.88;

impl Default for Config {
    fn default() -> Self {
        Config {
            keys: Keys { modifier: "ALT".into() },
            appearance: Appearance {
                theme: "dusk".into(),
                opacity: 0.82,
                blur_hint: true,
                window_radius: 20,
                gap_rail: 24,
                gap_outer: 30,
            },
            sidebar: Sidebar {
                start_state: "collapsed".into(),
                drawer_fillet: 20,
                show_launcher: true,
                show_workspaces: true,
                show_label: true,
                show_clock: true,
                show_system: true,
                show_files: true,
                show_extensions: true,
                show_themes: true,
                show_config: true,
            },
            background: Background {
                mode: "see-through".into(),
                source: None,
                dim: 30,
                blur: 0,
                tile_opacity: 0.92,
            },
            tiling: Tiling { layout: "dwindle".into(), gaps_in: 8, gaps_out: 12 },
            binds: Vec::new(),
            workspaces: BTreeMap::new(),
            extensions: BTreeMap::new(),
            lsp: BTreeMap::new(),
            toolchains: BTreeMap::new(),
        }
    }
}
