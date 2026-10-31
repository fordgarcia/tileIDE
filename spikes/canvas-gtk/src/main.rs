//! v0.0 spikes 1, 3, 4 (R1, GTK4): the TileIDE canvas on Hyprland, the rail ⇄ drawer,
//! and real tiles.
//!
//!   tileide-canvas-spike <windowed|fullscreen|layer> [--solid] [--push] [--real-tiles] [--seconds N] [--bench-drawer N]
//!
//! windowed        a normal toplevel; the WM tiles it like any other app (spec §4.5)
//! fullscreen      a fullscreen toplevel covering the monitor
//! layer           a layer-shell surface on the Top layer (rejected in spike 1; kept for comparison)
//! --solid         opaque fallback colours (no compositor blur needed)
//! --push          the drawer pushes the window (default: overlays it); Q3
//! --real-tiles    GtkSourceView editor + VTE terminal tiles instead of drawn placeholders
//! --bench-drawer  toggle the drawer N times, print frame statistics, quit
//!   tileide-canvas-spike render <out.png> <drawer 0..1> [--push]
//!                 draw one frame offscreen (1280x720 on a checkerboard) to inspect shapes
//! Keys (placeholder tiles): b or a click on the launcher toggles the drawer; Esc collapses it,
//! or quits when collapsed; q quits.
//! Keys (--real-tiles, TileIDE modifier = Alt, spec §4.5): Alt+Return terminal, Alt+E editor,
//! Alt+W close tile, Alt+H/J/K/L focus, Alt+Shift+H/J/K/L resize, Alt+B drawer, Alt+Escape quit.

mod draw;
mod tile_view;
mod tiles;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4 as gtk;
use gtk4::cairo;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Windowed,
    Fullscreen,
    Layer,
}

#[derive(Clone, Copy)]
struct Options {
    mode: Mode,
    solid: bool,
    push: bool,
    real_tiles: bool,
    seconds: Option<u32>,
    bench_toggles: Option<u32>,
}

/// design.md §8: drawer expand/collapse 180–220 ms, ease-out, interruptible.
const DRAWER_DURATION_US: f64 = 200_000.0;

/// Drawer animation, driven by the frame clock. `progress` is linear time; drawing eases it.
#[derive(Default)]
struct Drawer {
    progress: f64,
    from: f64,
    target: f64,
    start_us: Option<i64>,
}

impl Drawer {
    fn toggle(&mut self) {
        self.set_target(if self.target > 0.5 { 0.0 } else { 1.0 });
    }

    /// Restarts from the current position, so reversing mid-animation never jumps.
    fn set_target(&mut self, target: f64) {
        self.from = self.progress;
        self.target = target;
        self.start_us = None;
    }

    fn animating(&self) -> bool {
        self.progress != self.target
    }

    fn tick(&mut self, now_us: i64) {
        let start = *self.start_us.get_or_insert(now_us);
        // A partial reverse takes proportionally less time.
        let span = (self.target - self.from).abs().max(f64::EPSILON);
        let t = ((now_us - start) as f64 / (DRAWER_DURATION_US * span)).min(1.0);
        self.progress = self.from + (self.target - self.from) * t;
    }

    fn eased(&self) -> f64 {
        1.0 - (1.0 - self.progress).powi(3)
    }
}

/// Frame-to-frame intervals and draw cost while the drawer animates.
#[derive(Default)]
struct FrameStats {
    last_frame_us: Option<i64>,
    intervals_us: Vec<i64>,
    draw_times: Vec<Duration>,
}

fn parse_args() -> Result<Options, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = match args.first().map(String::as_str) {
        Some("windowed") => Mode::Windowed,
        Some("fullscreen") => Mode::Fullscreen,
        Some("layer") => Mode::Layer,
        _ => return Err("usage: tileide-canvas-spike <windowed|fullscreen|layer> [--solid] [--push] [--seconds N] [--bench-drawer N]".into()),
    };
    let number = |flag: &str| -> Result<Option<u32>, String> {
        match args.iter().position(|a| a == flag) {
            Some(i) => Ok(Some(args.get(i + 1).and_then(|n| n.parse().ok()).ok_or(format!("{flag} needs a number"))?)),
            None => Ok(None),
        }
    };
    Ok(Options {
        mode,
        solid: args.iter().any(|a| a == "--solid"),
        push: args.iter().any(|a| a == "--push"),
        real_tiles: args.iter().any(|a| a == "--real-tiles"),
        seconds: number("--seconds")?,
        bench_toggles: number("--bench-drawer")?,
    })
}

fn main() -> glib::ExitCode {
    let started = Instant::now();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("render") {
        return match render(&args[1..]) {
            Ok(()) => glib::ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("{message}");
                glib::ExitCode::FAILURE
            }
        };
    }
    let options = match parse_args() {
        Ok(o) => o,
        Err(message) => {
            eprintln!("{message}");
            return glib::ExitCode::FAILURE;
        }
    };
    let app = gtk::Application::builder().application_id("dev.tileide.spike.canvas").build();
    app.connect_activate(move |app| build(app, options, started));
    // Our flags are not GTK's: hand GTK only the program name.
    app.run_with_args(&["tileide-canvas-spike"])
}

fn build(app: &gtk::Application, options: Options, started: Instant) {
    let window = gtk::ApplicationWindow::new(app);
    window.set_title(Some("TileIDE canvas spike"));
    window.set_decorated(false);

    let css = gtk::CssProvider::new();
    css.load_from_string(&format!("window, window.background {{ background: transparent; }}\n{}", tile_view::CSS));
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("a display"),
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    match options.mode {
        Mode::Windowed => window.set_default_size(1100, 700),
        Mode::Fullscreen => window.fullscreen(),
        Mode::Layer => {
            window.init_layer_shell();
            window.set_namespace(Some("tileide"));
            window.set_layer(Layer::Top);
            for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
                window.set_anchor(edge, true);
            }
            window.set_exclusive_zone(-1);
            window.set_keyboard_mode(KeyboardMode::Exclusive);
        }
    }

    let drawer = Rc::new(RefCell::new(Drawer::default()));
    let stats = Rc::new(RefCell::new(FrameStats::default()));
    let area = gtk::DrawingArea::new();

    let first_frame = Rc::new(Cell::new(true));
    {
        let (drawer, stats) = (drawer.clone(), stats.clone());
        area.set_draw_func(move |_, cr, width, height| {
            let begin = Instant::now();
            let d = drawer.borrow();
            let scene = draw::Scene {
                width: width as f64,
                height: height as f64,
                solid: options.solid,
                drawer: d.eased(),
                push: options.push,
                placeholder_tiles: !options.real_tiles,
            };
            if let Err(e) = draw::draw(cr, &scene) {
                eprintln!("draw failed: {e}");
            }
            if d.animating() {
                stats.borrow_mut().draw_times.push(begin.elapsed());
            }
            if first_frame.replace(false) {
                println!(
                    "first frame: {:.1} ms after start, surface {width}x{height} logical px, RSS {}",
                    started.elapsed().as_secs_f64() * 1000.0,
                    rss().unwrap_or_else(|| "?".into())
                );
            }
        });
    }

    let tiles = options.real_tiles.then(|| {
        let view = tile_view::TileView::new();
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut v = view.borrow_mut();
        v.add(tile_view::Kind::Editor(&src.join("src/tiles.rs")), None);
        v.add(tile_view::Kind::Editor(&src.join("../lua-config/examples/config/tileide.lua")), Some(tiles::Axis::Row));
        v.add(tile_view::Kind::Terminal, Some(tiles::Axis::Column));
        drop(v);
        view
    });
    // Keeps the tile widgets inside the (possibly pushed) window.
    let sync_tiles = {
        let (tiles, area) = (tiles.clone(), area.clone());
        move |eased: f64| {
            let Some(tiles) = &tiles else { return };
            let scene = draw::Scene {
                width: area.width() as f64,
                height: area.height() as f64,
                solid: options.solid,
                drawer: eased,
                push: options.push,
                placeholder_tiles: false,
            };
            let (x, y, w, h) = draw::window_rect(&scene);
            let inset = draw::token::GAP_TILE_OUT;
            tiles.borrow_mut().set_area(tiles::Rect { x: x + inset, y: y + inset, w: w - 2.0 * inset, h: h - 2.0 * inset });
        }
    };
    {
        let (drawer, sync_tiles) = (drawer.clone(), sync_tiles.clone());
        area.connect_resize(move |_, _, _| sync_tiles(drawer.borrow().eased()));
    }

    // Frame clock drives the animation; it records intervals only while moving.
    {
        let (drawer, stats, sync_tiles) = (drawer.clone(), stats.clone(), sync_tiles.clone());
        area.add_tick_callback(move |area, clock| {
            let now = clock.frame_time();
            let mut s = stats.borrow_mut();
            let eased = {
                let mut d = drawer.borrow_mut();
                if !d.animating() {
                    s.last_frame_us = None;
                    return glib::ControlFlow::Continue;
                }
                d.tick(now);
                d.eased()
            };
            if let Some(last) = s.last_frame_us {
                s.intervals_us.push(now - last);
            }
            s.last_frame_us = Some(now);
            sync_tiles(eased);
            area.queue_draw();
            glib::ControlFlow::Continue
        });
    }

    let toggle = {
        let drawer = drawer.clone();
        let area = area.clone();
        move || {
            drawer.borrow_mut().toggle();
            area.queue_draw();
        }
    };

    // On the window (capture phase), because tile widgets may sit above the drawing area.
    let click = gtk::GestureClick::new();
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let toggle = toggle.clone();
        click.connect_pressed(move |_, _, x, y| {
            let (dx, dy) = (x - draw::token::RAIL_WIDTH / 2.0, y - draw::LAUNCHER_Y);
            if dx.hypot(dy) <= draw::LAUNCHER_RADIUS + 6.0 {
                toggle();
            }
        });
    }
    window.add_controller(click);
    match &tiles {
        Some(tiles) => {
            let overlay = gtk::Overlay::new();
            overlay.set_child(Some(&area));
            overlay.add_overlay(tiles.borrow().widget());
            window.set_child(Some(&overlay));
            let tiles = tiles.clone();
            window.connect_show(move |_| {
                let tiles = tiles.clone();
                glib::idle_add_local_once(move || tiles.borrow().grab_focused());
            });
        }
        None => window.set_child(Some(&area)),
    }

    // Capture phase: TileIDE's bindings win over the focused editor or terminal.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let (app, drawer, toggle) = (app.clone(), drawer.clone(), toggle.clone());
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(tiles) = &tiles else {
                match key {
                    gdk::Key::b => toggle(),
                    gdk::Key::Escape if drawer.borrow().target > 0.5 => toggle(),
                    gdk::Key::Escape | gdk::Key::q => app.quit(),
                    _ => {}
                }
                return glib::Propagation::Proceed;
            };
            if !modifiers.contains(gdk::ModifierType::ALT_MASK) {
                return glib::Propagation::Proceed;
            }
            let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
            let direction = match key.to_lower() {
                gdk::Key::h => Some(tiles::Direction::Left),
                gdk::Key::j => Some(tiles::Direction::Down),
                gdk::Key::k => Some(tiles::Direction::Up),
                gdk::Key::l => Some(tiles::Direction::Right),
                _ => None,
            };
            let mut view = tiles.borrow_mut();
            match (key.to_lower(), direction) {
                (_, Some(d)) if shift => view.resize(d),
                (_, Some(d)) => view.focus_direction(d),
                (gdk::Key::Return, _) => view.add(tile_view::Kind::Terminal, None),
                (gdk::Key::e, _) => view.add(tile_view::Kind::Editor(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"))), None),
                (gdk::Key::w, _) => view.close_focused(),
                (gdk::Key::b, _) => toggle(),
                (gdk::Key::Escape, _) => app.quit(),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
    }
    window.add_controller(keys);

    if let Some(seconds) = options.seconds {
        let app = app.clone();
        glib::timeout_add_seconds_local_once(seconds, move || app.quit());
    }
    if let Some(toggles) = options.bench_toggles {
        start_bench(app, &window, toggles, toggle, stats);
    }
    window.present();
}

/// Toggles every 350 ms (each animation is 200 ms), then prints statistics and quits.
fn start_bench(app: &gtk::Application, window: &gtk::ApplicationWindow, toggles: u32, toggle: impl Fn() + 'static, stats: Rc<RefCell<FrameStats>>) {
    let remaining = Cell::new(toggles);
    let (app, window) = (app.clone(), window.clone());
    // Give the compositor time to map and size the surface first.
    glib::timeout_add_local(Duration::from_millis(350), move || {
        if remaining.get() == 0 {
            print_stats(&window, &stats.borrow());
            app.quit();
            return glib::ControlFlow::Break;
        }
        remaining.set(remaining.get() - 1);
        toggle();
        glib::ControlFlow::Continue
    });
}

fn print_stats(window: &gtk::ApplicationWindow, s: &FrameStats) {
    let refresh_mhz = window
        .surface()
        .and_then(|surface| WidgetExt::display(window).monitor_at_surface(&surface))
        .map(|m| m.refresh_rate())
        .filter(|r| *r > 0)
        .unwrap_or(60_000);
    let budget_us = 1_000_000_000 / refresh_mhz as i64;
    let mut intervals = s.intervals_us.clone();
    intervals.sort_unstable();
    let mut draws = s.draw_times.clone();
    draws.sort_unstable();
    if intervals.is_empty() || draws.is_empty() {
        println!("no animation frames recorded");
        return;
    }
    let ms = |us: i64| us as f64 / 1000.0;
    let dropped = intervals.iter().filter(|&&i| i as f64 > budget_us as f64 * 1.5).count();
    println!(
        "drawer animation @ {:.0} Hz (frame budget {:.2} ms): {} frames, interval median {:.2} ms, p95 {:.2} ms, max {:.2} ms, dropped (>1.5x budget) {} ({:.1}%)",
        refresh_mhz as f64 / 1000.0,
        ms(budget_us),
        intervals.len(),
        ms(intervals[intervals.len() / 2]),
        ms(intervals[intervals.len() * 95 / 100]),
        ms(*intervals.last().unwrap()),
        dropped,
        dropped as f64 * 100.0 / intervals.len() as f64,
    );
    println!(
        "draw callback (CPU, cairo path building): median {:.2} ms, max {:.2} ms",
        draws[draws.len() / 2].as_secs_f64() * 1000.0,
        draws.last().unwrap().as_secs_f64() * 1000.0
    );
}

/// Offscreen frame at 2x scale over a light checkerboard, so translucency and curves are visible.
fn render(args: &[String]) -> Result<(), String> {
    let usage = "usage: tileide-canvas-spike render <out.png> <drawer 0..1> [--push]";
    let out = args.first().ok_or(usage)?;
    let drawer: f64 = args.get(1).and_then(|p| p.parse().ok()).ok_or(usage)?;
    let (w, h, scale) = (1280.0, 720.0, 2.0);
    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, (w * scale) as i32, (h * scale) as i32).map_err(|e| e.to_string())?;
    let cr = cairo::Context::new(&surface).map_err(|e| e.to_string())?;
    cr.scale(scale, scale);
    let scene = draw::Scene { width: w, height: h, solid: false, drawer: drawer.clamp(0.0, 1.0), push: args.iter().any(|a| a == "--push"), placeholder_tiles: true };
    cr.push_group();
    draw::draw(&cr, &scene).map_err(|e| e.to_string())?;
    let frame = cr.pop_group().map_err(|e| e.to_string())?;
    for y in 0..(h / 20.0) as i32 {
        for x in 0..(w / 20.0) as i32 {
            let shade = if (x + y) % 2 == 0 { 0.85 } else { 0.95 };
            cr.set_source_rgb(shade, shade, shade);
            cr.rectangle(x as f64 * 20.0, y as f64 * 20.0, 20.0, 20.0);
            cr.fill().map_err(|e| e.to_string())?;
        }
    }
    cr.set_source(&frame).map_err(|e| e.to_string())?;
    cr.paint().map_err(|e| e.to_string())?;
    drop(cr);
    let mut file = std::fs::File::create(out).map_err(|e| format!("{out}: {e}"))?;
    surface.write_to_png(&mut file).map_err(|e| e.to_string())
}

fn rss() -> Option<String> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    Some(line.trim_start_matches("VmRSS:").trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drawer_reaches_target_after_duration() {
        let mut d = Drawer::default();
        d.toggle();
        d.tick(0);
        d.tick(DRAWER_DURATION_US as i64);
        assert_eq!(d.progress, 1.0);
        assert!(!d.animating());
    }

    #[test]
    fn reversing_mid_animation_is_continuous_and_shorter() {
        let mut d = Drawer::default();
        d.toggle();
        d.tick(0);
        d.tick(100_000);
        let at_reverse = d.progress;
        assert!((at_reverse - 0.5).abs() < 1e-9);
        d.toggle();
        d.tick(100_000);
        assert_eq!(d.progress, at_reverse, "no jump when reversing");
        d.tick(200_000);
        assert_eq!(d.progress, 0.0, "half the distance takes half the time");
    }
}
