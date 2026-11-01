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
pub const UI_FONT: &str = "Space Grotesk";
/// The card behind the empty-workspace hint, so it stays legible on any background.
pub const HINT_OPACITY: f64 = 0.85;
/// Columns layout: a column is never narrower than this (or the code region, if smaller).
pub const MIN_COLUMN_WIDTH: f64 = 480.0;

/// The configurable part of the layout (`tileide.tiling`, `appearance.window_radius`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub tile_radius: f64,
    pub gaps_in: f64,
    pub gaps_out: f64,
}

impl Default for Geometry {
    fn default() -> Self {
        Geometry { tile_radius: TILE_RADIUS, gaps_in: GAP_TILE_IN, gaps_out: GAP_TILE_OUT }
    }
}

impl Geometry {
    /// The frame's inner corners, concentric with the tile corners.
    pub fn frame_radius(&self) -> f64 {
        self.tile_radius + self.gaps_out
    }
}
