// SPDX-License-Identifier: GPL-3.0-or-later
//! Cairo drawing of the canvas: the frame (rail + thin screen border, one shape with a
//! rounded hole, as in `design/caelestia-reference.png`) and the rail modules
//! (`themes/theme-design.md` §2), grouped on two pills (`design/new_sidebar_theme_logo.png`).
//! Tiles sit straight in the hole, on the background; glass themes get a frosted copy of
//! the background under each tile.
//! Text goes through Pango (font fallback, shaping). Pure function of `Scene`; real
//! tile and drawer widgets sit on top.

use std::f64::consts::{FRAC_PI_2, PI};
use std::rc::Rc;

use gtk4::cairo::{Context, Error, FillRule, ImageSurface, Operator};
use gtk4::pango;
use tileide_core::theme::{Glass, Rgb, Theme};
use tileide_core::tiles::Rect;
use tileide_core::token::{self, Geometry, *};

thread_local! {
    static TILE_BACKDROP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the canvas paints a blurred backdrop under the tiles (then it must redraw
/// whenever tiles move).
pub fn tile_backdrop() -> bool {
    TILE_BACKDROP.get()
}

pub fn set_tile_backdrop(on: bool) {
    TILE_BACKDROP.set(on);
}

pub struct Scene<'a> {
    pub width: f64,
    pub height: f64,
    pub theme: Rc<Theme>,
    pub geometry: Geometry,
    pub modules: Modules,
    /// The theme tray is open (the palette button shows its active state).
    pub themes_open: bool,
    /// Settings is open (its gear shows its active state).
    pub settings_open: bool,
    /// Audio visualizer bars (`2 × BANDS`, 0..1) and how shown they are (0..1).
    pub visualizer: Option<(&'a [f64], f64)>,
    pub solid: bool,
    /// Current drawer extension in px (0 = collapsed), already eased.
    pub drawer: f64,
    /// Push: the tiles move right with the drawer. Overlay: the drawer covers their edge.
    pub push: bool,
    /// Workspace numbers on the rail (1..=10, where 10 is `Alt+0`), in rail order, each
    /// with its presence 0..1: a dot grows and fades in when its workspace is created and
    /// shrinks out when it is removed.
    pub workspaces: &'a [(u8, f64)],
    /// `None` while the Config workspace is shown.
    pub active: Option<u8>,
    pub clock: (&'a str, &'a str),
    /// Show the "no tiles yet" hint.
    pub empty: bool,
    /// Draw placeholder tiles (offscreen `render` only).
    pub placeholder_tiles: bool,
    /// Full-screen app background (spike 6), already baked at this size; `None` = see-through.
    pub background: Option<&'a ImageSurface>,
    /// The background blurred for under the tiles (Tiles & terminals → Blur).
    pub frosted: Option<&'a ImageSurface>,
    /// The background blurred for under the frame (Sidebar → Blur).
    pub frosted_frame: Option<&'a ImageSurface>,
    /// The frame's fill opacity (Sidebar → Transparency).
    pub frame_opacity: f64,
    /// The shown tiles, for the frosted backdrop.
    pub tile_rects: &'a [Rect],
    /// Tile fill opacity (the empty tile and offscreen placeholders; real tiles use CSS).
    pub tile_opacity: f64,
}

/// Which rail modules are shown (`tileide.sidebar { show_* }`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Modules {
    pub launcher: bool,
    pub workspaces: bool,
    /// The logo mid-rail (`show_label`; it was the workspace name before v0.4).
    pub label: bool,
    pub clock: bool,
    pub themes: bool,
    pub config: bool,
    /// The discs and squares behind the rail's icons (`sidebar.icon_backgrounds`); without
    /// them, an active icon is drawn in the accent colour instead.
    pub icon_backgrounds: bool,
}

impl Default for Modules {
    fn default() -> Self {
        Modules { launcher: true, workspaces: true, label: true, clock: true, themes: true, config: true, icon_backgrounds: true }
    }
}

/// Clickable things on the rail; positions shared by drawing and hit-testing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RailHit {
    /// The file-tree opener (toggles the drawer).
    Launcher,
    Workspace(u8),
    NewWorkspace,
    /// The palette button: the theme tray.
    Themes,
    /// The folder-with-a-cog under the workspace dots: the Config workspace.
    Config,
    /// The gear above the clock: Settings.
    Settings,
}

const BUTTON: f64 = 28.0;
/// The two pills the modules sit on: inset from the rail's edges, with round ends
/// concentric with the first and last item. Buttons keep 6px to the pill's sides
/// (less at their rounded corners, which sit in the pill's round end).
const PILL_INSET: f64 = 4.0;
const _: () = assert!((RAIL_WIDTH - 2.0 * PILL_INSET - BUTTON) / 2.0 >= 6.0, "rail buttons crowd the pills' sides");
const PILL_END: f64 = (RAIL_WIDTH - 2.0 * PILL_INSET) / 2.0;
const LAUNCHER_Y: f64 = PILL_INSET * 2.0 + PILL_END;
const WORKSPACES_TOP: f64 = LAUNCHER_Y + BUTTON / 2.0 + 12.0;
const ACTIVE_PILL: (f64, f64) = (10.0, 28.0);
const DOT: f64 = 9.0;
const PLUS_RING: f64 = 22.0;
/// The Config workspace's gear among the dots.
const CONFIG_GEAR: f64 = 22.0;
const ITEM_GAP: f64 = 20.0;
/// The logo in the middle of the rail.
const MID_LOGO: f64 = 24.0;
/// Centres, measured from the bottom of the screen.
const GEAR_FROM_BOTTOM: f64 = 90.0;
const THEMES_FROM_BOTTOM: f64 = 128.0;
const HOUR_FROM_BOTTOM: f64 = 52.0;
const MINUTE_FROM_BOTTOM: f64 = PILL_INSET * 2.0 + PILL_END;

/// The screen's height in rail units: the rail is laid out at its designed size and
/// drawn scaled by `token::rail_scale()` (Settings → Sidebar size).
fn rail_height(s: &Scene) -> f64 {
    s.height / token::rail_scale()
}

/// (hit, centre y, hit radius) for each rail item, top to bottom, on screen.
pub fn rail_items(s: &Scene) -> Vec<(RailHit, f64, f64)> {
    let k = token::rail_scale();
    rail_units(s).into_iter().map(|(hit, y, r)| (hit, y * k, r * k)).collect()
}

/// `rail_items` in rail units.
fn rail_units(s: &Scene) -> Vec<(RailHit, f64, f64)> {
    let mut items = Vec::new();
    if s.modules.launcher {
        items.push((RailHit::Launcher, LAUNCHER_Y, BUTTON / 2.0));
    }
    if s.modules.workspaces {
        let mut top = WORKSPACES_TOP;
        for &(n, k) in s.workspaces {
            let height = if Some(n) == s.active { ACTIVE_PILL.1 } else { DOT } * k;
            // At least the pill's half height, so small dots stay easy to hit.
            items.push((RailHit::Workspace(n), top + height / 2.0, (height / 2.0).max(10.0)));
            top += height + ITEM_GAP * k;
        }
        // The Config workspace: always below the open workspaces, above `+`.
        if s.modules.config {
            items.push((RailHit::Config, top + CONFIG_GEAR / 2.0, CONFIG_GEAR / 2.0));
            top += CONFIG_GEAR + ITEM_GAP * 0.6;
        }
        items.push((RailHit::NewWorkspace, top + PLUS_RING / 2.0, PLUS_RING / 2.0));
    } else if s.modules.config {
        items.push((RailHit::Config, WORKSPACES_TOP + CONFIG_GEAR / 2.0, CONFIG_GEAR / 2.0));
    }
    if s.modules.themes {
        items.push((RailHit::Themes, rail_height(s) - THEMES_FROM_BOTTOM, BUTTON / 2.0));
    }
    items.push((RailHit::Settings, rail_height(s) - GEAR_FROM_BOTTOM, BUTTON / 2.0));
    items
}

/// The rail's pills: the top one holds the opener and the workspaces, the bottom one
/// the palette, the gear and the clock. A group with no shown module has no pill.
/// In rail units.
fn pills(s: &Scene) -> Vec<Rect> {
    let items = rail_units(s);
    let span = |ys: &[f64]| {
        let (top, bottom) = (ys.iter().copied().fold(f64::MAX, f64::min), ys.iter().copied().fold(f64::MIN, f64::max));
        (top <= bottom).then_some(Rect { x: PILL_INSET, y: top - PILL_END, w: RAIL_WIDTH - 2.0 * PILL_INSET, h: bottom - top + 2.0 * PILL_END })
    };
    let top: Vec<f64> = items.iter().filter(|i| matches!(i.0, RailHit::Launcher | RailHit::Workspace(_) | RailHit::NewWorkspace | RailHit::Config)).map(|i| i.1).collect();
    let mut bottom: Vec<f64> = items.iter().filter(|i| matches!(i.0, RailHit::Themes | RailHit::Settings)).map(|i| i.1).collect();
    if s.modules.clock {
        bottom.extend([rail_height(s) - HOUR_FROM_BOTTOM, rail_height(s) - MINUTE_FROM_BOTTOM]);
    }
    [span(&top), span(&bottom)].into_iter().flatten().collect()
}

pub fn rail_hit(s: &Scene, x: f64, y: f64) -> Option<RailHit> {
    if x > token::rail_width() {
        return None;
    }
    rail_items(s).into_iter().find(|(_, cy, r)| (y - cy).abs() <= *r).map(|(hit, _, _)| hit)
}

/// The frame's rounded hole (the stage) the tiles sit in.
pub fn stage_rect(s: &Scene) -> Rect {
    let x = token::rail_width() + s.drawer;
    Rect { x, y: FRAME_BORDER, w: s.width - x - FRAME_BORDER, h: s.height - 2.0 * FRAME_BORDER }
}

/// Where tiles are laid out: the stage inset by `gaps_out`. In overlay mode it does not
/// move with the drawer.
pub fn tile_area(s: &Scene) -> Rect {
    let out = s.geometry.gaps_out;
    let x = token::rail_width() + out + if s.push { s.drawer } else { 0.0 };
    let y = FRAME_BORDER + out;
    Rect { x, y, w: (s.width - x - FRAME_BORDER - out).max(0.0), h: (s.height - 2.0 * y).max(0.0) }
}

pub fn draw(cr: &Context, s: &Scene) -> Result<(), Error> {
    let t = &*s.theme;
    // The solid fallback makes every surface opaque.
    let frame_alpha = if s.solid { 1.0 } else { s.frame_opacity };

    // The full-screen background across the whole canvas, else transparent. Windowed
    // mode never gets one (spec §4.4).
    cr.set_operator(Operator::Source);
    match s.background {
        Some(background) => cr.set_source_surface(background, 0.0, 0.0)?,
        None if s.solid => set(cr, t.surface, 1.0),
        None => cr.set_source_rgba(0.0, 0.0, 0.0, 0.0),
    }
    cr.paint()?;
    cr.set_operator(Operator::Over);

    let area = tile_area(s);
    let placeholders = placeholder_tiles(s.geometry, area);
    if let Some(frosted) = s.frosted {
        let rects: Vec<Rect> = match (s.placeholder_tiles, s.empty) {
            (true, _) => placeholders.iter().map(|p| p.0).collect(),
            (false, true) => vec![area],
            (false, false) => s.tile_rects.to_vec(),
        };
        for r in rects.into_iter().filter(|r| r.w > 2.0 * s.geometry.tile_radius && r.h > 2.0 * s.geometry.tile_radius) {
            rounded_rect(cr, r, s.geometry.tile_radius);
        }
        cr.set_source_surface(frosted, 0.0, 0.0)?;
        cr.fill()?;
    }
    if s.placeholder_tiles {
        draw_placeholder_tiles(cr, t, s.geometry, &placeholders, s.tile_opacity)?;
    } else if s.empty {
        draw_empty_tile(cr, s, area)?;
    }

    // Drawn last so that in overlay mode the (near-solid) drawer covers the tiles' edge.
    if let Some(frosted) = s.frosted_frame {
        frame_path(cr, s);
        cr.set_fill_rule(FillRule::EvenOdd);
        cr.set_source_surface(frosted, 0.0, 0.0)?;
        cr.fill()?;
    }
    frame_path(cr, s);
    set(cr, t.surface, frame_alpha);
    cr.set_fill_rule(FillRule::EvenOdd);
    cr.fill()?;
    cr.set_fill_rule(FillRule::Winding);
    draw_rail_modules(cr, s)
}

/// The frame is the screen rectangle minus one rounded hole. The rail is the frame's
/// wide left side; opening the drawer widens it, and the hole's rounded corners
/// move with it, so rail, drawer and border always read as one piece.
fn frame_path(cr: &Context, s: &Scene) {
    let hole = stage_rect(s);
    let radius = s.geometry.frame_radius();
    cr.new_path();
    cr.rectangle(0.0, 0.0, s.width, s.height);
    if hole.w > 2.0 * radius && hole.h > 2.0 * radius {
        rounded_rect(cr, hole, radius);
    }
}

/// What an empty workspace shows, as (what, keys) rows (VS Code's empty-editor style).
const EMPTY_HINTS: &[(&str, &str)] = &[
    ("Open a file", "Alt+E"),
    ("New terminal", "Alt+Return"),
    ("Open a folder", "Alt+D"),
    ("Files", "Alt+B"),
    ("Run the focused file", "Alt+R"),
    ("Settings", "Alt+,"),
    ("All keybinds", "Alt+K"),
];

/// An empty workspace: one tile filling the code region, with a faded logo and the
/// main keys centred in it, so it reads like an editor waiting for a file.
fn draw_empty_tile(cr: &Context, s: &Scene, area: Rect) -> Result<(), Error> {
    let (t, g) = (&*s.theme, s.geometry);
    if area.w < 2.0 * g.tile_radius || area.h < 2.0 * g.tile_radius {
        return Ok(());
    }
    rounded_rect(cr, area, g.tile_radius);
    // Like a tile (any frosted backdrop is already under it).
    set(cr, t.tile, s.tile_opacity);
    cr.fill_preserve()?;
    set(cr, t.text, 0.08);
    cr.set_line_width(2.0);
    cr.stroke()?;

    const LOGO_SIZE: f64 = 96.0;
    const ROW: f64 = 30.0;
    const COLUMN_GAP: f64 = 16.0;
    let scale = token::ui_scale();
    let rows = if area.h > 420.0 { EMPTY_HINTS.len() } else { 4 };
    let height = LOGO_SIZE + 36.0 + rows as f64 * ROW * scale;
    if height > area.h - 24.0 {
        return Ok(());
    }
    let cx = area.x + area.w / 2.0;
    let mut y = area.y + (area.h - height) / 2.0;
    set(cr, t.text, 0.10);
    crate::logo::fill(cr, cx, y + LOGO_SIZE / 2.0, LOGO_SIZE)?;
    y += LOGO_SIZE + 36.0;
    for (what, keys) in &EMPTY_HINTS[..rows] {
        let (w, h) = text_size(cr, what, UI_FONT, 13.0);
        set(cr, t.muted, 1.0);
        draw_text(cr, cx - COLUMN_GAP / 2.0 - w, y + (ROW * scale - h) / 2.0, what, UI_FONT, 13.0);
        // Keys on a soft chip, left-aligned in the right column.
        let (kw, kh) = text_size(cr, keys, UI_FONT, 12.0);
        let chip = Rect { x: cx + COLUMN_GAP / 2.0, y: y + (ROW * scale - kh) / 2.0 - 3.0, w: kw + 14.0, h: kh + 6.0 };
        rounded_rect(cr, chip, 6.0);
        set(cr, t.text, 0.07);
        cr.fill()?;
        set(cr, t.text, 0.85);
        draw_text(cr, chip.x + 7.0, chip.y + 3.0, keys, UI_FONT, 12.0);
        y += ROW * scale;
    }
    Ok(())
}

fn presence(s: &Scene, n: u8) -> f64 {
    s.workspaces.iter().find(|w| w.0 == n).map_or(1.0, |w| w.1)
}

fn draw_rail_modules(cr: &Context, s: &Scene) -> Result<(), Error> {
    cr.save()?;
    cr.scale(token::rail_scale(), token::rail_scale());
    let result = draw_rail_units(cr, s);
    cr.restore()?;
    result
}

/// The rail at its designed size (`draw_rail_modules` scales it).
fn draw_rail_units(cr: &Context, s: &Scene) -> Result<(), Error> {
    let t = &*s.theme;
    let cx = RAIL_WIDTH / 2.0;
    for pill in pills(s) {
        rounded_rect(cr, pill, PILL_END);
        set(cr, t.text, 0.06);
        cr.fill_preserve()?;
        set(cr, t.text, 0.08);
        cr.set_line_width(1.0);
        cr.stroke()?;
    }
    let icons = token::icon_scale();
    for (hit, y, _) in rail_units(s) {
        // Settings → Icon size: each icon grows or shrinks around its own centre.
        cr.save()?;
        cr.translate(cx, y);
        cr.scale(icons, icons);
        cr.translate(-cx, -y);
        match hit {
            RailHit::Launcher => draw_launcher(cr, t, cx, y, s.drawer > 0.0, s.modules.icon_backgrounds)?,
            // Current workspace: a plain accent pill.
            RailHit::Workspace(n) if Some(n) == s.active => {
                let k = presence(s, n);
                let (w, h) = (ACTIVE_PILL.0 * k, ACTIVE_PILL.1 * k);
                rounded_rect(cr, Rect { x: cx - w / 2.0, y: y - h / 2.0, w, h }, w / 2.0);
                set(cr, t.accent, k);
                cr.fill()?;
            }
            RailHit::Workspace(n) => {
                let k = presence(s, n);
                cr.arc(cx, y, DOT / 2.0 * k, 0.0, 2.0 * PI);
                set(cr, t.muted, 0.7 * k);
                cr.fill()?;
            }
            // Dashed ring with a plus: create the next free workspace.
            RailHit::NewWorkspace => {
                set(cr, t.muted, 0.9);
                cr.set_line_width(1.2);
                cr.set_dash(&[2.5, 2.5], 0.0);
                cr.arc(cx, y, PLUS_RING / 2.0 - 0.6, 0.0, 2.0 * PI);
                cr.stroke()?;
                cr.set_dash(&[], 0.0);
                cr.set_line_width(1.5);
                cr.move_to(cx - 4.5, y);
                cr.line_to(cx + 4.5, y);
                cr.move_to(cx, y - 4.5);
                cr.line_to(cx, y + 4.5);
                cr.stroke()?;
            }
            RailHit::Themes => {
                let fg = rail_button(cr, t, cx, y, s.themes_open, s.modules.icon_backgrounds)?;
                draw_palette(cr, cx, y, fg)?;
            }
            // Like a workspace: a muted folder-with-a-cog (the config folder; the plain gear
            // below is Settings), or an accent disc while the Config workspace shows.
            RailHit::Config => {
                let fg = if s.active.is_none() && !s.modules.icon_backgrounds {
                    t.accent
                } else if s.active.is_none() {
                    cr.arc(cx, y, CONFIG_GEAR / 2.0, 0.0, 2.0 * PI);
                    set(cr, t.accent, 1.0);
                    cr.fill()?;
                    t.on_accent
                } else {
                    t.muted
                };
                draw_config_folder(cr, cx, y, fg)?;
            }
            RailHit::Settings => {
                let fg = rail_button(cr, t, cx, y, s.settings_open, s.modules.icon_backgrounds)?;
                draw_gear(cr, cx, y, fg)?;
            }
        }
        cr.restore()?;
    }

    // The visualizer takes the label's place while the drawer is closed (with it open,
    // it sits under the file tree instead).
    let wave = s.visualizer.filter(|(_, k)| *k > 0.0 && s.drawer <= 0.0);
    if s.modules.label {
        let k = wave.map_or(0.0, |(_, k)| k);
        if k < 1.0 {
            // The logo, quiet, mid-rail (it was the workspace name until v0.4).
            set(cr, t.muted, 0.55 * (1.0 - k));
            crate::logo::fill(cr, cx, rail_height(s) / 2.0, MID_LOGO)?;
        }
    }
    if let Some((bars, k)) = wave {
        let h = (bars.len() as f64 * 6.0).min(rail_height(s) * 0.3);
        let area = Rect { x: cx - (RAIL_WIDTH - 2.0 * PILL_INSET) / 2.0 + 4.0, y: rail_height(s) / 2.0 - h / 2.0, w: RAIL_WIDTH - 2.0 * PILL_INSET - 8.0, h };
        waveform(cr, area, bars, t.accent, k, true)?;
    }
    if s.modules.clock {
        draw_clock(cr, s, cx)?;
    }
    Ok(())
}

/// Clock (read-only): hour over minute with a hairline between.
fn draw_clock(cr: &Context, s: &Scene, cx: f64) -> Result<(), Error> {
    let t = &*s.theme;
    set(cr, t.text, 0.9);
    for (text, from_bottom) in [(s.clock.0, HOUR_FROM_BOTTOM), (s.clock.1, MINUTE_FROM_BOTTOM)] {
        let (w, h) = text_size(cr, text, UI_FONT, 14.0);
        draw_text(cr, cx - w / 2.0, rail_height(s) - from_bottom - h / 2.0, text, UI_FONT, 14.0);
    }
    set(cr, t.muted, 0.5);
    let mid = rail_height(s) - (HOUR_FROM_BOTTOM + MINUTE_FROM_BOTTOM) / 2.0;
    cr.move_to(cx - 5.0, mid.round() + 0.5);
    cr.line_to(cx + 5.0, mid.round() + 0.5);
    cr.stroke()
}

/// The file-tree opener: a 28px circle with the tree icon, filled accent while the tree
/// is open, a 2px accent ring while it is closed (just the icon without backgrounds).
fn draw_launcher(cr: &Context, t: &Theme, cx: f64, cy: f64, open: bool, background: bool) -> Result<(), Error> {
    let r = BUTTON / 2.0;
    let fg = if !background {
        if open { t.accent } else { t.muted }
    } else if open {
        cr.arc(cx, cy, r, 0.0, 2.0 * PI);
        set(cr, t.accent, 1.0);
        cr.fill()?;
        t.on_accent
    } else {
        cr.arc(cx, cy, r - 1.0, 0.0, 2.0 * PI);
        set(cr, t.accent, 1.0);
        cr.set_line_width(2.0);
        cr.stroke()?;
        t.accent
    };
    draw_tree_icon(cr, cx, cy, fg)
}

/// The file tree: a trunk with three branches to rounded nodes, 14px.
fn draw_tree_icon(cr: &Context, cx: f64, cy: f64, colour: Rgb) -> Result<(), Error> {
    set(cr, colour, 1.0);
    cr.set_line_width(1.4);
    cr.set_line_cap(gtk4::cairo::LineCap::Round);
    cr.set_line_join(gtk4::cairo::LineJoin::Round);
    let (x0, top) = (cx - 4.5, cy - 6.0);
    // Root node, then the trunk down from it with branches to two children.
    rounded_rect(cr, Rect { x: x0 - 2.0, y: top - 1.0, w: 6.0, h: 4.0 }, 1.2);
    cr.fill()?;
    cr.move_to(x0, top + 3.0);
    cr.line_to(x0, cy + 5.0);
    cr.move_to(x0, cy);
    cr.line_to(cx + 0.5, cy);
    cr.move_to(x0, cy + 5.0);
    cr.line_to(cx + 0.5, cy + 5.0);
    cr.stroke()?;
    for y in [cy, cy + 5.0] {
        rounded_rect(cr, Rect { x: cx + 1.0, y: y - 2.0, w: 6.0, h: 4.0 }, 1.2);
        cr.fill()?;
    }
    cr.set_line_cap(gtk4::cairo::LineCap::Butt);
    Ok(())
}

/// 28px rounded square behind a rail icon: tinted when idle, filled accent when
/// active. Returns the icon colour. Without backgrounds, only the colour says it.
fn rail_button(cr: &Context, t: &Theme, cx: f64, cy: f64, active: bool, background: bool) -> Result<Rgb, Error> {
    if !background {
        return Ok(if active { t.accent } else { t.muted });
    }
    rounded_rect(cr, Rect { x: cx - BUTTON / 2.0, y: cy - BUTTON / 2.0, w: BUTTON, h: BUTTON }, 10.0);
    set(cr, t.accent, if active { 1.0 } else { 0.14 });
    cr.fill()?;
    Ok(if active { t.on_accent } else { t.accent })
}

/// Palette: a rounded outline with a thumb notch, and three paint dots.
fn draw_palette(cr: &Context, cx: f64, cy: f64, colour: Rgb) -> Result<(), Error> {
    set(cr, colour, 1.0);
    cr.set_line_width(1.5);
    cr.new_path();
    cr.arc(cx, cy, 8.5, 0.35 * PI, 2.15 * PI);
    // Thumb notch, bottom right.
    cr.curve_to(cx + 5.5, cy + 2.0, cx + 2.0, cy + 3.0, cx + 2.5, cy + 6.0);
    cr.close_path();
    cr.stroke()?;
    for (dx, dy) in [(-3.8, -1.0), (-1.0, -4.2), (3.0, -3.0)] {
        cr.arc(cx + dx, cy + dy, 1.3, 0.0, 2.0 * PI);
        cr.fill()?;
    }
    Ok(())
}

/// The Config workspace: a folder outline with a small cog in it, 16px wide.
fn draw_config_folder(cr: &Context, cx: f64, cy: f64, colour: Rgb) -> Result<(), Error> {
    set(cr, colour, 1.0);
    cr.set_line_width(1.3);
    cr.set_line_join(gtk4::cairo::LineJoin::Round);
    cr.new_path();
    cr.move_to(cx - 7.5, cy - 5.0);
    cr.line_to(cx - 2.8, cy - 5.0);
    cr.line_to(cx - 1.3, cy - 3.4);
    cr.line_to(cx + 7.5, cy - 3.4);
    cr.line_to(cx + 7.5, cy + 5.5);
    cr.line_to(cx - 7.5, cy + 5.5);
    cr.close_path();
    cr.stroke()?;
    // Cog: a ring with six teeth, centred in the folder's body.
    let (gx, gy) = (cx, cy + 1.1);
    cr.set_line_width(1.1);
    cr.new_sub_path();
    cr.arc(gx, gy, 1.9, 0.0, 2.0 * PI);
    cr.stroke()?;
    cr.set_line_cap(gtk4::cairo::LineCap::Round);
    for i in 0..6 {
        let a = i as f64 * PI / 3.0;
        cr.move_to(gx + 1.9 * a.cos(), gy + 1.9 * a.sin());
        cr.line_to(gx + 3.1 * a.cos(), gy + 3.1 * a.sin());
    }
    cr.stroke()?;
    cr.set_line_cap(gtk4::cairo::LineCap::Butt);
    Ok(())
}

/// A symmetrical waveform: one bar per level (0..1), each drawn equally on both sides
/// of the centre line. `vertical`: bars stacked top to bottom, extending left and
/// right (the rail); else side by side, extending up and down (under the file tree).
pub fn waveform(cr: &Context, r: Rect, bars: &[f64], colour: Rgb, alpha: f64, vertical: bool) -> Result<(), Error> {
    if bars.is_empty() || r.w <= 0.0 || r.h <= 0.0 {
        return Ok(());
    }
    let n = bars.len() as f64;
    let (along, across) = if vertical { (r.h, r.w) } else { (r.w, r.h) };
    let pitch = along / n;
    let thickness = (pitch * 0.55).clamp(1.5, 4.0);
    set(cr, colour, alpha);
    cr.set_line_width(thickness);
    cr.set_line_cap(gtk4::cairo::LineCap::Round);
    for (i, level) in bars.iter().enumerate() {
        // A dot at rest, so the line reads as a waveform even in quiet passages.
        let half = (level.clamp(0.0, 1.0) * (across - thickness) / 2.0).max(0.01);
        let at = pitch * (i as f64 + 0.5);
        if vertical {
            let (y, cx) = (r.y + at, r.x + r.w / 2.0);
            cr.move_to(cx - half, y);
            cr.line_to(cx + half, y);
        } else {
            let (x, cy) = (r.x + at, r.y + r.h / 2.0);
            cr.move_to(x, cy - half);
            cr.line_to(x, cy + half);
        }
    }
    cr.stroke()?;
    cr.set_line_cap(gtk4::cairo::LineCap::Butt);
    Ok(())
}

/// Outline gear: eight teeth on a ring, and a hub.
fn draw_gear(cr: &Context, cx: f64, cy: f64, colour: Rgb) -> Result<(), Error> {
    const TEETH: usize = 8;
    let (outer, inner, hub) = (8.5, 6.5, 2.6);
    let step = 2.0 * PI / TEETH as f64;
    cr.new_path();
    for i in 0..TEETH {
        let a = i as f64 * step;
        cr.arc(cx, cy, outer, a + step * 0.1, a + step * 0.5);
        cr.arc(cx, cy, inner, a + step * 0.6, a + step * 1.0);
    }
    cr.close_path();
    cr.new_sub_path();
    cr.arc(cx, cy, hub, 0.0, 2.0 * PI);
    set(cr, colour, 1.0);
    cr.set_line_width(1.5);
    cr.set_line_join(gtk4::cairo::LineJoin::Round);
    cr.stroke()
}

/// Offscreen `render` only: three fake tiles to judge the layout without widgets.
fn placeholder_tiles(g: Geometry, area: Rect) -> [(Rect, &'static str, bool); 3] {
    let half_w = (area.w - g.gaps_in) / 2.0;
    let half_h = (area.h - g.gaps_in) / 2.0;
    let right = area.x + half_w + g.gaps_in;
    [
        (Rect { w: half_w, ..area }, "main.rs", true),
        (Rect { x: right, w: half_w, h: half_h, ..area }, "config.rs", false),
        (Rect { x: right, y: area.y + half_h + g.gaps_in, w: half_w, h: half_h }, "terminal", false),
    ]
}

/// Paints like `tile_view::css`: a solid fill at `tile_opacity`, or the glass gradient.
fn draw_placeholder_tiles(cr: &Context, t: &Theme, g: Geometry, tiles: &[(Rect, &str, bool)], tile_opacity: f64) -> Result<(), Error> {
    for &(r, label, focused) in tiles {
        let radius = g.tile_radius;
        if r.w <= 2.0 * radius || r.h <= 2.0 * radius {
            continue;
        }
        rounded_rect(cr, r, radius);
        match t.glass {
            Some(glass) => cr.set_source(glass_fill(t, glass, r, tile_opacity))?,
            None => set(cr, t.tile, tile_opacity),
        }
        cr.fill()?;
        let inset = Rect { x: r.x + 1.0, y: r.y + 1.0, w: r.w - 2.0, h: r.h - 2.0 };
        rounded_rect(cr, inset, radius - 1.0);
        match (focused, t.glass) {
            (true, Some(glass)) => set(cr, t.accent, glass.focus_opacity),
            (true, None) => set(cr, t.accent, 1.0),
            (false, Some(glass)) => set(cr, t.text, glass.edge_opacity),
            (false, None) => set(cr, t.text, 0.08),
        }
        cr.set_line_width(2.0);
        cr.stroke()?;
        set(cr, t.muted, 1.0);
        draw_text(cr, r.x + 24.0, r.y + 18.0, label, "monospace", 13.0);
        set(cr, t.text, 1.0);
        for (i, line) in ["fn main() {", "    let tiles = 3;", "    println!(\"{tiles}\");", "}"].iter().enumerate() {
            draw_text(cr, r.x + 24.0, r.y + 48.0 + i as f64 * 24.0, line, "monospace", 13.0);
        }
    }
    Ok(())
}

/// CSS `linear-gradient(160deg, …)` across `r`: from the top-left-ish corner down.
fn glass_fill(t: &Theme, glass: Glass, r: Rect, opacity: f64) -> gtk4::cairo::LinearGradient {
    let (dx, dy) = (160f64.to_radians().sin(), -160f64.to_radians().cos());
    let half = (r.w * dx.abs() + r.h * dy.abs()) / 2.0;
    let (mx, my) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let gradient = gtk4::cairo::LinearGradient::new(mx - dx * half, my - dy * half, mx + dx * half, my + dy * half);
    let (a, b) = (t.tile, glass.bottom);
    gradient.add_color_stop_rgba(0.0, a.0, a.1, a.2, opacity);
    gradient.add_color_stop_rgba(1.0, b.0, b.1, b.2, (opacity + glass.bottom_opacity - glass.opacity).clamp(0.0, 1.0));
    gradient
}

fn layout(cr: &Context, text: &str, family: &str, size_px: f64) -> pango::Layout {
    let layout = pangocairo::functions::create_layout(cr);
    let mut font = pango::FontDescription::new();
    // Interface text follows Settings' font and scale; code text doesn't.
    if family == UI_FONT {
        font.set_family(&token::ui_font());
        font.set_absolute_size(size_px * token::ui_scale() * pango::SCALE as f64);
    } else {
        font.set_family(family);
        font.set_absolute_size(size_px * pango::SCALE as f64);
    }
    layout.set_font_description(Some(&font));
    layout.set_text(text);
    layout
}

fn text_size(cr: &Context, text: &str, family: &str, size_px: f64) -> (f64, f64) {
    let (w, h) = layout(cr, text, family, size_px).pixel_size();
    (w as f64, h as f64)
}

/// Draws `text` with its top-left corner at (x, y) in the current source colour.
fn draw_text(cr: &Context, x: f64, y: f64, text: &str, family: &str, size_px: f64) {
    let layout = layout(cr, text, family, size_px);
    cr.move_to(x, y);
    pangocairo::functions::show_layout(cr, &layout);
}

fn rounded_rect(cr: &Context, r: Rect, radius: f64) {
    cr.new_sub_path();
    cr.arc(r.x + r.w - radius, r.y + radius, radius, -FRAC_PI_2, 0.0);
    cr.arc(r.x + r.w - radius, r.y + r.h - radius, radius, 0.0, FRAC_PI_2);
    cr.arc(r.x + radius, r.y + r.h - radius, radius, FRAC_PI_2, PI);
    cr.arc(r.x + radius, r.y + radius, radius, PI, PI + FRAC_PI_2);
    cr.close_path();
}

fn set(cr: &Context, (r, g, b): Rgb, alpha: f64) {
    cr.set_source_rgba(r, g, b, alpha);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(workspaces: &[u8], active: Option<u8>) -> Scene<'static> {
        let workspaces = Box::leak(workspaces.iter().map(|&n| (n, 1.0)).collect::<Box<[_]>>());
        Scene {
            width: 1280.0,
            height: 720.0,
            theme: Rc::new(Theme::builtin()),
            geometry: Geometry::default(),
            modules: Modules::default(),
            themes_open: false,
            settings_open: false,
            visualizer: None,
            solid: false,
            drawer: 0.0,
            push: true,
            workspaces,
            active,
            clock: ("12", "00"),
            empty: true,
            placeholder_tiles: false,
            background: None,
            frosted: None,
            frosted_frame: None,
            frame_opacity: FRAME_OPACITY,
            tile_rects: &[],
            tile_opacity: TILE_OPACITY,
        }
    }

    #[test]
    fn rail_hit_finds_each_item() {
        let s = scene(&[1, 2, 5], Some(2));
        for (hit, y, _) in rail_items(&s) {
            assert_eq!(rail_hit(&s, 30.0, y), Some(hit));
        }
        assert_eq!(rail_hit(&s, 200.0, LAUNCHER_Y), None, "outside the rail");
    }

    #[test]
    fn rail_items_do_not_overlap() {
        let s = scene(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10], Some(4));
        let items = rail_items(&s);
        for pair in items.windows(2) {
            assert!(pair[0].1 + pair[0].2 <= pair[1].1 - pair[1].2, "{:?} overlaps {:?}", pair[0].0, pair[1].0);
        }
    }

    #[test]
    fn hidden_modules_take_no_space_or_clicks() {
        let mut s = scene(&[1, 2], Some(1));
        s.modules = Modules { workspaces: false, themes: false, ..Modules::default() };
        let hits: Vec<_> = rail_items(&s).into_iter().map(|i| i.0).collect();
        assert_eq!(hits, vec![RailHit::Launcher, RailHit::Config, RailHit::Settings]);
    }

    #[test]
    fn config_gear_follows_the_workspaces_and_settings_sits_above_the_clock() {
        let s = scene(&[1, 2, 3], Some(1));
        let items = rail_items(&s);
        let at = |hit| items.iter().position(|i| i.0 == hit).unwrap();
        assert_eq!(at(RailHit::Config), at(RailHit::Workspace(3)) + 1, "right below the last dot");
        assert_eq!(at(RailHit::NewWorkspace), at(RailHit::Config) + 1, "then +");
        let (_, y, r) = items[at(RailHit::Settings)];
        assert!(y + r <= s.height - HOUR_FROM_BOTTOM - 8.0, "settings gear overlaps the clock");
    }

    #[test]
    fn pills_hold_their_items_and_stay_apart() {
        let s = scene(&[1, 2, 3], Some(1));
        let pills = pills(&s);
        assert_eq!(pills.len(), 2);
        for (_, y, r) in rail_items(&s) {
            assert!(pills.iter().any(|p| y - r >= p.y && y + r <= p.y + p.h), "item at {y} sticks out");
        }
        assert!(pills[0].y + pills[0].h < pills[1].y, "pills overlap");
        assert_eq!(pills[1].y + pills[1].h, s.height - 2.0 * PILL_INSET, "bottom pill keeps the top pill's margin");
        assert_eq!(pills[0].y, 2.0 * PILL_INSET);
    }

    /// Every button's rounded-square corner stays inside the pill's round end.
    #[test]
    fn buttons_clear_the_pill_ends() {
        let corner = 10.0;
        let half = BUTTON / 2.0;
        let reach = (half - corner) * std::f64::consts::SQRT_2 + corner;
        assert!(PILL_END - reach >= 4.0, "corner gap {}", PILL_END - reach);
    }

    #[test]
    fn tiles_sit_gaps_out_inside_the_stage() {
        let mut s = scene(&[1], Some(1));
        s.drawer = 100.0;
        let (stage, area) = (stage_rect(&s), tile_area(&s));
        assert_eq!(area.x - stage.x, GAP_TILE_OUT);
        assert_eq!(area.y - stage.y, GAP_TILE_OUT);
        assert_eq!((stage.x + stage.w) - (area.x + area.w), GAP_TILE_OUT);
        assert_eq!(s.geometry.frame_radius(), TILE_RADIUS + GAP_TILE_OUT, "corners stay concentric");
        s.push = false;
        assert_eq!(tile_area(&s).x, RAIL_WIDTH + GAP_TILE_OUT, "overlay: tiles stay put");
    }
}
