// SPDX-License-Identifier: GPL-3.0-or-later
//! TileIDE v0.0 preview: the Caelestia-style canvas (rail + frame + drawer), workspaces,
//! a Files drawer, and GtkSourceView editor / VTE terminal tiles in a tiling layout.
//! Grown from the v0.0 spikes; v0.1 is a clean rebuild. `tileide --help` lists options.
//!
//! Mouse: rail launcher = Files drawer of the active workspace, workspace dot = peek at
//!   that workspace's files (click the dot again or the panel's header to switch; the
//!   peek closes when the pointer leaves it), + = new workspace, gear = Config workspace,
//!   drag the drawer's right edge to resize it, click a tile to focus it.

mod background;
mod bench;
mod cli;
mod draw;
mod drawer;
mod file_tree;
mod tile_view;
mod tiles;
mod workspaces;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4 as gtk;

use bench::FrameStats;
use cli::{Mode, Options};
use drawer::Drawer;
use tile_view::Kind;
use workspaces::{Active, Workspaces};

const APP_ID: &str = "dev.tileide.TileIDE";

/// Grab distance around the drawer's right edge for mouse resizing.
const RESIZE_GRAB: f64 = 6.0;
const PEEK_WIDTH: i32 = 260;
const PEEK_HEIGHT: i32 = 360;
/// Space between the rail's right edge and the peek panel; the pointer may cross it.
const PEEK_GAP: f64 = 6.0;

/// The panel a workspace dot opens: that workspace's file tree, without switching to it.
struct Peek {
    widget: gtk::Box,
    go: gtk::Button,
    tree: Rc<RefCell<file_tree::FileTree>>,
    /// The workspace shown, and its dot's centre y; `None` while hidden.
    target: Rc<Cell<Option<(u8, f64)>>>,
}

struct App {
    options: Options,
    window: gtk::ApplicationWindow,
    area: gtk::DrawingArea,
    drawer: RefCell<Drawer>,
    /// The Files section, laid over the open drawer.
    panel: gtk::Box,
    /// Owned here: the tree's own callbacks only hold weak references.
    _files: Rc<RefCell<file_tree::FileTree>>,
    peek: Peek,
    workspaces: Rc<RefCell<Workspaces>>,
    clock: RefCell<(String, String)>,
    /// Full-screen only (spec §4.4); `None` = see-through.
    background: RefCell<Option<background::Background>>,
    folder_name: String,
    stats: RefCell<FrameStats>,
}

fn main() -> glib::ExitCode {
    let started = Instant::now();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let options = match cli::parse(&args, &cwd) {
        Ok(cli::Command::Run(options)) => *options,
        Ok(cli::Command::Help) => {
            println!("{}", cli::HELP);
            return glib::ExitCode::SUCCESS;
        }
        Ok(cli::Command::Version) => {
            println!("tileide {}", env!("CARGO_PKG_VERSION"));
            return glib::ExitCode::SUCCESS;
        }
        Ok(cli::Command::Render(rest)) => return report(bench::render(&rest)),
        Err(message) => return report(Err(message)),
    };
    // Like `code .`: hand the shell back at once. Benchmarks stay attached to report.
    if !options.foreground && options.seconds.is_none() && options.bench_toggles.is_none() {
        return report(detach(&args));
    }
    // Spikes 1/3/4: GTK's default Vulkan renderer costs ~900 ms at startup; cairo is
    // fastest, smoothest and lightest for this UI. An explicit GSK_RENDERER still wins.
    if std::env::var_os("GSK_RENDERER").is_none() {
        // SAFETY: no other threads exist yet; GTK is not initialised.
        unsafe { std::env::set_var("GSK_RENDERER", "cairo") };
    }
    // Terminals start in the opened folder.
    if let Err(e) = std::env::set_current_dir(&options.folder) {
        eprintln!("{}: {e}", options.folder.display());
    }
    // Non-unique: every `tileide` call opens its own window, like `code -n`.
    let app = gtk::Application::builder().application_id(APP_ID).flags(gtk::gio::ApplicationFlags::NON_UNIQUE).build();
    app.connect_activate(move |app| build(app, options.clone(), started));
    // Our flags are not GTK's: hand GTK only the program name.
    app.run_with_args(&["tileide"])
}

fn report(result: Result<(), String>) -> glib::ExitCode {
    match result {
        Ok(()) => glib::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("tileide: {message}");
            glib::ExitCode::FAILURE
        }
    }
}

/// Re-runs this binary in its own session with no terminal attached, then returns.
/// Arguments were already validated, so errors were reported before detaching.
fn detach(args: &[String]) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Command::new(exe)
        .args(args)
        .arg("--foreground")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map(drop)
        .map_err(|e| format!("cannot start: {e}"))
}

/// `$XDG_CONFIG_HOME/tileide/tileide.lua` (default `~/.config`), seeded on first run
/// from the packaged sample (`/usr/share/tileide/config`) or the source tree's.
fn config_path() -> PathBuf {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let samples = [PathBuf::from("/usr/share/tileide/config"), Path::new(env!("CARGO_MANIFEST_DIR")).join("config")];
    cli::config_path(&config_home, &samples)
}

fn now_clock() -> (String, String) {
    let format = |f: &str| glib::DateTime::now_local().and_then(|d| d.format(f)).map(|s| s.to_string()).unwrap_or_default();
    (format("%H"), format("%M"))
}

fn build(app: &gtk::Application, options: Options, started: Instant) {
    let fullscreen = options.mode == Mode::Fullscreen;
    if !fullscreen && options.background.source != background::Source::SeeThrough {
        eprintln!("background: ignored outside full-screen mode (spec §4.4)");
    }
    let backdrop = if fullscreen { background::Background::new(options.background.clone()) } else { None };
    let tile_opacity = if fullscreen { options.background.tile_opacity } else { draw::token::TILE_OPACITY };
    let window = gtk::ApplicationWindow::new(app);
    window.set_title(Some(&format!("TileIDE · {}", options.folder.file_name().map_or_else(|| "/".into(), |n| n.to_string_lossy()))));
    window.set_decorated(false);

    let css = gtk::CssProvider::new();
    css.load_from_string(&format!(
        "window, window.background {{ background: transparent; font-family: \"{}\"; }}\n{}\n{}",
        draw::token::UI_FONT,
        tile_view::css(tile_opacity),
        file_tree::CSS
    ));
    gtk::style_context_add_provider_for_display(&gdk::Display::default().expect("a display"), &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

    match options.mode {
        Mode::Windowed => window.set_default_size(1100, 700),
        Mode::Fullscreen => window.fullscreen(),
    }

    let area = gtk::DrawingArea::new();
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&area));
    window.set_child(Some(&overlay));

    let on_change: Rc<dyn Fn()> = {
        let area = area.clone();
        Rc::new(move || area.queue_draw())
    };
    // Every workspace's tiles live in this one layer, added before the drawer panel and
    // the peek: a workspace created later must not stack above them and take their clicks.
    let tile_layer = gtk::Overlay::new();
    overlay.add_overlay(&tile_layer);
    let workspaces = Rc::new(RefCell::new(Workspaces::new(tile_layer, config_path(), options.folder.clone(), on_change)));

    let open_file: Rc<dyn Fn(&Path)> = {
        let workspaces = workspaces.clone();
        Rc::new(move |path: &Path| {
            let view = workspaces.borrow().current();
            view.borrow_mut().add(Kind::Editor(Some(path)), None);
        })
    };
    let tree = file_tree::FileTree::new(options.folder.clone(), open_file);
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    panel.set_halign(gtk::Align::Start);
    panel.set_valign(gtk::Align::Fill);
    panel.set_margin_start(draw::token::RAIL_WIDTH as i32);
    panel.set_margin_top(draw::token::FRAME_BORDER as i32);
    panel.set_margin_bottom(draw::token::FRAME_BORDER as i32);
    panel.set_overflow(gtk::Overflow::Hidden);
    panel.set_visible(false);
    let tree_widget = tree.borrow().widget().clone();
    tree_widget.set_vexpand(true);
    panel.append(&tree_widget);
    overlay.add_overlay(&panel);
    {
        let tree = Rc::downgrade(&tree);
        workspaces.borrow_mut().set_on_switch(Rc::new(move |root: &Path| {
            if let Some(tree) = tree.upgrade() {
                tree.borrow_mut().set_root(root);
            }
        }));
    }
    let peek = build_peek(&overlay, &workspaces);

    let folder_name = options.folder.file_name().map_or_else(|| "/".into(), |n| n.to_string_lossy().into_owned());
    let app_state = Rc::new(App {
        options: options.clone(),
        window: window.clone(),
        area: area.clone(),
        drawer: RefCell::new(Drawer::default()),
        panel,
        _files: tree,
        peek,
        workspaces,
        clock: RefCell::new(now_clock()),
        background: RefCell::new(backdrop),
        folder_name,
        stats: RefCell::new(FrameStats::default()),
    });

    if let Some(file) = &options.open {
        let view = app_state.workspaces.borrow().current();
        view.borrow_mut().add(Kind::Editor(Some(file)), None);
    }

    load_background(&app_state);
    connect_drawing(&app_state, started);
    connect_mouse(&app_state);
    connect_keys(&app_state, app);

    {
        let state = app_state.clone();
        // Checked every second so the minute flips on time; redrawn only when it changes.
        glib::timeout_add_seconds_local(1, move || {
            let now = now_clock();
            if *state.clock.borrow() != now {
                *state.clock.borrow_mut() = now;
                state.area.queue_draw();
            }
            glib::ControlFlow::Continue
        });
    }
    {
        let state = app_state.clone();
        window.connect_show(move |_| {
            let state = state.clone();
            glib::idle_add_local_once(move || state.workspaces.borrow().current().borrow().grab_focused());
        });
    }
    if let Some(seconds) = options.seconds {
        let app = app.clone();
        glib::timeout_add_seconds_local_once(seconds, move || app.quit());
    }
    if let Some(toggles) = options.bench_toggles {
        start_bench(app, &app_state, toggles);
    }
    window.present();
}

impl App {
    fn scene<'a>(&self, numbers: &'a [u8], label: &'a str, clock: &'a (String, String)) -> draw::Scene<'a> {
        let workspaces = self.workspaces.borrow();
        draw::Scene {
            width: self.area.width() as f64,
            height: self.area.height() as f64,
            solid: self.options.solid,
            drawer: self.drawer.borrow().extension(),
            push: self.options.push,
            workspaces: numbers,
            active: match workspaces.active() {
                Active::Number(n) => Some(n),
                Active::Config => None,
            },
            label,
            clock: (&clock.0, &clock.1),
            empty: workspaces.current().borrow().is_empty(),
            placeholder_tiles: false,
            background: None,
            tile_opacity: draw::token::TILE_OPACITY,
        }
    }

    fn label(&self) -> String {
        match self.workspaces.borrow().active() {
            Active::Number(n) => format!("{} · {}", self.folder_name, workspaces::display(n)),
            Active::Config => "config".into(),
        }
    }

    /// Places tiles and the Files panel for the current drawer extension and window size.
    fn sync(&self) {
        let numbers = self.workspaces.borrow().numbers();
        let clock = self.clock.borrow();
        let scene = self.scene(&numbers, "", &clock);
        let (x, y, w, h) = draw::window_rect(&scene);
        let inset = draw::token::GAP_TILE_OUT;
        self.workspaces.borrow_mut().set_area(tiles::Rect { x: x + inset, y: y + inset, w: w - 2.0 * inset, h: h - 2.0 * inset });

        // The panel appears once the drawer is wide enough for its content (it fades
        // in during the last part of the expansion, design.md §8).
        let drawer = self.drawer.borrow();
        let extension = drawer.extension();
        let shown = extension >= draw::token::DRAWER_MIN_WIDTH * 0.9;
        self.panel.set_visible(shown);
        if shown {
            self.panel.set_size_request(extension as i32, -1);
            self.panel.set_opacity(((extension / drawer.width - 0.6) / 0.4).clamp(0.0, 1.0));
        }
    }

    fn toggle_drawer(&self) {
        self.drawer.borrow_mut().toggle();
        self.area.queue_draw();
    }

    /// Shows workspace `n`'s files next to its dot (a click on the dot).
    fn show_peek(&self, n: u8, dot_y: f64) {
        let peek = &self.peek;
        peek.tree.borrow_mut().set_root(&self.workspaces.borrow().root(Active::Number(n)));
        peek.go.set_label(&format!("Workspace {}  →", workspaces::display(n)));
        let max_top = (self.area.height() - PEEK_HEIGHT - draw::token::FRAME_BORDER as i32).max(0);
        peek.widget.set_margin_top(((dot_y - 20.0) as i32).clamp(0, max_top));
        peek.widget.set_visible(true);
        peek.target.set(Some((n, dot_y)));
    }

    /// Closes the peek once the pointer is neither on its dot nor over the panel.
    fn track_peek(&self, x: f64, y: f64) {
        let Some((_, dot_y)) = self.peek.target.get() else { return };
        let on_dot = x <= draw::token::RAIL_WIDTH + PEEK_GAP && (y - dot_y).abs() <= 14.0;
        let on_panel = self.peek.widget.compute_bounds(&self.window).is_some_and(|b| b.contains_point(&gtk::graphene::Point::new(x as f32, y as f32)));
        if !on_dot && !on_panel {
            hide_peek(&self.peek.widget, &self.peek.target);
        }
    }

    /// Prints how long after `started` the next frame finished painting (spec §9 budget).
    fn report_after_paint(&self, started: Instant, what: &'static str) {
        let Some(clock) = self.area.frame_clock() else { return };
        let handler: Rc<RefCell<Option<glib::SignalHandlerId>>> = Rc::default();
        let h = handler.clone();
        let id = clock.connect_after_paint(move |clock| {
            println!("{what}: {:.1} ms to painted frame", started.elapsed().as_secs_f64() * 1000.0);
            if let Some(id) = h.borrow_mut().take() {
                clock.disconnect(id);
            }
        });
        *handler.borrow_mut() = Some(id);
    }

    fn near_resize_edge(&self, x: f64) -> bool {
        let drawer = self.drawer.borrow();
        drawer.is_open() && (x - (draw::token::RAIL_WIDTH + drawer.width)).abs() <= RESIZE_GRAB
    }
}

/// Decodes an image background on a worker thread, then redraws with it.
fn load_background(state: &Rc<App>) {
    let Some(path) = state.background.borrow().as_ref().and_then(background::Background::image_path) else { return };
    let s = state.clone();
    glib::spawn_future_local(async move {
        let started = Instant::now();
        let decoded = gtk::gio::spawn_blocking(move || background::decode(&path)).await;
        let result = match decoded {
            Ok(Ok(decoded)) => s.background.borrow_mut().as_mut().map_or(Ok(()), |b| b.set_image(decoded)),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("decoder thread panicked".into()),
        };
        match result {
            Ok(()) => println!("background: decoded in {:.1} ms (off the main thread)", started.elapsed().as_secs_f64() * 1000.0),
            Err(e) => {
                eprintln!("background: {e}; staying see-through");
                *s.background.borrow_mut() = None;
            }
        }
        s.area.queue_draw();
    });
}

fn connect_drawing(state: &Rc<App>, started: Instant) {
    let first_frame = Cell::new(true);
    let s = state.clone();
    state.area.set_draw_func(move |area, cr, width, height| {
        let begin = Instant::now();
        // Cheap clone (refcounted); baked only when the size changes.
        // The fractional scale (e.g. 1.5), so the paint is a 1:1 copy, not a resample.
        let scale = area.native().and_then(|n| n.surface()).map_or(area.scale_factor() as f64, |s| s.scale());
        let background = s.background.borrow_mut().as_mut().and_then(|b| b.surface(width, height, scale).cloned());
        let numbers = s.workspaces.borrow().numbers();
        let label = s.label();
        let clock = s.clock.borrow();
        let mut scene = s.scene(&numbers, &label, &clock);
        scene.background = background.as_ref();
        if let Err(e) = draw::draw(cr, &scene) {
            eprintln!("draw failed: {e}");
        }
        if s.drawer.borrow().animating() {
            s.stats.borrow_mut().draw_times.push(begin.elapsed());
        }
        if first_frame.replace(false) {
            println!(
                "first frame: {:.1} ms after start, surface {width}x{height} logical px, RSS {}",
                started.elapsed().as_secs_f64() * 1000.0,
                bench::rss().unwrap_or_else(|| "?".into())
            );
        }
    });

    let s = state.clone();
    state.area.connect_resize(move |_, _, _| s.sync());

    // Frame clock drives the drawer animation; intervals are recorded only while it moves.
    let s = state.clone();
    state.area.add_tick_callback(move |area, clock| {
        let now = clock.frame_time();
        {
            let mut d = s.drawer.borrow_mut();
            let mut stats = s.stats.borrow_mut();
            if !d.animating() {
                stats.last_frame_us = None;
                return glib::ControlFlow::Continue;
            }
            d.tick(now);
            if let Some(last) = stats.last_frame_us {
                stats.intervals_us.push(now - last);
            }
            stats.last_frame_us = Some(now);
        }
        s.sync();
        area.queue_draw();
        glib::ControlFlow::Continue
    });
}

fn connect_mouse(state: &Rc<App>) {
    // On the window, capture phase: tile and panel widgets sit above the drawing area.
    let click = gtk::GestureClick::new();
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let s = state.clone();
    click.connect_pressed(move |_, _, x, y| {
        let numbers = s.workspaces.borrow().numbers();
        let clock = s.clock.borrow().clone();
        let hit = draw::rail_hit(&s.scene(&numbers, "", &clock), x, y);
        match hit {
            Some(draw::RailHit::Launcher) => s.toggle_drawer(),
            // A second click on the peeked dot switches, like a nav-bar entry.
            Some(draw::RailHit::Workspace(n)) if s.peek.target.get().is_some_and(|(t, _)| t == n) => {
                hide_peek(&s.peek.widget, &s.peek.target);
                s.workspaces.borrow_mut().go(n);
            }
            Some(draw::RailHit::Workspace(n)) => {
                let dot_y = draw::rail_items(&s.scene(&numbers, "", &clock)).iter().find(|i| i.0 == draw::RailHit::Workspace(n)).map_or(y, |i| i.1);
                s.show_peek(n, dot_y);
            }
            Some(draw::RailHit::NewWorkspace) => s.workspaces.borrow_mut().create_next(),
            Some(draw::RailHit::Config) => s.workspaces.borrow_mut().toggle_config(),
            None => {}
        }
    });
    state.window.add_controller(click);

    // Drag the open drawer's right edge to resize it.
    let drag = gtk::GestureDrag::new();
    drag.set_propagation_phase(gtk::PropagationPhase::Capture);
    let start_width = Rc::new(Cell::new(0.0));
    let (s, start) = (state.clone(), start_width.clone());
    drag.connect_drag_begin(move |gesture, x, _| {
        if s.near_resize_edge(x) {
            start.set(s.drawer.borrow().width);
            gesture.set_state(gtk::EventSequenceState::Claimed);
        } else {
            gesture.set_state(gtk::EventSequenceState::Denied);
        }
    });
    let (s, start) = (state.clone(), start_width);
    drag.connect_drag_update(move |_, dx, _| {
        s.drawer.borrow_mut().set_width(start.get() + dx);
        s.sync();
        s.area.queue_draw();
    });
    state.window.add_controller(drag);

    let motion = gtk::EventControllerMotion::new();
    motion.set_propagation_phase(gtk::PropagationPhase::Capture);
    let s = state.clone();
    motion.connect_motion(move |_, x, y| {
        let cursor = s.near_resize_edge(x).then_some("col-resize");
        s.window.set_cursor_from_name(cursor);
        s.track_peek(x, y);
    });
    let s = state.clone();
    motion.connect_leave(move |_| hide_peek(&s.peek.widget, &s.peek.target));
    state.window.add_controller(motion);
}

fn hide_peek(widget: &gtk::Box, target: &Cell<Option<(u8, f64)>>) {
    widget.set_visible(false);
    target.set(None);
}

/// The peek panel: a header that switches to the workspace, then its file tree.
/// Opening a file from it switches too, and opens the file there.
fn build_peek(overlay: &gtk::Overlay, workspaces: &Rc<RefCell<Workspaces>>) -> Peek {
    let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
    widget.add_css_class("drawer");
    widget.add_css_class("peek");
    widget.set_halign(gtk::Align::Start);
    widget.set_valign(gtk::Align::Start);
    widget.set_margin_start((draw::token::RAIL_WIDTH + PEEK_GAP) as i32);
    widget.set_size_request(PEEK_WIDTH, PEEK_HEIGHT);
    widget.set_overflow(gtk::Overflow::Hidden);
    widget.set_visible(false);
    let target: Rc<Cell<Option<(u8, f64)>>> = Rc::new(Cell::new(None));

    let go = gtk::Button::with_label("");
    go.add_css_class("go");
    go.set_halign(gtk::Align::Fill);
    if let Some(label) = go.child().and_downcast::<gtk::Label>() {
        label.set_xalign(0.0);
    }
    let (w, t, ws) = (widget.clone(), target.clone(), workspaces.clone());
    go.connect_clicked(move |_| {
        if let Some((n, _)) = t.get() {
            hide_peek(&w, &t);
            ws.borrow_mut().go(n);
        }
    });
    widget.append(&go);

    let (w, t, ws) = (widget.clone(), target.clone(), workspaces.clone());
    let open_there: Rc<dyn Fn(&Path)> = Rc::new(move |path: &Path| {
        let Some((n, _)) = t.get() else { return };
        hide_peek(&w, &t);
        ws.borrow_mut().go(n);
        let view = ws.borrow().current();
        view.borrow_mut().add(Kind::Editor(Some(path)), None);
    });
    let tree = file_tree::FileTree::new(PathBuf::new(), open_there);
    let tree_widget = tree.borrow().widget().clone();
    tree_widget.set_vexpand(true);
    widget.append(&tree_widget);
    overlay.add_overlay(&widget);
    Peek { widget, go, tree, target }
}

fn connect_keys(state: &Rc<App>, app: &gtk::Application) {
    // Capture phase: TileIDE's bindings win over the focused editor or terminal.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let (s, app) = (state.clone(), app.clone());
    keys.connect_key_pressed(move |_, key, _, modifiers| {
        let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
        let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
        let key = key.to_lower();
        let view = s.workspaces.borrow().current();

        if ctrl && !alt && key == gdk::Key::s {
            view.borrow_mut().save_focused(s.window.upcast_ref());
            return glib::Propagation::Stop;
        }
        if !alt {
            return glib::Propagation::Proceed;
        }
        if let Some(n) = key.to_unicode().and_then(|c| c.to_digit(10)).and_then(workspaces::from_digit) {
            let started = Instant::now();
            s.workspaces.borrow_mut().go(n);
            s.report_after_paint(started, "workspace switch");
            return glib::Propagation::Stop;
        }
        let direction = match key {
            gdk::Key::h => Some(tiles::Direction::Left),
            gdk::Key::j => Some(tiles::Direction::Down),
            gdk::Key::k => Some(tiles::Direction::Up),
            gdk::Key::l => Some(tiles::Direction::Right),
            _ => None,
        };
        match (key, direction) {
            (_, Some(d)) if shift => view.borrow_mut().resize(d),
            (_, Some(d)) => view.borrow_mut().focus_direction(d),
            (gdk::Key::Return, _) => view.borrow_mut().add(Kind::Terminal, None),
            (gdk::Key::e, _) => view.borrow_mut().add(Kind::Editor(None), None),
            (gdk::Key::w, _) => view.borrow_mut().close_focused(),
            (gdk::Key::c, _) => s.workspaces.borrow_mut().toggle_config(),
            (gdk::Key::b, _) => s.toggle_drawer(),
            (gdk::Key::z, _) => s.workspaces.borrow_mut().toggle_wrap(),
            (gdk::Key::Escape, _) => app.quit(),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    state.window.add_controller(keys);
}

/// Toggles every 350 ms (each animation is 200 ms), then prints statistics and quits.
fn start_bench(app: &gtk::Application, state: &Rc<App>, toggles: u32) {
    let remaining = Cell::new(toggles);
    let (app, s) = (app.clone(), state.clone());
    // Give the compositor time to map and size the surface first.
    glib::timeout_add_local(Duration::from_millis(350), move || {
        if remaining.get() == 0 {
            bench::print_stats(&s.window, &s.stats.borrow());
            app.quit();
            return glib::ControlFlow::Break;
        }
        remaining.set(remaining.get() - 1);
        s.toggle_drawer();
        glib::ControlFlow::Continue
    });
}
