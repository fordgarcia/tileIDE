// SPDX-License-Identifier: GPL-3.0-or-later
//! Spike 6: the full-screen background layer (spec §4.4, region 4b). Drawn across the
//! whole canvas, behind the frame, the gap ring and the translucent tiles. Windowed
//! mode never has one. App-drawn modes are baked once per window size into one surface
//! (fit + blur + dim + tint), so a frame costs a single paint.

use std::path::PathBuf;

use gtk4::cairo::{self, Context, Filter, Format, ImageSurface, Operator, SurfacePattern};
use gtk4::gdk_pixbuf::Pixbuf;

use tileide_core::theme::{Rgb, Theme, contrast, over, rgb};
use tileide_core::token;

#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// The desktop shows through (compositor blur); TileIDE draws nothing.
    SeeThrough,
    Image(PathBuf),
    /// Top to bottom.
    Gradient(Rgb, Rgb),
    Solid(Rgb),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fit {
    Cover,
    Contain,
    Tile,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub source: Source,
    /// 0..1 black overlay.
    pub dim: f64,
    /// Colour overlay and its strength 0..1.
    pub tint: Option<(Rgb, f64)>,
    /// Editor and terminal tile background opacity.
    pub tile_opacity: f64,
    /// App-side blur radius in px (app-drawn modes only).
    pub blur: f64,
    /// Backdrop blur under the tiles and under the sidebar (frame), px; 0 = none.
    pub tile_blur: f64,
    pub frame_blur: f64,
    pub fit: Fit,
    /// Raise `dim` until text passes the contrast check (spec §4.4 legibility guard).
    pub auto_dim: bool,
}

/// design.md §4.2: editor tiles stay at least 88% opaque.
pub const MIN_TILE_OPACITY: f64 = 0.88;
/// design.md §7: WCAG AA for body text.
pub const MIN_CONTRAST: f64 = 4.5;

impl Default for Settings {
    fn default() -> Self {
        Settings {
            source: Source::SeeThrough,
            dim: 0.0,
            tint: None,
            tile_opacity: token::TILE_OPACITY,
            blur: 0.0,
            tile_blur: 0.0,
            frame_blur: 0.0,
            fit: Fit::Cover,
            auto_dim: true,
        }
    }
}

pub const USAGE: &str = "--bg see-through|image:PATH|gradient:RRGGBB,RRGGBB|solid:RRGGBB [--dim 0..100] [--tint RRGGBB@0..100] [--tile-opacity 88..100] [--blur PX] [--fit cover|contain|tile] [--no-auto-dim]";

impl Settings {
    /// Reads the background flags; values are percentages on the command line.
    pub fn from_args(args: &[String]) -> Result<Settings, String> {
        let value = |flag: &str| args.iter().position(|a| a == flag).map(|i| args.get(i + 1).cloned().ok_or(format!("{flag} needs a value")));
        let percent = |flag: &str| -> Result<Option<f64>, String> {
            value(flag)
                .transpose()?
                .map(|v| v.parse::<f64>().ok().filter(|p| (0.0..=100.0).contains(p)).map(|p| p / 100.0).ok_or(format!("{flag} needs 0..100")))
                .transpose()
        };
        let mut s = Settings::default();
        if let Some(bg) = value("--bg").transpose()? {
            s.source = parse_source(&bg)?;
        }
        if let Some(dim) = percent("--dim")? {
            s.dim = dim;
        }
        if let Some(tint) = value("--tint").transpose()? {
            let (hex, strength) = tint.split_once('@').ok_or("--tint needs RRGGBB@0..100")?;
            let strength: f64 = strength.parse().ok().filter(|p| (0.0..=100.0).contains(p)).ok_or("--tint strength needs 0..100")?;
            s.tint = Some((parse_hex(hex)?, strength / 100.0));
        }
        if let Some(opacity) = percent("--tile-opacity")? {
            if opacity < MIN_TILE_OPACITY {
                eprintln!("background: --tile-opacity below {}% hurts legibility; using {}%", MIN_TILE_OPACITY * 100.0, MIN_TILE_OPACITY * 100.0);
            }
            s.tile_opacity = opacity.max(MIN_TILE_OPACITY);
        }
        if let Some(blur) = value("--blur").transpose()? {
            s.blur = blur.parse().ok().filter(|b: &f64| *b >= 0.0).ok_or("--blur needs a number of px")?;
        }
        if let Some(fit) = value("--fit").transpose()? {
            s.fit = match fit.as_str() {
                "cover" => Fit::Cover,
                "contain" => Fit::Contain,
                "tile" => Fit::Tile,
                _ => return Err("--fit needs cover, contain or tile".into()),
            };
        }
        s.auto_dim = !args.iter().any(|a| a == "--no-auto-dim");
        Ok(s)
    }
}

fn parse_source(text: &str) -> Result<Source, String> {
    match text.split_once(':') {
        None if text == "see-through" => Ok(Source::SeeThrough),
        Some(("image", path)) => Ok(Source::Image(expand_home(path))),
        Some(("solid", hex)) => Ok(Source::Solid(parse_hex(hex)?)),
        Some(("gradient", pair)) => {
            let (a, b) = pair.split_once(',').ok_or("gradient needs two colours: gradient:RRGGBB,RRGGBB")?;
            Ok(Source::Gradient(parse_hex(a)?, parse_hex(b)?))
        }
        _ => Err(format!("unknown background '{text}'; {USAGE}")),
    }
}

/// The shell leaves `~` alone after `image:`, so expand it here.
fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

fn parse_hex(text: &str) -> Result<Rgb, String> {
    let hex = text.trim_start_matches('#');
    u32::from_str_radix(hex, 16).ok().filter(|_| hex.len() == 6).map(rgb).ok_or(format!("'{text}' is not an RRGGBB colour"))
}

/// The baked background for one window size, plus what the legibility check found.
/// The background blurred for what sits on it: under the tiles, under the sidebar.
#[derive(Default)]
struct Backdrops {
    tiles: Option<ImageSurface>,
    frame: Option<ImageSurface>,
}

pub struct Background {
    pub settings: Settings,
    theme: Theme,
    image: Option<ImageSurface>,
    /// (width, height, background, backdrops) in device pixels.
    baked: Option<(i32, i32, ImageSurface, Backdrops)>,
    /// What the last bake found, for the background picker.
    report: Option<Report>,
}

/// The legibility check's outcome, in words for the picker.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    /// Worst text contrast on tiles, after any automatic dimming.
    pub contrast: f64,
    /// The dim asked for and the dim used (higher when auto-dim stepped in).
    pub dim: (f64, f64),
}

impl Report {
    /// (text, whether it is a warning).
    pub fn describe(&self) -> (String, bool) {
        if self.dim.1 > self.dim.0 + 0.001 {
            return (format!("Dimmed to {:.0}% so text stays readable ({:.1}:1).", self.dim.1 * 100.0, self.contrast), true);
        }
        if self.contrast < MIN_CONTRAST {
            return (format!("Text contrast is {:.1}:1, below {MIN_CONTRAST}:1. Raise Dim or Tiles.", self.contrast), true);
        }
        (format!("Text contrast {:.1}:1.", self.contrast), false)
    }
}

impl Background {
    /// `None` for see-through. An image source is decoded separately (`decode`), off the
    /// main thread in the app, so it never delays the first frame.
    pub fn new(settings: Settings, theme: Theme) -> Option<Background> {
        (settings.source != Source::SeeThrough).then_some(Background { settings, theme, image: None, baked: None, report: None })
    }

    pub fn report(&self) -> Option<&Report> {
        self.report.as_ref()
    }

    pub fn image_path(&self) -> Option<PathBuf> {
        match &self.settings.source {
            Source::Image(path) => Some(path.clone()),
            _ => None,
        }
    }

    pub fn set_image(&mut self, decoded: Decoded) -> Result<(), String> {
        self.image = Some(decoded.into_surface()?);
        self.baked = None;
        Ok(())
    }

    /// The background at this logical size, baked in device pixels (`scale`) on first
    /// use and after every resize or scale change.
    pub fn surface(&mut self, width: i32, height: i32, scale: f64) -> Option<&ImageSurface> {
        let (width, height) = ((width as f64 * scale).ceil() as i32, (height as f64 * scale).ceil() as i32);
        // Until the image is decoded the canvas stays see-through.
        if width <= 0 || height <= 0 || (self.image_path().is_some() && self.image.is_none()) {
            return None;
        }
        if !matches!(&self.baked, Some((w, h, _, _)) if *w == width && *h == height) {
            let started = std::time::Instant::now();
            match self.bake(width, height, scale) {
                Ok((surface, frosted)) => {
                    println!("background: baked {width}x{height} in {:.1} ms", started.elapsed().as_secs_f64() * 1000.0);
                    self.baked = Some((width, height, surface, frosted));
                }
                Err(e) => {
                    eprintln!("background: {e}");
                    return None;
                }
            }
        }
        self.baked.as_ref().map(|(_, _, s, _)| s)
    }

    /// The background blurred for under the tiles (glass themes also saturate it). Valid
    /// after `surface()` at the same size.
    pub fn frosted(&self) -> Option<&ImageSurface> {
        self.baked.as_ref().and_then(|(_, _, _, b)| b.tiles.as_ref())
    }

    /// The background blurred for under the sidebar.
    pub fn frosted_frame(&self) -> Option<&ImageSurface> {
        self.baked.as_ref().and_then(|(_, _, _, b)| b.frame.as_ref())
    }

    fn bake(&mut self, width: i32, height: i32, scale: f64) -> Result<(ImageSurface, Backdrops), cairo::Error> {
        let surface = ImageSurface::create(Format::Rgb24, width, height)?;
        {
            let cr = Context::new(&surface)?;
            self.paint_source(&cr, width as f64, height as f64)?;
        }
        let surface = blur(surface, self.settings.blur)?;
        let mut dim = self.settings.dim;
        if self.settings.auto_dim {
            let needed = required_dim(&luminances(&surface), &self.theme, self.settings.tile_opacity, self.settings.tint, dim);
            if needed > dim {
                eprintln!(
                    "background: text would fall below {MIN_CONTRAST}:1 on this background; dimming {:.0}% instead of {:.0}% (--no-auto-dim to keep it)",
                    needed * 100.0,
                    dim * 100.0
                );
                dim = needed;
            }
        }
        let cr = Context::new(&surface)?;
        if let Some((tint, strength)) = self.settings.tint {
            cr.set_source_rgba(tint.0, tint.1, tint.2, strength);
            cr.paint()?;
        }
        cr.set_source_rgba(0.0, 0.0, 0.0, dim);
        cr.paint()?;
        drop(cr);
        let report = legibility(&luminances(&surface), &self.theme, self.settings.tile_opacity);
        println!("background: worst contrast {:.1}:1 on tiles, {:.1}:1 on the empty-workspace hint (minimum {MIN_CONTRAST}:1)", report.tile, report.hint);
        self.report = Some(Report { contrast: report.tile.min(report.hint), dim: (self.settings.dim, dim) });
        let saturation = self.theme.glass.map_or(1.0, |g| g.saturate);
        let backdrop = |radius: f64, saturation: f64| -> Result<Option<ImageSurface>, cairo::Error> {
            if radius < 1.0 {
                return Ok(None);
            }
            let f = blur_with(copy(&surface)?, radius * scale, |small| if saturation == 1.0 { Ok(small) } else { saturate(small, saturation) })?;
            f.set_device_scale(scale, scale);
            Ok(Some(f))
        };
        let frosted = Backdrops { tiles: backdrop(self.settings.tile_blur, saturation)?, frame: backdrop(self.settings.frame_blur, 1.0)? };
        // Drawn in logical units from here on.
        surface.set_device_scale(scale, scale);
        Ok((surface, frosted))
    }

    fn paint_source(&self, cr: &Context, w: f64, h: f64) -> Result<(), cairo::Error> {
        match (&self.settings.source, &self.image) {
            (Source::Solid(c), _) => {
                cr.set_source_rgb(c.0, c.1, c.2);
                cr.paint()
            }
            (Source::Gradient(a, b), _) => {
                let gradient = cairo::LinearGradient::new(0.0, 0.0, 0.0, h);
                gradient.add_color_stop_rgb(0.0, a.0, a.1, a.2);
                gradient.add_color_stop_rgb(1.0, b.0, b.1, b.2);
                cr.set_source(&gradient)?;
                cr.paint()
            }
            (Source::Image(_), Some(image)) => paint_image(cr, image, w, h, self.settings.fit, &self.theme),
            _ => Ok(()),
        }
    }
}

/// Decoded pixels in cairo's ARGB32 layout; plain data, so it can cross threads.
pub struct Decoded {
    width: i32,
    height: i32,
    pixels: Vec<u8>,
}

impl Decoded {
    fn into_surface(self) -> Result<ImageSurface, String> {
        ImageSurface::create_for_data(self.pixels, Format::ARgb32, self.width, self.height, self.width * 4).map_err(|e| e.to_string())
    }
}

/// Safe to call on a worker thread: touches no cairo or GTK state.
pub fn decode(path: &std::path::Path) -> Result<Decoded, String> {
    let pixbuf = Pixbuf::from_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let pixbuf = pixbuf.add_alpha(false, 0, 0, 0).map_err(|e| e.to_string())?;
    let (w, h) = (pixbuf.width() as usize, pixbuf.height() as usize);
    let (src_stride, src) = (pixbuf.rowstride() as usize, pixbuf.read_pixel_bytes());
    let mut pixels = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let p = &src[y * src_stride + x * 4..][..4];
            let a = p[3] as u32;
            let pre = |c: u8| (c as u32 * a / 255) as u8;
            // Cairo's ARGB32 is premultiplied, native-endian 0xAARRGGBB.
            let px = u32::from_be_bytes([p[3], pre(p[0]), pre(p[1]), pre(p[2])]);
            pixels[(y * w + x) * 4..][..4].copy_from_slice(&px.to_ne_bytes());
        }
    }
    Ok(Decoded { width: w as i32, height: h as i32, pixels })
}

fn paint_image(cr: &Context, image: &ImageSurface, w: f64, h: f64, fit: Fit, theme: &Theme) -> Result<(), cairo::Error> {
    let (iw, ih) = (image.width() as f64, image.height() as f64);
    let pattern = SurfacePattern::create(image);
    pattern.set_filter(Filter::Good);
    if fit == Fit::Tile {
        pattern.set_extend(cairo::Extend::Repeat);
        cr.set_source(&pattern)?;
        return cr.paint();
    }
    // Contain letterboxes on the frame colour.
    let base = theme.surface;
    cr.set_source_rgb(base.0, base.1, base.2);
    cr.paint()?;
    let scale = match fit {
        Fit::Cover => (w / iw).max(h / ih),
        _ => (w / iw).min(h / ih),
    };
    cr.save()?;
    cr.translate((w - iw * scale) / 2.0, (h - ih * scale) / 2.0);
    cr.scale(scale, scale);
    cr.set_source(&pattern)?;
    cr.paint()?;
    cr.restore()
}

/// Cheap blur: shrink by the radius, then scale back up with bilinear filtering.
/// Good enough for a static background that is baked once.
fn blur(surface: ImageSurface, radius: f64) -> Result<ImageSurface, cairo::Error> {
    blur_with(surface, radius, Ok)
}

/// `blur` with a step on the shrunk copy. Saturation is linear in RGB, so applying it
/// there gives the same result at a fraction of the pixels.
fn blur_with(surface: ImageSurface, radius: f64, on_small: impl FnOnce(ImageSurface) -> Result<ImageSurface, cairo::Error>) -> Result<ImageSurface, cairo::Error> {
    if radius < 1.0 {
        return on_small(surface);
    }
    let (w, h) = (surface.width(), surface.height());
    let factor = radius.max(2.0);
    let (sw, sh) = (((w as f64 / factor).ceil() as i32).max(1), ((h as f64 / factor).ceil() as i32).max(1));
    let small = ImageSurface::create(Format::Rgb24, sw, sh)?;
    {
        let cr = Context::new(&small)?;
        cr.scale(sw as f64 / w as f64, sh as f64 / h as f64);
        let pattern = SurfacePattern::create(&surface);
        pattern.set_filter(Filter::Good);
        cr.set_source(&pattern)?;
        cr.paint()?;
    }
    let small = on_small(small)?;
    let out = ImageSurface::create(Format::Rgb24, w, h)?;
    {
        let cr = Context::new(&out)?;
        cr.scale(w as f64 / sw as f64, h as f64 / sh as f64);
        let pattern = SurfacePattern::create(&small);
        pattern.set_filter(Filter::Bilinear);
        pattern.set_extend(cairo::Extend::Pad);
        cr.set_source(&pattern)?;
        cr.set_operator(Operator::Source);
        cr.paint()?;
    }
    Ok(out)
}

fn copy(surface: &ImageSurface) -> Result<ImageSurface, cairo::Error> {
    let out = ImageSurface::create(Format::Rgb24, surface.width(), surface.height())?;
    let cr = Context::new(&out)?;
    cr.set_source_surface(surface, 0.0, 0.0)?;
    cr.set_operator(Operator::Source);
    cr.paint()?;
    drop(cr);
    Ok(out)
}

/// CSS `saturate()`: each channel moves away from (or toward) the pixel's luminance.
fn saturate(mut surface: ImageSurface, amount: f64) -> Result<ImageSurface, cairo::Error> {
    if (amount - 1.0).abs() < 1e-3 {
        return Ok(surface);
    }
    surface.flush();
    {
        let mut data = surface.data().map_err(|_| cairo::Error::InvalidStatus)?;
        for px in data.chunks_exact_mut(4) {
            let v = u32::from_ne_bytes(px[..4].try_into().unwrap());
            let (r, g, b) = (((v >> 16) & 0xFF) as f64, ((v >> 8) & 0xFF) as f64, (v & 0xFF) as f64);
            let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let c = |x: f64| (l + (x - l) * amount).round().clamp(0.0, 255.0) as u32;
            px.copy_from_slice(&((c(r) << 16) | (c(g) << 8) | c(b)).to_ne_bytes());
        }
    }
    surface.mark_dirty();
    Ok(surface)
}

/// Sampled pixels of a baked RGB24 surface (about 4k samples).
fn luminances(surface: &ImageSurface) -> Vec<Rgb> {
    let (w, h, stride) = (surface.width() as usize, surface.height() as usize, surface.stride() as usize);
    let step = ((w * h / 4096) as f64).sqrt().max(1.0) as usize;
    let mut out = Vec::new();
    // Reading needs a flushed surface; `data()` would need exclusive access, so copy.
    let mut bytes = vec![0u8; stride * h];
    surface.flush();
    let _ = surface.with_data(|d| bytes.copy_from_slice(d));
    for y in (0..h).step_by(step) {
        for x in (0..w).step_by(step) {
            let px = u32::from_ne_bytes(bytes[y * stride + x * 4..][..4].try_into().unwrap());
            out.push((((px >> 16) & 0xFF) as f64 / 255.0, ((px >> 8) & 0xFF) as f64 / 255.0, (px & 0xFF) as f64 / 255.0));
        }
    }
    out
}

pub struct Legibility {
    /// Code text on an editor tile.
    pub tile: f64,
    /// The empty-workspace hint on its card (no tile).
    pub hint: f64,
}

/// Worst contrast over the brightest 2% of the background is ignored (specks), so one
/// glint doesn't force a darker background.
pub fn legibility(pixels: &[Rgb], theme: &Theme, tile_opacity: f64) -> Legibility {
    let worst = |surface: Rgb, alpha: f64| {
        let mut c: Vec<f64> = pixels.iter().map(|&p| contrast(theme.text, over(surface, alpha, p))).collect();
        c.sort_by(f64::total_cmp);
        c.get(c.len() / 50).copied().unwrap_or(f64::INFINITY)
    };
    let (fill, alpha) = (theme.tile, tile_opacity);
    Legibility { tile: worst(fill, alpha), hint: worst(theme.surface, token::HINT_OPACITY) }
}

/// The smallest dim (5% steps, at least `dim`) at which both checks pass.
fn required_dim(pixels: &[Rgb], theme: &Theme, tile_opacity: f64, tint: Option<(Rgb, f64)>, dim: f64) -> f64 {
    let mut d = dim;
    while d < 1.0 {
        let shaded: Vec<Rgb> = pixels.iter().map(|&p| over((0.0, 0.0, 0.0), d, tint.map_or(p, |(t, s)| over(t, s, p)))).collect();
        let l = legibility(&shaded, theme, tile_opacity);
        if l.tile >= MIN_CONTRAST && l.hint >= MIN_CONTRAST {
            return d;
        }
        d = (d + 0.05).min(1.0);
    }
    1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn parses_flags() {
        let s = Settings::from_args(&args("fullscreen --bg gradient:112233,445566 --dim 30 --tint FF0000@10 --tile-opacity 95 --blur 8 --fit contain")).unwrap();
        assert_eq!(s.source, Source::Gradient(rgb(0x112233), rgb(0x445566)));
        assert!((s.dim - 0.3).abs() < 1e-9);
        assert_eq!(s.tint, Some((rgb(0xFF0000), 0.1)));
        assert!((s.tile_opacity - 0.95).abs() < 1e-9);
        assert_eq!((s.blur, s.fit), (8.0, Fit::Contain));
        assert_eq!(Settings::from_args(&args("fullscreen")).unwrap(), Settings::default());
    }

    #[test]
    fn image_path_expands_home() {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is set in tests"));
        let s = Settings::from_args(&args("--bg image:~/wall.png")).unwrap();
        assert_eq!(s.source, Source::Image(home.join("wall.png")));
        let s = Settings::from_args(&args("--bg image:/abs/wall.png")).unwrap();
        assert_eq!(s.source, Source::Image(PathBuf::from("/abs/wall.png")));
    }

    #[test]
    fn rejects_bad_values_and_floors_tile_opacity() {
        assert!(Settings::from_args(&args("--bg plaid")).is_err());
        assert!(Settings::from_args(&args("--bg solid:12345")).is_err());
        assert!(Settings::from_args(&args("--dim 150")).is_err());
        let s = Settings::from_args(&args("--tile-opacity 50")).unwrap();
        assert_eq!(s.tile_opacity, MIN_TILE_OPACITY);
    }

    #[test]
    fn contrast_matches_wcag() {
        assert!((contrast((1.0, 1.0, 1.0), (0.0, 0.0, 0.0)) - 21.0).abs() < 1e-9);
        assert!((contrast((0.5, 0.5, 0.5), (0.5, 0.5, 0.5)) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn white_background_stays_legible_without_dimming() {
        // With the window fill gone (2026-11-01), only tiles and the hint card carry text.
        let white = vec![(1.0, 1.0, 1.0); 100];
        let l = legibility(&white, &Theme::builtin(), MIN_TILE_OPACITY);
        assert!(l.tile >= MIN_CONTRAST && l.hint >= MIN_CONTRAST);
        assert_eq!(required_dim(&white, &Theme::builtin(), MIN_TILE_OPACITY, None, 0.0), 0.0);
        assert_eq!(required_dim(&vec![(0.1, 0.1, 0.1); 100], &Theme::builtin(), MIN_TILE_OPACITY, None, 0.2), 0.2, "keeps the user's dim when it passes");
    }

    #[test]
    fn glass_tiles_dim_a_bright_background() {
        let glass = Theme { glass: Some(tileide_core::theme::Glass::default()), text: rgb(0xF2F2F4), tile: rgb(0x2C2C30), ..Theme::builtin() };
        let white = vec![(1.0, 1.0, 1.0); 100];
        // The glass look: 56% opaque tiles over white need the background dimmed.
        assert!(legibility(&white, &glass, 0.56).tile < MIN_CONTRAST);
        let dim = required_dim(&white, &glass, 0.56, None, 0.0);
        assert!(dim > 0.0 && dim < 1.0, "{dim}");
    }

    #[test]
    fn saturate_keeps_grey_and_spreads_colour() {
        let s = ImageSurface::create(Format::Rgb24, 2, 1).unwrap();
        {
            let cr = Context::new(&s).unwrap();
            cr.set_source_rgb(0.5, 0.5, 0.5);
            cr.rectangle(0.0, 0.0, 1.0, 1.0);
            cr.fill().unwrap();
            cr.set_source_rgb(0.4, 0.5, 0.6);
            cr.rectangle(1.0, 0.0, 1.0, 1.0);
            cr.fill().unwrap();
        }
        let p = luminances(&saturate(s, 2.0).unwrap());
        assert!((p[0].0 - p[0].2).abs() < 0.01, "grey stays grey");
        assert!(p[1].2 - p[1].0 > 0.25, "blue-grey gets bluer: {:?}", p[1]);
    }
}
