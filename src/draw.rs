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

pub struct Scene<'a> {
    pub width: f64,
    pub height: f64,
    pub theme: Rc<Theme>,
    pub geometry: Geometry,
    pub modules: Modules,
    /// The theme tray is open (the palette button shows its active state).
    pub themes_open: bool,
    pub solid: bool,
    /// Current drawer extension in px (0 = collapsed), already eased.
    pub drawer: f64,
    /// Push: the tiles move right with the drawer. Overlay: the drawer covers their edge.
    pub push: bool,
    /// Workspace numbers that exist (1..=10, where 10 is `Alt+0`), in rail order.
    pub workspaces: &'a [u8],
    /// `None` while the Config workspace is shown.
    pub active: Option<u8>,
    pub label: &'a str,
    pub clock: (&'a str, &'a str),
    /// Show the "no tiles yet" hint.
    pub empty: bool,
    /// Draw placeholder tiles (offscreen `render` only).
    pub placeholder_tiles: bool,
    /// Full-screen app background (spike 6), already baked at this size; `None` = see-through.
    pub background: Option<&'a ImageSurface>,
    /// Glass themes: the blurred, saturated background painted under each tile.
    pub frosted: Option<&'a ImageSurface>,
    /// The shown tiles, for the frosted backdrop.
    pub tile_rects: &'a [Rect],
    /// Editor tile opacity (offscreen placeholder tiles; real tiles use CSS).
    pub tile_opacity: f64,
}

/// Which rail modules are shown (`tileide.sidebar { show_* }`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Modules {
    pub launcher: bool,
    pub workspaces: bool,
    pub label: bool,
    pub clock: bool,
    pub themes: bool,
    pub config: bool,
}

impl Default for Modules {
    fn default() -> Self {
        Modules { launcher: true, workspaces: true, label: true, clock: true, themes: true, config: true }
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
    /// The gear above the clock: the Config workspace.
    Config,
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
const ITEM_GAP: f64 = 20.0;
const LOGO: f64 = 17.0;
/// Centres, measured from the bottom of the screen.
const GEAR_FROM_BOTTOM: f64 = 90.0;
const THEMES_FROM_BOTTOM: f64 = 128.0;
const HOUR_FROM_BOTTOM: f64 = 52.0;
const MINUTE_FROM_BOTTOM: f64 = PILL_INSET * 2.0 + PILL_END;

/// (hit, centre y, hit radius) for each rail item, top to bottom.
pub fn rail_items(s: &Scene) -> Vec<(RailHit, f64, f64)> {
    let mut items = Vec::new();
    if s.modules.launcher {
        items.push((RailHit::Launcher, LAUNCHER_Y, BUTTON / 2.0));
    }
    if s.modules.workspaces {
        let mut top = WORKSPACES_TOP;
        for &n in s.workspaces {
            let height = if Some(n) == s.active { ACTIVE_PILL.1 } else { DOT };
            // At least the pill's half height, so small dots stay easy to hit.
            items.push((RailHit::Workspace(n), top + height / 2.0, (height / 2.0).max(10.0)));
            top += height + ITEM_GAP;
        }
        items.push((RailHit::NewWorkspace, top + PLUS_RING / 2.0, PLUS_RING / 2.0));
    }
    if s.modules.themes {
        items.push((RailHit::Themes, s.height - THEMES_FROM_BOTTOM, BUTTON / 2.0));
    }
    if s.modules.config {
        items.push((RailHit::Config, s.height - GEAR_FROM_BOTTOM, BUTTON / 2.0));
    }
    items
}

/// The rail's pills: the top one holds the opener and the workspaces, the bottom one
/// the palette, the gear and the clock. A group with no shown module has no pill.
fn pills(s: &Scene) -> Vec<Rect> {
    let items = rail_items(s);
    let span = |ys: &[f64]| {
        let (top, bottom) = (ys.iter().copied().fold(f64::MAX, f64::min), ys.iter().copied().fold(f64::MIN, f64::max));
        (top <= bottom).then_some(Rect { x: PILL_INSET, y: top - PILL_END, w: RAIL_WIDTH - 2.0 * PILL_INSET, h: bottom - top + 2.0 * PILL_END })
    };
    let top: Vec<f64> = items.iter().filter(|i| matches!(i.0, RailHit::Launcher | RailHit::Workspace(_) | RailHit::NewWorkspace)).map(|i| i.1).collect();
    let mut bottom: Vec<f64> = items.iter().filter(|i| matches!(i.0, RailHit::Themes | RailHit::Config)).map(|i| i.1).collect();
    if s.modules.clock {
        bottom.extend([s.height - HOUR_FROM_BOTTOM, s.height - MINUTE_FROM_BOTTOM]);
    }
    [span(&top), span(&bottom)].into_iter().flatten().collect()
}

pub fn rail_hit(s: &Scene, x: f64, y: f64) -> Option<RailHit> {
    if x > RAIL_WIDTH {
        return None;
    }
    rail_items(s).into_iter().find(|(_, cy, r)| (y - cy).abs() <= *r).map(|(hit, _, _)| hit)
}

/// The frame's rounded hole (the stage) the tiles sit in.
pub fn stage_rect(s: &Scene) -> Rect {
    let x = RAIL_WIDTH + s.drawer;
    Rect { x, y: FRAME_BORDER, w: s.width - x - FRAME_BORDER, h: s.height - 2.0 * FRAME_BORDER }
}

/// Where tiles are laid out: the stage inset by `gaps_out`. In overlay mode it does not
/// move with the drawer.
pub fn tile_area(s: &Scene) -> Rect {
    let out = s.geometry.gaps_out;
    let x = RAIL_WIDTH + out + if s.push { s.drawer } else { 0.0 };
    let y = FRAME_BORDER + out;
    Rect { x, y, w: (s.width - x - FRAME_BORDER - out).max(0.0), h: (s.height - 2.0 * y).max(0.0) }
}

pub fn draw(cr: &Context, s: &Scene) -> Result<(), Error> {
    let t = &*s.theme;
    // design.md §4.2 opacities; the solid fallback makes every surface opaque.
    let frame_alpha = if s.solid { 1.0 } else { FRAME_OPACITY };

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
        let rects: Vec<Rect> = if s.placeholder_tiles { placeholders.iter().map(|p| p.0).collect() } else { s.tile_rects.to_vec() };
        for r in rects.into_iter().filter(|r| r.w > 2.0 * s.geometry.tile_radius && r.h > 2.0 * s.geometry.tile_radius) {
            rounded_rect(cr, r, s.geometry.tile_radius);
        }
        cr.set_source_surface(frosted, 0.0, 0.0)?;
        cr.fill()?;
    }
    if s.placeholder_tiles {
        draw_placeholder_tiles(cr, t, s.geometry, &placeholders, s.tile_opacity, s.background.is_some())?;
    } else if s.empty {
        draw_hint(cr, t, s.geometry, area)?;
    }

    // Drawn last so that in overlay mode the (near-solid) drawer covers the tiles' edge.
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

/// Key hints on a card, so they read on any background.
fn draw_hint(cr: &Context, t: &Theme, g: Geometry, area: Rect) -> Result<(), Error> {
    let lines = [
        ("No tiles open", 15.0, t.text),
        ("Alt+Return   terminal", 13.0, t.muted),
        ("Alt+E   open a file (new tile)", 13.0, t.muted),
        ("Alt+F   open a file here", 13.0, t.muted),
        ("Alt+B   files", 13.0, t.muted),
        ("Alt+Z   word wrap", 13.0, t.muted),
        ("Alt+V   columns layout", 13.0, t.muted),
        ("Alt+T   themes", 13.0, t.muted),
        ("Alt+1 … 9, 0   workspaces", 13.0, t.muted),
    ];
    const LINE_GAP: f64 = 8.0;
    const PADDING: f64 = 24.0;
    let sizes: Vec<(f64, f64)> = lines.iter().map(|(text, size, _)| text_size(cr, text, UI_FONT, *size)).collect();
    let width = sizes.iter().map(|s| s.0).fold(0.0, f64::max) + 2.0 * PADDING;
    let height = sizes.iter().map(|s| s.1 + LINE_GAP).sum::<f64>() - LINE_GAP + 2.0 * PADDING;
    if width > area.w || height > area.h {
        return Ok(());
    }
    let card = Rect { x: area.x + (area.w - width) / 2.0, y: area.y + (area.h - height) / 2.0, w: width, h: height };
    rounded_rect(cr, card, g.tile_radius);
    set(cr, t.surface, HINT_OPACITY);
    cr.fill()?;
    let mut y = card.y + PADDING;
    for ((text, size, colour), (w, h)) in lines.iter().zip(sizes) {
        set(cr, *colour, 1.0);
        draw_text(cr, card.x + (card.w - w) / 2.0, y, text, UI_FONT, *size);
        y += h + LINE_GAP;
    }
    Ok(())
}

fn draw_rail_modules(cr: &Context, s: &Scene) -> Result<(), Error> {
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
    for (hit, y, _) in rail_items(s) {
        match hit {
            RailHit::Launcher => draw_launcher(cr, t, cx, y, s.drawer > 0.0)?,
            // Current workspace: a plain accent pill.
            RailHit::Workspace(n) if Some(n) == s.active => {
                let (w, h) = ACTIVE_PILL;
                rounded_rect(cr, Rect { x: cx - w / 2.0, y: y - h / 2.0, w, h }, w / 2.0);
                set(cr, t.accent, 1.0);
                cr.fill()?;
            }
            RailHit::Workspace(_) => {
                cr.arc(cx, y, DOT / 2.0, 0.0, 2.0 * PI);
                set(cr, t.muted, 0.7);
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
                let fg = rail_button(cr, t, cx, y, s.themes_open)?;
                draw_palette(cr, cx, y, fg)?;
            }
            RailHit::Config => {
                let fg = rail_button(cr, t, cx, y, s.active.is_none())?;
                draw_gear(cr, cx, y, fg)?;
            }
        }
    }

    if s.modules.label {
        draw_label(cr, s, cx)?;
    }
    if s.modules.clock {
        draw_clock(cr, s, cx)?;
    }
    Ok(())
}

/// Vertical label (rotated, as in Caelestia), letter-spaced.
fn draw_label(cr: &Context, s: &Scene, cx: f64) -> Result<(), Error> {
    let t = &*s.theme;
    cr.save()?;
    let label = label_layout(cr, s.label);
    let (w, h) = label.pixel_size();
    cr.translate(cx - h as f64 / 2.0, s.height / 2.0 + w as f64 / 2.0);
    cr.rotate(-FRAC_PI_2);
    set(cr, t.muted, 1.0);
    cr.move_to(0.0, 0.0);
    pangocairo::functions::show_layout(cr, &label);
    cr.restore()
}

/// Clock (read-only): hour over minute with a hairline between.
fn draw_clock(cr: &Context, s: &Scene, cx: f64) -> Result<(), Error> {
    let t = &*s.theme;
    set(cr, t.text, 0.9);
    for (text, from_bottom) in [(s.clock.0, HOUR_FROM_BOTTOM), (s.clock.1, MINUTE_FROM_BOTTOM)] {
        let (w, h) = text_size(cr, text, UI_FONT, 14.0);
        draw_text(cr, cx - w / 2.0, s.height - from_bottom - h / 2.0, text, UI_FONT, 14.0);
    }
    set(cr, t.muted, 0.5);
    let mid = s.height - (HOUR_FROM_BOTTOM + MINUTE_FROM_BOTTOM) / 2.0;
    cr.move_to(cx - 5.0, mid.round() + 0.5);
    cr.line_to(cx + 5.0, mid.round() + 0.5);
    cr.stroke()
}

/// The file-tree opener: a 28px circle with the logo, filled accent while the tree is
/// open, a 2px accent ring while it is closed.
fn draw_launcher(cr: &Context, t: &Theme, cx: f64, cy: f64, open: bool) -> Result<(), Error> {
    let r = BUTTON / 2.0;
    let fg = if open {
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
    set(cr, fg, 1.0);
    crate::logo::fill(cr, cx, cy, LOGO)
}

/// 28px rounded square behind a rail icon: tinted when idle, filled accent when
/// active. Returns the icon colour.
fn rail_button(cr: &Context, t: &Theme, cx: f64, cy: f64, active: bool) -> Result<Rgb, Error> {
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
fn draw_placeholder_tiles(cr: &Context, t: &Theme, g: Geometry, tiles: &[(Rect, &str, bool)], tile_opacity: f64, see_behind: bool) -> Result<(), Error> {
    for &(r, label, focused) in tiles {
        let radius = g.tile_radius;
        if r.w <= 2.0 * radius || r.h <= 2.0 * radius {
            continue;
        }
        rounded_rect(cr, r, radius);
        match t.glass {
            Some(glass) => cr.set_source(glass_fill(t, glass, r, see_behind))?,
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
fn glass_fill(t: &Theme, glass: Glass, r: Rect, see_behind: bool) -> gtk4::cairo::LinearGradient {
    let floor = if see_behind { 0.0 } else { glass.min_tint };
    let (dx, dy) = (160f64.to_radians().sin(), -160f64.to_radians().cos());
    let half = (r.w * dx.abs() + r.h * dy.abs()) / 2.0;
    let (mx, my) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let gradient = gtk4::cairo::LinearGradient::new(mx - dx * half, my - dy * half, mx + dx * half, my + dy * half);
    let (a, b) = (t.tile, glass.bottom);
    gradient.add_color_stop_rgba(0.0, a.0, a.1, a.2, glass.opacity.max(floor));
    gradient.add_color_stop_rgba(1.0, b.0, b.1, b.2, glass.bottom_opacity.max(floor));
    gradient
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

/// 13px, medium weight, letter-spaced (theme-design.md §2).
fn label_layout(cr: &Context, text: &str) -> pango::Layout {
    let l = layout(cr, text, token::UI_FONT, 13.0);
    let mut font = l.font_description().unwrap_or_default();
    font.set_weight(pango::Weight::Medium);
    l.set_font_description(Some(&font));
    let attrs = pango::AttrList::new();
    attrs.insert(pango::AttrInt::new_letter_spacing(pango::SCALE));
    l.set_attributes(Some(&attrs));
    l
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

    fn scene<'a>(workspaces: &'a [u8], active: Option<u8>) -> Scene<'a> {
        Scene {
            width: 1280.0,
            height: 720.0,
            theme: Rc::new(Theme::builtin()),
            geometry: Geometry::default(),
            modules: Modules::default(),
            themes_open: false,
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
            frosted: None,
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
        assert_eq!(hits, vec![RailHit::Launcher, RailHit::Config]);
    }

    #[test]
    fn gear_sits_above_the_clock() {
        let s = scene(&[1], Some(1));
        let (_, y, r) = *rail_items(&s).iter().find(|i| i.0 == RailHit::Config).unwrap();
        assert!(y + r <= s.height - HOUR_FROM_BOTTOM - 8.0, "gear overlaps the clock");
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
