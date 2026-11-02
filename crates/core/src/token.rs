// SPDX-License-Identifier: GPL-3.0-or-later
//! Layout tokens in logical pixels (`themes/theme-design.md` §3). Colours live in
//! `theme`; these are rice settings and become configurable in v0.2.

pub const RAIL_WIDTH: f64 = 48.0;
pub const DRAWER_WIDTH: f64 = 220.0;
pub const DRAWER_MIN_WIDTH: f64 = 200.0;
pub const DRAWER_MAX_WIDTH: f64 = 600.0;
/// Frame border on the top, right and bottom screen edges.
pub const FRAME_BORDER: f64 = 12.0;
pub const FRAME_OPACITY: f64 = 0.95;
/// Tile corner radius (`window_radius` in the config).
pub const TILE_RADIUS: f64 = 14.0;
/// `gaps_in`: between tiles.
pub const GAP_TILE_IN: f64 = 10.0;
/// `gaps_out`: the frame's inner edge to the tiles.
pub const GAP_TILE_OUT: f64 = 16.0;
pub const TILE_OPACITY: f64 = 0.92;
/// `appearance.tile_border` default (px).
pub const TILE_BORDER: f64 = 2.0;
/// Upper bound of both border widths (px).
pub const BORDER_MAX: f64 = 8.0;
pub const UI_FONT: &str = "Space Grotesk";
/// The card behind the empty-workspace hint, so it stays legible on any background.
pub const HINT_OPACITY: f64 = 0.85;
/// Columns layout: a column is never narrower than this (or the code region, if smaller).
pub const MIN_COLUMN_WIDTH: f64 = 480.0;
/// A tile's content is laid out at least this big (w, h); a smaller tile clips it.
pub const MIN_TILE: (f64, f64) = (160.0, 120.0);
/// `tiling.peek` default (px) and upper bound.
pub const PEEK: f64 = 40.0;
pub const PEEK_MAX: f64 = 160.0;
/// `tiling.max_tiles`: tiles per workspace.
pub const MAX_TILES: u32 = 16;

use std::sync::RwLock;

static UI: RwLock<Option<(String, f64)>> = RwLock::new(None);

/// The interface font and text scale chosen in Settings (`appearance.ui_font`, `ui_scale`).
pub fn set_ui_font(family: &str, scale: f64) {
    if let Ok(mut ui) = UI.write() {
        *ui = Some((family.to_string(), scale));
    }
}

static RAIL: RwLock<(f64, f64)> = RwLock::new((1.0, 1.0));

/// The rail's size and its icons' size chosen in Settings (`sidebar.scale`, `icon_scale`).
pub fn set_rail(scale: f64, icons: f64) {
    if let Ok(mut r) = RAIL.write() {
        *r = (scale, icons);
    }
}

/// Rail size factor: the rail is drawn in `RAIL_WIDTH` units scaled by this.
pub fn rail_scale() -> f64 {
    RAIL.read().map_or(1.0, |r| r.0)
}

/// Icon size factor within the rail.
pub fn icon_scale() -> f64 {
    RAIL.read().map_or(1.0, |r| r.1)
}

/// The rail's width on screen.
pub fn rail_width() -> f64 {
    RAIL_WIDTH * rail_scale()
}

/// The overall arrangement (`appearance.rice`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rice {
    /// The rail on the left, Files as a drawer, dwindle or scrolling columns.
    #[default]
    Default,
    /// A top bar, Files on the left, workspaces on the right, tiles stacked vertically
    /// (`design/001/wikipedia-rice-design.png`).
    Wikipedia,
}

impl Rice {
    pub const KEYS: [&'static str; 2] = ["default", "wikipedia"];

    pub fn key(self) -> &'static str {
        Rice::KEYS[self as usize]
    }

    pub fn from_key(key: &str) -> Option<Rice> {
        [Rice::Default, Rice::Wikipedia].into_iter().find(|r| r.key() == key)
    }
}

/// Wikipedia rice: the top bar's height and the workspace list's width.
pub const TOP_BAR: f64 = 60.0;
pub const WORKSPACE_LIST_WIDTH: f64 = 224.0;

static RICE: RwLock<(Rice, bool)> = RwLock::new((Rice::Default, true));

/// The rice in use, and (Wikipedia) whether the workspace list shows.
pub fn set_rice(rice: Rice, workspace_list: bool) {
    if let Ok(mut r) = RICE.write() {
        *r = (rice, workspace_list);
    }
}

pub fn rice() -> Rice {
    RICE.read().map_or(Rice::Default, |r| r.0)
}

/// What the frame takes on the left before the drawer: the rail, or (Wikipedia) a border.
pub fn left_inset() -> f64 {
    match rice() {
        Rice::Default => rail_width(),
        Rice::Wikipedia => FRAME_BORDER,
    }
}

/// The frame's top: a border, or (Wikipedia) the top bar.
pub fn top_inset() -> f64 {
    match rice() {
        Rice::Default => FRAME_BORDER,
        Rice::Wikipedia => TOP_BAR,
    }
}

/// The frame's right side: a border, plus (Wikipedia) the workspace list when shown.
pub fn right_inset() -> f64 {
    match RICE.read().map_or((Rice::Default, false), |r| *r) {
        (Rice::Wikipedia, true) => FRAME_BORDER + WORKSPACE_LIST_WIDTH,
        _ => FRAME_BORDER,
    }
}

/// The interface font family (`UI_FONT` until Settings changes it).
pub fn ui_font() -> String {
    UI.read().ok().and_then(|ui| ui.as_ref().map(|u| u.0.clone())).unwrap_or_else(|| UI_FONT.to_string())
}

/// Interface text scale, 1.0 = as designed.
pub fn ui_scale() -> f64 {
    UI.read().ok().and_then(|ui| ui.as_ref().map(|u| u.1)).unwrap_or(1.0)
}

/// The configurable part of the layout (`tileide.tiling`, `appearance.window_radius`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub tile_radius: f64,
    pub gaps_in: f64,
    pub gaps_out: f64,
    /// Tile outline width (0 = none) and colour; a focused tile's is always the accent.
    pub tile_border: f64,
    pub tile_edge: crate::theme::Edge,
    /// The line around the frame's hole (0 = none) and its colour.
    pub frame_border: f64,
    pub frame_edge: crate::theme::Edge,
}

impl Default for Geometry {
    fn default() -> Self {
        Geometry {
            tile_radius: TILE_RADIUS,
            gaps_in: GAP_TILE_IN,
            gaps_out: GAP_TILE_OUT,
            tile_border: TILE_BORDER,
            tile_edge: crate::theme::Edge::Subtle,
            frame_border: 0.0,
            frame_edge: crate::theme::Edge::Accent,
        }
    }
}

impl Geometry {
    /// The frame's inner corners, concentric with the tile corners.
    pub fn frame_radius(&self) -> f64 {
        self.tile_radius + self.gaps_out
    }
}
