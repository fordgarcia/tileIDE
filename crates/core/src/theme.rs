// SPDX-License-Identifier: GPL-3.0-or-later
//! Colours only (`themes/theme-design.md`). Every drawing path and stylesheet takes a
//! `Theme`; nothing else holds colour literals. Themes load from `themes/*.lua`
//! (`tileide-config`); the built-in one is the fallback when a theme file fails.

/// 0..1 RGB.
pub type Rgb = (f64, f64, f64);

pub const fn rgb(hex: u32) -> Rgb {
    (((hex >> 16) & 0xFF) as f64 / 255.0, ((hex >> 8) & 0xFF) as f64 / 255.0, (hex & 0xFF) as f64 / 255.0)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: String,
    /// Picks the editor's light or dark syntax scheme.
    pub dark: bool,
    /// Frame, rail and drawer.
    pub surface: Rgb,
    /// Editor and terminal tiles.
    pub tile: Rgb,
    pub text: Rgb,
    pub muted: Rgb,
    pub accent: Rgb,
    /// Text drawn on an accent fill (filled rail buttons).
    pub on_accent: Rgb,
    pub error: Rgb,
    /// The theme tray's strip: surface, a mid (wallpaper) tone, accent.
    pub strip: [Rgb; 3],
    /// Translucent "glass" tiles (Black Glass); `None` = solid tiles at `tile_opacity`.
    pub glass: Option<Glass>,
    /// Terminal colours the theme sets itself (ANSI order); the rest are derived.
    pub ansi: [Option<Rgb>; 16],
    /// Syntax colours the theme sets itself (`SYNTAX_KEYS` order); the rest are derived.
    pub code: [Option<Rgb>; 8],
}

/// Theme file keys of `syntax = { … }`, in `Theme::code` order.
/// A border's colour, from the theme (`appearance.tile_border_color`, `frame_border_color`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Edge {
    /// A faint line in the text colour (glass themes: their own edge).
    #[default]
    Subtle,
    Accent,
    Text,
    Muted,
}

impl Edge {
    pub const KEYS: [&'static str; 4] = ["subtle", "accent", "text", "muted"];

    pub fn key(self) -> &'static str {
        Edge::KEYS[self as usize]
    }

    pub fn from_key(key: &str) -> Option<Edge> {
        [Edge::Subtle, Edge::Accent, Edge::Text, Edge::Muted].into_iter().find(|e| e.key() == key)
    }

    /// The colour and opacity to draw with in `theme`.
    pub fn paint(self, theme: &Theme) -> (Rgb, f64) {
        match self {
            Edge::Subtle => (theme.text, theme.glass.map_or(0.08, |g| g.edge_opacity)),
            Edge::Accent => (theme.accent, 1.0),
            Edge::Text => (theme.text, 0.9),
            Edge::Muted => (theme.muted, 0.8),
        }
    }
}

pub const SYNTAX_KEYS: [&str; 8] = ["keyword", "type", "string", "constant", "function", "preprocessor", "builtin", "comment"];
/// Theme file keys of `terminal = { … }`, in ANSI order.
pub const ANSI_KEYS: [&str; 16] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white", "bright_black", "bright_red", "bright_green", "bright_yellow", "bright_blue", "bright_magenta", "bright_cyan", "bright_white"];

/// Settings → Themes & background: transparency in percent, blur in px; `None` = the
/// theme decides.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SurfaceChoice {
    pub sidebar_transparency: Option<f64>,
    pub sidebar_blur: Option<f64>,
    pub tile_transparency: Option<f64>,
    pub tile_blur: Option<f64>,
}

/// One surface's fill opacity (0..1) and backdrop blur (px; drawn only over a background
/// TileIDE paints itself).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layer {
    pub opacity: f64,
    pub blur: f64,
}

/// The sidebar (rail, border and drawer: the frame) and the tiles (editors, terminals).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Surfaces {
    pub sidebar: Layer,
    pub tiles: Layer,
}

/// Smoked-glass tiles (`themes/theme-design.md` §4): a diagonal fill from `tile` at
/// `opacity` to `bottom` at `bottom_opacity`, over a blurred, saturated copy of the
/// full-screen background.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glass {
    pub opacity: f64,
    pub bottom: Rgb,
    pub bottom_opacity: f64,
    /// Unfocused border, in the text colour.
    pub edge_opacity: f64,
    /// Focused border, in the accent colour.
    pub focus_opacity: f64,
    /// Backdrop blur in px (app-drawn full-screen backgrounds only).
    pub blur: f64,
    pub saturate: f64,
    /// Fill opacity floor where TileIDE cannot see what is behind the tiles
    /// (see-through and windowed), so it cannot dim for legibility.
    pub min_tint: f64,
}

impl Glass {
    /// The design's values (theme-design.md §4).
    pub const fn default() -> Glass {
        Glass { opacity: 0.56, bottom: (0.0, 0.0, 0.0), bottom_opacity: 0.66, edge_opacity: 0.24, focus_opacity: 0.60, blur: 30.0, saturate: 1.7, min_tint: 0.80 }
    }
}

impl Theme {
    /// "Dusk", warm since 0.3.0-alpha.3 (design.md §4.3 had a cool teal).
    pub fn builtin() -> Theme {
        Theme {
            name: "Dusk".into(),
            dark: true,
            surface: rgb(0x14110F),
            tile: rgb(0x201B18),
            text: rgb(0xEDE6DD),
            muted: rgb(0xA99D90),
            accent: rgb(0xF0A37A),
            on_accent: rgb(0x14110F),
            error: rgb(0xE8735F),
            strip: [rgb(0x14110F), rgb(0x6B5244), rgb(0xF0A37A)],
            glass: None,
            ansi: [None; 16],
            code: [None; 8],
        }
    }

    /// How see-through and blurred the sidebar and the tiles are: the user's choice
    /// where there is one, else this theme's look (glass tiles: its opacity and blur;
    /// solid ones: `legacy_tile_opacity`, from `background.tile_opacity`).
    /// `see_behind`: TileIDE draws the background; where it can't, glass tiles keep their
    /// `min_tint` floor (unless the user chose a transparency).
    pub fn surfaces(&self, choice: SurfaceChoice, legacy_tile_opacity: f64, see_behind: bool) -> Surfaces {
        let opacity = |percent: f64| (1.0 - percent / 100.0).clamp(0.0, 1.0);
        let tile_opacity = match (choice.tile_transparency, self.glass) {
            (Some(t), _) => opacity(t),
            (None, Some(g)) if see_behind => g.opacity,
            (None, Some(g)) => g.opacity.max(g.min_tint),
            (None, None) => legacy_tile_opacity,
        };
        Surfaces {
            sidebar: Layer { opacity: choice.sidebar_transparency.map_or(crate::token::FRAME_OPACITY, opacity), blur: choice.sidebar_blur.unwrap_or(0.0) },
            tiles: Layer { opacity: tile_opacity, blur: choice.tile_blur.or(self.glass.map(|g| g.blur)).unwrap_or(0.0) },
        }
    }

    /// The terminal's 16 ANSI colours, so prompts, `ls` and `git` output match the theme
    /// instead of VTE's stock xterm palette. Red is the theme's `error`; the other hues
    /// are soft and pulled a little toward `accent`; every colour is nudged in lightness
    /// until it reads on the tile (≥ 4.5:1).
    pub fn terminal_palette(&self) -> [Rgb; 16] {
        let (bg, dark) = (self.tile, self.dark);
        let (normal_l, bright_l) = if dark { (0.68, 0.76) } else { (0.38, 0.30) };
        let tone = |hue: f64, s: f64, l: f64| legible(over(self.accent, 0.12, from_hsl(hue, s, l)), bg, dark);
        let (red_h, red_s, red_l) = to_hsl(self.error);
        // ANSI order after black: red, green, yellow, blue, magenta, cyan.
        let hues = [red_h, 100.0, 42.0, 215.0, 320.0, 180.0];
        let mut p = [(0.0, 0.0, 0.0); 16];
        for (i, hue) in hues.into_iter().enumerate() {
            p[1 + i] = tone(hue, 0.45, normal_l);
            p[9 + i] = tone(hue, 0.55, bright_l);
        }
        p[1] = legible(self.error, bg, dark);
        p[9] = legible(from_hsl(red_h, red_s, red_l + if dark { 0.08 } else { -0.08 }), bg, dark);
        // Black and white keep their usual meaning (darkest, lightest) in both modes.
        let (ink, paper) = if dark { (self.surface, self.text) } else { (self.text, self.surface) };
        p[0] = if dark { over(self.text, 0.15, ink) } else { ink };
        p[7] = over(ink, 0.2, paper);
        p[8] = legible(self.muted, bg, dark);
        p[15] = paper;
        for (slot, set) in p.iter_mut().zip(self.ansi) {
            if let Some(c) = set {
                *slot = c;
            }
        }
        p
    }
}

/// The editor's syntax colours. They reuse the terminal's hues, so code reads the same
/// in an editor tile and in `bat` or `git diff` in a terminal tile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Syntax {
    pub keyword: Rgb,
    pub type_: Rgb,
    pub string: Rgb,
    /// Numbers, booleans and other constants.
    pub constant: Rgb,
    pub function: Rgb,
    pub preprocessor: Rgb,
    pub builtin: Rgb,
    pub comment: Rgb,
}

impl Theme {
    pub fn syntax(&self) -> Syntax {
        let p = self.terminal_palette();
        let pick = |i: usize, derived: Rgb| self.code[i].unwrap_or(derived);
        Syntax { keyword: pick(0, p[5]), type_: pick(1, p[3]), string: pick(2, p[2]), constant: pick(3, p[6]), function: pick(4, p[4]), preprocessor: pick(5, p[13]), builtin: pick(6, p[14]), comment: pick(7, p[8]) }
    }
}

/// Moves `c` away from `bg` in lightness (lighter on dark themes) until it reaches 4.5:1.
fn legible(c: Rgb, bg: Rgb, dark: bool) -> Rgb {
    let (h, s, mut l) = to_hsl(c);
    let mut out = c;
    while contrast(out, bg) < 4.5 && (0.0..=1.0).contains(&l) {
        l += if dark { 0.02 } else { -0.02 };
        out = from_hsl(h, s, l.clamp(0.0, 1.0));
    }
    out
}

/// Hue in degrees, saturation and lightness 0..1.
fn to_hsl((r, g, b): Rgb) -> (f64, f64, f64) {
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let d = max - min;
    if d == 0.0 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h, s, l)
}

fn from_hsl(h: f64, s: f64, l: f64) -> Rgb {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h.rem_euclid(360.0) / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (r + m, g + m, b + m)
}

/// CSS `rgba()` for GTK stylesheets.
pub fn css(c: Rgb, alpha: f64) -> String {
    format!("rgba({}, {}, {}, {alpha})", (c.0 * 255.0).round(), (c.1 * 255.0).round(), (c.2 * 255.0).round())
}

pub fn luminance((r, g, b): Rgb) -> f64 {
    let lin = |c: f64| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// WCAG 2 contrast ratio.
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `top` at `alpha` over `under`, as cairo blends (in sRGB).
pub fn over(top: Rgb, alpha: f64, under: Rgb) -> Rgb {
    let mix = |t: f64, u: f64| t * alpha + u * (1.0 - alpha);
    (mix(top.0, under.0), mix(top.1, under.1), mix(top.2, under.2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surfaces_follow_the_theme_until_the_user_chooses() {
        let solid = Theme::builtin();
        let glass = Theme { glass: Some(Glass::default()), ..Theme::builtin() };
        let none = SurfaceChoice::default();
        let s = solid.surfaces(none, 0.92, true);
        assert_eq!((s.sidebar.opacity, s.sidebar.blur, s.tiles.opacity, s.tiles.blur), (crate::token::FRAME_OPACITY, 0.0, 0.92, 0.0));
        let g = glass.surfaces(none, 0.92, true);
        assert_eq!((g.tiles.opacity, g.tiles.blur), (0.56, 30.0), "the glass look");
        assert_eq!(glass.surfaces(none, 0.92, false).tiles.opacity, 0.80, "min_tint where TileIDE can't see behind");
        let mine = SurfaceChoice { sidebar_transparency: Some(40.0), sidebar_blur: Some(12.0), tile_transparency: Some(70.0), tile_blur: Some(0.0) };
        let m = glass.surfaces(mine, 0.92, false);
        assert!((m.sidebar.opacity - 0.6).abs() < 1e-9 && m.sidebar.blur == 12.0);
        assert!((m.tiles.opacity - 0.3).abs() < 1e-9 && m.tiles.blur == 0.0, "the user's choice wins over the theme and the floor");
    }

    #[test]
    fn builtin_text_passes_aa() {
        let t = Theme::builtin();
        for bg in [t.surface, t.tile] {
            assert!(contrast(t.text, bg) >= 4.5);
            assert!(contrast(t.muted, bg) >= 4.5);
        }
        assert!(contrast(t.on_accent, t.accent) >= 4.5);
    }

    #[test]
    fn terminal_colours_read_on_the_tile() {
        let light = Theme { dark: false, surface: rgb(0xF2E8D8), tile: rgb(0xFBF5EA), text: rgb(0x2B241C), muted: rgb(0x6E6253), accent: rgb(0x2E6B5E), error: rgb(0xB3402A), ..Theme::builtin() };
        for t in [Theme::builtin(), light] {
            let p = t.terminal_palette();
            for i in (1..=6).chain(8..=14) {
                assert!(contrast(p[i], t.tile) >= 4.5, "{}: colour {i} is {:.2}:1", t.name, contrast(p[i], t.tile));
            }
            assert_eq!(p[1], t.error, "{}: red is the theme's own", t.name);
            assert!(to_hsl(p[2]).0 > 60.0 && to_hsl(p[2]).0 < 160.0, "{}: green stays green", t.name);
            assert!(luminance(p[0]) < luminance(p[15]), "{}: black is darker than white", t.name);
        }
    }

    #[test]
    fn syntax_colours_read_and_differ() {
        let t = Theme::builtin();
        let s = t.syntax();
        let all = [s.keyword, s.type_, s.string, s.constant, s.function, s.preprocessor, s.builtin, s.comment];
        for (i, c) in all.iter().enumerate() {
            assert!(contrast(*c, t.tile) >= 4.5, "syntax colour {i}");
            assert!(all[..i].iter().all(|o| o != c), "syntax colour {i} repeats");
        }
    }

    #[test]
    fn theme_colours_override_derived_ones() {
        let mut t = Theme::builtin();
        t.ansi[1] = Some(rgb(0x123456));
        t.code[2] = Some(rgb(0xABCDEF));
        assert_eq!(t.terminal_palette()[1], rgb(0x123456));
        assert_eq!(t.terminal_palette()[2], Theme::builtin().terminal_palette()[2], "unset ones stay derived");
        assert_eq!(t.syntax().string, rgb(0xABCDEF));
        assert_eq!(t.syntax().keyword, Theme::builtin().syntax().keyword);
    }

    #[test]
    fn hsl_round_trips() {
        for c in [rgb(0xE8735F), rgb(0x2E6B5E), rgb(0x808080), rgb(0xFFF1E0)] {
            let (h, s, l) = to_hsl(c);
            let back = from_hsl(h, s, l);
            assert!((back.0 - c.0).abs() < 1e-9 && (back.1 - c.1).abs() < 1e-9 && (back.2 - c.2).abs() < 1e-9);
        }
    }

    #[test]
    fn css_rounds_channels() {
        assert_eq!(css(rgb(0x4FD1D9), 0.5), "rgba(79, 209, 217, 0.5)");
    }
}
