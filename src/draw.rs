// SPDX-License-Identifier: GPL-3.0-or-later
//! Cairo drawing of the canvas: the frame (rail + thin screen border, one shape with
//! a rounded hole, as in `design/caelestia-reference.png`), rail modules, the gap
//! ring and the outlined window. Text goes through Pango (font fallback, shaping).
//! Pure function of `Scene`; real tile and drawer widgets sit on top.

use std::f64::consts::{FRAC_PI_2, PI};

use gtk4::cairo::{Context, Error, FillRule, ImageSurface, Operator};
use gtk4::pango;

/// design.md §4.1 tokens, in logical pixels.
pub mod token {
    pub const RAIL_WIDTH: f64 = 50.0;
    pub const DRAWER_WIDTH: f64 = 280.0;
    pub const DRAWER_MIN_WIDTH: f64 = 200.0;
    pub const DRAWER_MAX_WIDTH: f64 = 600.0;
    /// Frame border on the top, right and bottom screen edges (Caelestia reference ≈ 10 px).
    pub const FRAME_BORDER: f64 = 10.0;
    /// Radius of the frame's inner (hole) corners (Caelestia reference ≈ 25 px).
    pub const FRAME_RADIUS: f64 = 25.0;
    pub const FRAME_OPACITY: f64 = 0.95;
    pub const GAP_RAIL: f64 = 24.0;
    /// Frame's inner edge to the window (top, right, bottom).
    pub const GAP_OUTER: f64 = 20.0;
    pub const WINDOW_RADIUS: f64 = 20.0;
    pub const TILE_RADIUS: f64 = 10.0;
    pub const GAP_TILE_IN: f64 = 8.0;
    pub const GAP_TILE_OUT: f64 = 12.0;
    pub const TILE_OPACITY: f64 = 0.92;
    pub const UI_FONT: &str = "Space Grotesk";
}

/// "Dusk" palette (design.md §4.3) as 0..1 RGB.
pub mod colour {
    pub type Rgb = (f64, f64, f64);
    pub const fn rgb(hex: u32) -> Rgb {
        (((hex >> 16) & 0xFF) as f64 / 255.0, ((hex >> 8) & 0xFF) as f64 / 255.0, (hex & 0xFF) as f64 / 255.0)
    }
    pub const SURFACE_BASE: Rgb = rgb(0x0E1013);
    pub const SURFACE_EDITOR: Rgb = rgb(0x1B1F26);
    pub const TEXT: Rgb = rgb(0xE6E8EC);
    pub const MUTED: Rgb = rgb(0x9AA1AD);
    pub const ACCENT: Rgb = rgb(0x4FD1D9);
}

/// Opacity of the window (region 3) fill, design.md §4.2.
pub const WINDOW_ALPHA: f64 = 0.6;

pub struct Scene<'a> {
    pub width: f64,
    pub height: f64,
    pub solid: bool,
    /// Current drawer extension in px (0 = collapsed), already eased.
    pub drawer: f64,
    /// Push: the window shrinks and `gap.rail` is kept. Overlay: the drawer covers the window edge.
    pub push: bool,
    /// Workspace numbers that exist (1..=10, where 10 is `Alt+0`), in rail order.
    pub workspaces: &'a [u8],
    /// `None` while the Config workspace is shown.
    pub active: Option<u8>,
    pub label: &'a str,
    pub clock: (&'a str, &'a str),
    /// Show the "no tiles yet" hint in the window.
    pub empty: bool,
    /// Draw placeholder tiles (offscreen `render` only).
    pub placeholder_tiles: bool,
    /// Full-screen app background (spike 6), already baked at this size; `None` = see-through.
    pub background: Option<&'a ImageSurface>,
    /// Editor tile opacity (offscreen placeholder tiles; real tiles use CSS).
    pub tile_opacity: f64,
}

/// Clickable things on the rail; positions shared by drawing and hit-testing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RailHit {
    Launcher,
    Workspace(u8),
    NewWorkspace,
    /// The gear above the clock: the Config workspace.
    Config,
}

const LAUNCHER_Y: f64 = 26.0;
const WORKSPACES_TOP: f64 = 58.0;
const WORKSPACE_STEP: f64 = 20.0;
const ACTIVE_PILL: f64 = 26.0;
/// Top of the clock's first line, from the bottom of the screen.
const CLOCK_FROM_BOTTOM: f64 = 64.0;
/// Centre of the gear, from the bottom of the screen (just above the clock).
const GEAR_FROM_BOTTOM: f64 = 86.0;

/// (hit, centre y, hit radius) for each rail item, top to bottom.
pub fn rail_items(s: &Scene) -> Vec<(RailHit, f64, f64)> {
    let mut items = vec![(RailHit::Launcher, LAUNCHER_Y, 14.0)];
    let mut y = WORKSPACES_TOP;
    for &n in s.workspaces {
        let height = if Some(n) == s.active { ACTIVE_PILL } else { 8.0 };
        items.push((RailHit::Workspace(n), y + height / 2.0, 9.0));
        y += height + WORKSPACE_STEP - 8.0;
    }
    items.push((RailHit::NewWorkspace, y + 10.0, 10.0));
    items.push((RailHit::Config, s.height - GEAR_FROM_BOTTOM, 12.0));
    items
}

pub fn rail_hit(s: &Scene, x: f64, y: f64) -> Option<RailHit> {
    if x > token::RAIL_WIDTH {
        return None;
    }
    rail_items(s).into_iter().find(|(_, cy, r)| (y - cy).abs() <= *r).map(|(hit, _, _)| hit)
}

/// Where the window (region 3) sits for a scene; real tile widgets are laid out inside it.
pub fn window_rect(s: &Scene) -> (f64, f64, f64, f64) {
    use token::*;
    let x = RAIL_WIDTH + GAP_RAIL + if s.push { s.drawer } else { 0.0 };
    let y = FRAME_BORDER + GAP_OUTER;
    (x, y, s.width - x - FRAME_BORDER - GAP_OUTER, s.height - 2.0 * y)
}

pub fn draw(cr: &Context, s: &Scene) -> Result<(), Error> {
    use token::*;
    // design.md §4.2 opacities; the solid fallback makes every surface opaque.
    let (frame_alpha, window_alpha) = if s.solid { (1.0, 1.0) } else { (FRAME_OPACITY, WINDOW_ALPHA) };

    // Region 0 / 4b: the full-screen background across the whole canvas, else
    // transparent. Windowed mode never gets one (spec §4.4).
    cr.set_operator(Operator::Source);
    match s.background {
        Some(background) => cr.set_source_surface(background, 0.0, 0.0)?,
        None => cr.set_source_rgba(0.0, 0.0, 0.0, if s.solid { 1.0 } else { 0.0 }),
    }
    cr.paint()?;
    cr.set_operator(Operator::Over);

    draw_window(cr, s, window_alpha)?;

    // Drawn last so that in overlay mode the (near-solid) drawer covers the window edge.
    frame_path(cr, s.width, s.height, s.drawer);
    set(cr, colour::SURFACE_BASE, frame_alpha);
    cr.set_fill_rule(FillRule::EvenOdd);
    cr.fill()?;
    cr.set_fill_rule(FillRule::Winding);
    draw_rail_modules(cr, s)
}

/// The frame is the screen rectangle minus one rounded hole. The rail is the frame's
/// wide left side; opening the drawer widens it, and the hole's rounded corners
/// move with it, so rail, drawer and border always read as one piece.
fn frame_path(cr: &Context, width: f64, height: f64, extension: f64) {
    use token::*;
    cr.new_path();
    cr.rectangle(0.0, 0.0, width, height);
    let left = RAIL_WIDTH + extension;
    rounded_rect(cr, left, FRAME_BORDER, width - left - FRAME_BORDER, height - 2.0 * FRAME_BORDER, [FRAME_RADIUS; 4]);
}

fn draw_window(cr: &Context, s: &Scene, window_alpha: f64) -> Result<(), Error> {
    use token::*;
    let (wx, wy, ww, wh) = window_rect(s);
    if ww <= 2.0 * WINDOW_RADIUS || wh <= 2.0 * WINDOW_RADIUS {
        return Ok(());
    }
    rounded_rect(cr, wx, wy, ww, wh, [WINDOW_RADIUS; 4]);
    set(cr, colour::SURFACE_BASE, window_alpha);
    cr.fill_preserve()?;
    cr.set_line_width(1.0);
    set(cr, colour::ACCENT, 0.35);
    cr.stroke()?;

    if s.empty {
        let lines = [
            ("No tiles open", 15.0, colour::TEXT),
            ("Alt+Return   terminal", 13.0, colour::MUTED),
            ("Alt+E   new file", 13.0, colour::MUTED),
            ("Alt+B   files", 13.0, colour::MUTED),
            ("Alt+Z   word wrap", 13.0, colour::MUTED),
            ("Alt+1 … 9, 0   workspaces", 13.0, colour::MUTED),
        ];
        let mut y = wy + wh / 2.0 - 60.0;
        for (text, size, colour) in lines {
            let (w, h) = text_size(cr, text, UI_FONT, size);
            set(cr, colour, 0.9);
            draw_text(cr, wx + (ww - w) / 2.0, y, text, UI_FONT, size);
            y += h + 8.0;
        }
    }
    if s.placeholder_tiles {
        draw_placeholder_tiles(cr, wx, wy, ww, wh, s.tile_opacity)?;
    }
    Ok(())
}

fn draw_rail_modules(cr: &Context, s: &Scene) -> Result<(), Error> {
    let cx = token::RAIL_WIDTH / 2.0;
    for (hit, y, _) in rail_items(s) {
        match hit {
            // 1a launcher (also the drawer toggle)
            RailHit::Launcher => {
                cr.arc(cx, y, 10.0, 0.0, 2.0 * PI);
                set(cr, colour::ACCENT, 0.9);
                cr.fill()?;
            }
            // 1b workspaces: the active one is a pill, the others dots
            RailHit::Workspace(n) if Some(n) == s.active => {
                rounded_rect(cr, cx - 5.0, y - ACTIVE_PILL / 2.0, 10.0, ACTIVE_PILL, [5.0; 4]);
                set(cr, colour::ACCENT, 0.9);
                cr.fill()?;
            }
            RailHit::Workspace(_) => {
                cr.arc(cx, y, 4.0, 0.0, 2.0 * PI);
                set(cr, colour::MUTED, 0.8);
                cr.fill()?;
            }
            // + under the workspaces: create the next free one
            RailHit::NewWorkspace => {
                set(cr, colour::MUTED, 0.8);
                cr.set_line_width(1.5);
                cr.move_to(cx - 5.0, y);
                cr.line_to(cx + 5.0, y);
                cr.move_to(cx, y - 5.0);
                cr.line_to(cx, y + 5.0);
                cr.stroke()?;
            }
            RailHit::Config => draw_gear(cr, cx, y, if s.active.is_none() { (colour::ACCENT, 0.9) } else { (colour::MUTED, 0.8) })?,
        }
    }

    // 1c vertical label (rotated, as in Caelestia)
    cr.save()?;
    let (w, _) = text_size(cr, s.label, token::UI_FONT, 13.0);
    cr.translate(cx - 9.0, s.height / 2.0 + w / 2.0);
    cr.rotate(-FRAC_PI_2);
    set(cr, colour::TEXT, 0.9);
    draw_text(cr, 0.0, 0.0, s.label, token::UI_FONT, 13.0);
    cr.restore()?;

    // 1d clock (read-only)
    set(cr, colour::TEXT, 0.9);
    for (i, line) in [s.clock.0, s.clock.1].iter().enumerate() {
        let (w, _) = text_size(cr, line, token::UI_FONT, 13.0);
        draw_text(cr, cx - w / 2.0, s.height - CLOCK_FROM_BOTTOM + i as f64 * 18.0, line, token::UI_FONT, 13.0);
    }
    Ok(())
}

/// Eight square teeth on a ring with a hole, filled as one even-odd path.
fn draw_gear(cr: &Context, cx: f64, cy: f64, (colour, alpha): ((f64, f64, f64), f64)) -> Result<(), Error> {
    const TEETH: usize = 8;
    let (outer, inner, hole) = (8.0, 6.0, 2.5);
    cr.new_path();
    let step = 2.0 * PI / TEETH as f64;
    for i in 0..TEETH {
        // Each tooth spans the first half of its step at the outer radius.
        let a = i as f64 * step;
        cr.arc(cx, cy, outer, a, a + step / 2.0);
        cr.arc(cx, cy, inner, a + step / 2.0, a + step);
    }
    cr.close_path();
    cr.new_sub_path();
    cr.arc(cx, cy, hole, 0.0, 2.0 * PI);
    set(cr, colour, alpha);
    cr.set_fill_rule(FillRule::EvenOdd);
    cr.fill()?;
    cr.set_fill_rule(FillRule::Winding);
    Ok(())
}

/// Offscreen `render` only: three fake tiles to judge the window without widgets.
fn draw_placeholder_tiles(cr: &Context, wx: f64, wy: f64, ww: f64, wh: f64, tile_opacity: f64) -> Result<(), Error> {
    use token::*;
    let (cx, cy) = (wx + GAP_TILE_OUT, wy + GAP_TILE_OUT);
    let (cw, ch) = (ww - 2.0 * GAP_TILE_OUT, wh - 2.0 * GAP_TILE_OUT);
    let half_w = (cw - GAP_TILE_IN) / 2.0;
    let half_h = (ch - GAP_TILE_IN) / 2.0;
    let tiles = [
        (cx, cy, half_w, ch, "main.rs"),
        (cx + half_w + GAP_TILE_IN, cy, half_w, half_h, "config.rs"),
        (cx + half_w + GAP_TILE_IN, cy + half_h + GAP_TILE_IN, half_w, half_h, "terminal"),
    ];
    for (x, y, w, h, label) in tiles {
        if w <= 2.0 * TILE_RADIUS {
            continue;
        }
        rounded_rect(cr, x, y, w, h, [TILE_RADIUS; 4]);
        set(cr, colour::SURFACE_EDITOR, tile_opacity);
        cr.fill()?;
        set(cr, colour::MUTED, 1.0);
        draw_text(cr, x + 16.0, y + 14.0, label, "monospace", 13.0);
        set(cr, colour::TEXT, 1.0);
        for (i, line) in ["fn main() {", "    let tiles = 3;", "    println!(\"{tiles}\");", "}"].iter().enumerate() {
            draw_text(cr, x + 16.0, y + 44.0 + i as f64 * 20.0, line, "monospace", 13.0);
        }
    }
    Ok(())
}

fn layout(cr: &Context, text: &str, family: &str, size_px: f64) -> pango::Layout {
    let layout = pangocairo::functions::create_layout(cr);
    let mut font = pango::FontDescription::new();
    font.set_family(family);
    font.set_absolute_size(size_px * pango::SCALE as f64);
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

/// Rounded rectangle with per-corner radii: top-left, top-right, bottom-right, bottom-left.
fn rounded_rect(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: [f64; 4]) {
    cr.new_sub_path();
    cr.arc(x + w - r[1], y + r[1], r[1], -FRAC_PI_2, 0.0);
    cr.arc(x + w - r[2], y + h - r[2], r[2], 0.0, FRAC_PI_2);
    cr.arc(x + r[3], y + h - r[3], r[3], FRAC_PI_2, PI);
    cr.arc(x + r[0], y + r[0], r[0], PI, PI + FRAC_PI_2);
    cr.close_path();
}

fn set(cr: &Context, (r, g, b): (f64, f64, f64), alpha: f64) {
    cr.set_source_rgba(r, g, b, alpha);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(workspaces: &[u8], active: Option<u8>) -> Scene<'_> {
        Scene {
            width: 1280.0,
            height: 720.0,
            solid: false,
            drawer: 0.0,
            push: true,
            workspaces,
            active,
            label: "tileIDE",
            clock: ("12", "00"),
            empty: true,
            placeholder_tiles: false,
            background: None,
            tile_opacity: token::TILE_OPACITY,
        }
    }

    #[test]
    fn rail_hit_finds_each_item() {
        let s = scene(&[1, 2, 5], Some(2));
        for (hit, y, _) in rail_items(&s) {
            assert_eq!(rail_hit(&s, 25.0, y), Some(hit));
        }
        assert_eq!(rail_hit(&s, 200.0, LAUNCHER_Y), None, "outside the rail");
    }

    #[test]
    fn plus_button_sits_below_the_workspaces() {
        let s = scene(&[1, 2, 3], Some(1));
        let items = rail_items(&s);
        let plus = items.iter().find(|i| i.0 == RailHit::NewWorkspace).unwrap();
        let last_workspace = items.iter().rfind(|i| matches!(i.0, RailHit::Workspace(_))).unwrap();
        assert!(plus.1 > last_workspace.1);
    }

    #[test]
    fn gear_sits_just_above_the_clock() {
        let s = scene(&[1], Some(1));
        let (_, y, r) = *rail_items(&s).iter().find(|i| i.0 == RailHit::Config).unwrap();
        assert!(y + r <= s.height - CLOCK_FROM_BOTTOM, "gear overlaps the clock");
        assert_eq!(rail_hit(&s, 25.0, y), Some(RailHit::Config));
    }
}
