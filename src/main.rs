// SPDX-License-Identifier: GPL-3.0-or-later
//! TileIDE: the Caelestia-style canvas (rail + frame + drawer), workspaces, a Files
//! drawer, and GtkSourceView editor / VTE terminal tiles in a dwindle or scrolling
//! columns layout. Configured by `~/.config/tileide/tileide.lua` (hot-reloaded).
//! `tileide --help` lists options.
//!
//! Mouse: rail launcher = Files drawer of the active workspace, workspace dot = peek at
//!   that workspace's files (click the dot again or the panel's header to switch; the
//!   peek closes when the pointer leaves it), + = new workspace, palette = theme tray,
//!   gear = Config workspace, drag the drawer's right edge to resize it, click a tile
//!   to focus it.

mod background;
mod bench;
mod cli;
mod dialog;
mod draw;
mod file_tree;
mod logo;
mod motion;
mod overview;
mod quick_open;
mod settings;
mod tile_view;
mod tray;
mod workspaces;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4 as gtk;

use bench::FrameStats;
use cli::{Mode, Options};
use dialog::Role;
use overview::Overview;
use quick_open::QuickOpen;
use settings::{KeyBind, Paths};
use tile_view::Kind;
use tileide_config::{Action, Config, State};
use tileide_core::drawer::Drawer;
use tileide_core::theme::{self, Theme};
use tileide_core::token::{self, Geometry};
use tray::Tray;
use workspaces::{Active, Workspaces};

const APP_ID: &str = "dev.tileide.TileIDE";

/// Grab distance around the drawer's right edge for mouse resizing.
const RESIZE_GRAB: f64 = 6.0;
const PEEK_WIDTH: i32 = 260;
const PEEK_HEIGHT: i32 = 360;
/// Space between the rail's right edge and the peek panel; the pointer may cross it.
const PEEK_GAP: f64 = 6.0;
/// Editors save in bursts (temp file, rename); wait for the burst before reloading.
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(60);

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
    files: Rc<RefCell<file_tree::FileTree>>,
    peek: Peek,
    workspaces: Rc<RefCell<Workspaces>>,
    clock: RefCell<(String, String)>,
    /// Full-screen only (spec §4.4); `None` = see-through. Rebuilt when its settings
    /// or the theme change.
    background: RefCell<Option<background::Background>>,
    background_key: RefCell<Option<(background::Settings, String)>>,
    stats: RefCell<FrameStats>,

    paths: Paths,
    /// The last config that loaded; a broken edit keeps it.
    config: RefCell<Config>,
    state: RefCell<State>,
    /// A tray pick made while the config pins the theme: lasts until restart (Q15).
    session_theme: RefCell<Option<String>>,
    theme_name: RefCell<String>,
    theme: RefCell<Rc<Theme>>,
    binds: RefCell<Vec<KeyBind>>,
    css: gtk::CssProvider,
    /// Config errors and warnings, shown over the stage until clicked away.
    banner: gtk::Label,
    /// Problems from the last config load (theme problems are added as they occur).
    problems: RefCell<Vec<String>>,
    tray: Tray,
    overview: Rc<Overview>,
    quick_open: Rc<QuickOpen>,
    /// A still of the old frame that fades away when the theme changes (`motion::crossfade`).
    cover: gtk::Picture,
    reload_timer: RefCell<Option<glib::SourceId>>,
    _monitors: RefCell<Vec<gio::FileMonitor>>,
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
        Ok(cli::Command::CheckConfig(file)) => return check_config(file),
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
    // Non-unique: every `tileide` call opens its own window, like `code -n`.
    let app = gtk::Application::builder().application_id(APP_ID).flags(gio::ApplicationFlags::NON_UNIQUE).build();
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

/// `tileide --check-config [FILE]`: for dotfile repos and CI.
fn check_config(file: Option<PathBuf>) -> glib::ExitCode {
    let paths = Paths::new(file.unwrap_or_else(config_path));
    match paths.load() {
        Ok(loaded) => {
            loaded.warnings.iter().for_each(|w| eprintln!("{w}"));
            let (_, key_problems) = settings::key_binds(&loaded.config.binds);
            key_problems.iter().for_each(|p| eprintln!("{p}"));
            let theme = loaded.config.appearance.theme.as_deref().map(|name| paths.theme(name));
            if let Some((_, Some(problem))) = &theme {
                eprintln!("{problem}");
            }
            println!("{}: OK ({:.1} ms, {} warning(s))", paths.file.display(), loaded.elapsed.as_secs_f64() * 1000.0, loaded.warnings.len() + key_problems.len());
            glib::ExitCode::SUCCESS
        }
        Err(e) => report(Err(e)),
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
    cli::config_path(&config_home, &settings::sample_dirs())
}

fn now_clock() -> (String, String) {
    let format = |f: &str| glib::DateTime::now_local().and_then(|d| d.format(f)).map(|s| s.to_string()).unwrap_or_default();
    (format("%H"), format("%M"))
}

fn build(app: &gtk::Application, options: Options, started: Instant) {
    let paths = Paths::new(config_path());
    let (config, mut problems) = match paths.load() {
        Ok(loaded) => {
            println!("config: {} loaded in {:.1} ms", paths.file.display(), loaded.elapsed.as_secs_f64() * 1000.0);
            (loaded.config, loaded.warnings)
        }
        Err(e) => (Config::default(), vec![format!("config error: {e}. Using the defaults until it is fixed.")]),
    };
    let state = tileide_config::load_state(&paths.state).unwrap_or_else(|e| {
        problems.push(format!("{}: {e}; ignored", paths.state.display()));
        State::default()
    });
    let fullscreen = options.mode == Mode::Fullscreen;
    if !fullscreen && options.background.is_some() {
        eprintln!("background: ignored outside full-screen mode (spec §4.4)");
    }

    let window = gtk::ApplicationWindow::new(app);
    window.set_title(Some(&format!("TileIDE · {}", folder_name(&options.folder))));
    window.set_decorated(false);
    let css = gtk::CssProvider::new();
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
    let theme = Rc::new(Theme::builtin());
    let workspaces = Rc::new(RefCell::new(Workspaces::new(tile_layer, paths.file.clone(), options.folder.clone(), config.tiling.clone(), theme.clone(), on_change)));

    let open_file: Rc<dyn Fn(&Path)> = {
        let workspaces = workspaces.clone();
        Rc::new(move |path: &Path| {
            let view = workspaces.borrow().current();
            view.borrow_mut().add(Kind::Editor(Some(path)));
        })
    };
    let files = file_tree::FileTree::new(options.folder.clone(), open_file);
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    panel.set_halign(gtk::Align::Start);
    panel.set_valign(gtk::Align::Fill);
    panel.set_margin_start(token::RAIL_WIDTH as i32);
    panel.set_margin_top(token::FRAME_BORDER as i32);
    panel.set_margin_bottom(token::FRAME_BORDER as i32);
    panel.set_overflow(gtk::Overflow::Hidden);
    panel.set_visible(false);
    let tree_widget = files.borrow().widget().clone();
    tree_widget.set_vexpand(true);
    panel.append(&tree_widget);
    overlay.add_overlay(&panel);
    let peek = build_peek(&overlay, &workspaces);

    let banner = gtk::Label::new(None);
    banner.add_css_class("banner");
    banner.set_halign(gtk::Align::Center);
    banner.set_valign(gtk::Align::Start);
    banner.set_margin_top((token::FRAME_BORDER + 10.0) as i32);
    // Over the stage, never over the rail's buttons.
    banner.set_margin_start((token::RAIL_WIDTH + 16.0) as i32);
    banner.set_margin_end(16);
    banner.set_wrap(true);
    banner.set_max_width_chars(100);
    banner.set_xalign(0.0);
    banner.set_visible(false);
    let dismiss = gtk::GestureClick::new();
    let b = banner.clone();
    dismiss.connect_released(move |_, _, _, _| b.set_visible(false));
    banner.add_controller(dismiss);
    overlay.add_overlay(&banner);

    let app_state = Rc::new_cyclic(|me: &Weak<App>| {
        let me1 = me.clone();
        let on_apply: Rc<dyn Fn(&str)> = Rc::new(move |key: &str| {
            if let Some(app) = me1.upgrade() {
                app.pick_theme(key);
            }
        });
        let me2 = me.clone();
        let on_close: Rc<dyn Fn()> = Rc::new(move || {
            if let Some(app) = me2.upgrade() {
                app.area.queue_draw();
                app.workspaces.borrow().current().borrow().grab_focused();
            }
        });
        let tray = Tray::new(&overlay, on_apply, on_close.clone());
        let me3 = me.clone();
        let on_pick: Rc<dyn Fn(u8)> = Rc::new(move |n| {
            if let Some(app) = me3.upgrade() {
                app.workspaces.borrow_mut().go(n);
            }
        });
        let overview = Overview::new(&overlay, on_pick, on_close.clone());
        let me4 = me.clone();
        let on_file: Rc<dyn Fn(quick_open::Target, Option<PathBuf>)> = Rc::new(move |target, path| {
            if let Some(app) = me4.upgrade() {
                app.open_picked(target, path);
            }
        });
        let quick_open = QuickOpen::new(&overlay, on_file, on_close);
        // Last, so confirmations sit above every other layer.
        dialog::install(&overlay);
        let cover = gtk::Picture::new();
        cover.set_can_target(false);
        cover.set_can_shrink(true);
        cover.set_content_fit(gtk::ContentFit::Fill);
        cover.set_visible(false);
        overlay.add_overlay(&cover);
        App {
            options: options.clone(),
            window: window.clone(),
            area: area.clone(),
            drawer: RefCell::new(Drawer::default()),
            panel,
            files,
            peek,
            workspaces,
            clock: RefCell::new(now_clock()),
            background: RefCell::new(None),
            background_key: RefCell::new(None),
            stats: RefCell::new(FrameStats::default()),
            paths,
            config: RefCell::new(Config::default()),
            state: RefCell::new(state),
            session_theme: RefCell::new(None),
            theme_name: RefCell::new(String::new()),
            theme: RefCell::new(theme),
            binds: RefCell::new(Vec::new()),
            css,
            banner,
            problems: RefCell::new(Vec::new()),
            tray,
            overview,
            quick_open,
            cover,
            reload_timer: RefCell::new(None),
            _monitors: RefCell::new(Vec::new()),
        }
    });

    {
        let me = Rc::downgrade(&app_state);
        app_state.workspaces.borrow_mut().set_on_switch(Rc::new(move |info: &workspaces::Switch| me.upgrade().and_then(|app| app.on_switch(info))));
    }
    let start_expanded = config.sidebar.start_expanded && config.sidebar.show_files;
    app_state.apply_config(config, problems);
    if start_expanded {
        app_state.toggle_drawer();
    }
    if let Some(file) = &options.open {
        let view = app_state.workspaces.borrow().current();
        view.borrow_mut().add(Kind::Editor(Some(file)));
    }

    watch_config(&app_state);
    connect_drawing(&app_state, started);
    connect_mouse(&app_state);
    connect_keys(&app_state);
    connect_close(&app_state);

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

fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(|| "/".into(), |n| n.to_string_lossy().into_owned())
}

impl App {
    fn scene<'a>(&self, numbers: &'a [u8], label: &'a str, clock: &'a (String, String)) -> draw::Scene<'a> {
        let workspaces = self.workspaces.borrow();
        let config = self.config.borrow();
        let s = &config.sidebar;
        draw::Scene {
            width: self.area.width() as f64,
            height: self.area.height() as f64,
            theme: self.theme.borrow().clone(),
            geometry: self.geometry(),
            modules: draw::Modules {
                launcher: s.show_launcher && s.show_files,
                workspaces: s.show_workspaces,
                label: s.show_label,
                clock: s.show_clock,
                themes: s.show_themes,
                config: s.show_config,
            },
            themes_open: self.tray.is_open(),
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
            frosted: None,
            tile_rects: &[],
            tile_opacity: token::TILE_OPACITY,
        }
    }

    fn geometry(&self) -> Geometry {
        let config = self.config.borrow();
        Geometry {
            tile_radius: config.appearance.window_radius as f64,
            gaps_in: config.tiling.gaps_in as f64,
            gaps_out: config.tiling.gaps_out as f64,
        }
    }

    fn label(&self) -> String {
        let workspaces = self.workspaces.borrow();
        match workspaces.active() {
            Active::Number(n) => {
                let name = workspaces.name(n).unwrap_or_else(|| folder_name(&workspaces.root(Active::Number(n))));
                format!("{name} · {}", workspaces::display(n))
            }
            Active::Config => "config".into(),
        }
    }

    /// Applies a freshly loaded config (or the defaults at startup after an error).
    fn apply_config(&self, config: Config, mut problems: Vec<String>) {
        let (binds, key_problems) = settings::key_binds(&config.effective_binds());
        problems.extend(key_problems);
        for (n, w) in &config.workspaces {
            if let Some(folder) = &w.folder
                && !workspaces::expand_home(folder).is_dir()
            {
                problems.push(format!("workspace({n}): folder \"{folder}\" does not exist; using {}", self.options.folder.display()));
            }
        }
        // Editing the theme in the config replaces a session-only tray pick.
        if config.appearance.theme != self.config.borrow().appearance.theme {
            *self.session_theme.borrow_mut() = None;
        }
        *self.binds.borrow_mut() = binds;
        motion::set_enabled(config.appearance.animations);
        let (tiling, per_workspace) = (config.tiling.clone(), config.workspaces.clone());
        *self.config.borrow_mut() = config;
        *self.problems.borrow_mut() = problems;
        // Re-roots the tree and refreshes theme, background, CSS and banner (`on_switch`).
        self.workspaces.borrow_mut().configure(tiling, per_workspace);
        self.sync();
    }

    fn reload(&self) {
        match self.paths.load() {
            Ok(loaded) => {
                println!("config: reloaded in {:.1} ms", loaded.elapsed.as_secs_f64() * 1000.0);
                self.apply_config(loaded.config, loaded.warnings);
            }
            Err(e) => {
                eprintln!("config: {e}");
                self.problems.borrow_mut().retain(|p| !p.starts_with("config error"));
                self.problems.borrow_mut().insert(0, format!("config error: {e}. The last working config stays active."));
                self.show_problems();
            }
        }
    }

    /// After a workspace switch or a config change: the drawer's tree, the theme and
    /// the background follow the active workspace. Runs inside a `Workspaces` borrow, so
    /// it must not touch `self.workspaces`; a changed theme is returned for it to apply.
    fn on_switch(&self, info: &workspaces::Switch) -> Option<Rc<Theme>> {
        self.files.borrow_mut().set_root(&info.root);
        let theme = self.refresh_theme(info.theme.as_deref());
        self.refresh_background(info.background.as_deref());
        self.reload_css();
        self.show_problems();
        self.area.queue_draw();
        theme
    }

    /// `on_switch` from outside a `Workspaces` borrow (e.g. after a tray pick).
    fn refresh(&self) {
        let info = self.workspaces.borrow().switch_info();
        if let Some(theme) = self.on_switch(&info) {
            self.workspaces.borrow_mut().set_theme(theme);
        }
    }

    /// The theme for the active workspace; `Some` when it changed.
    fn refresh_theme(&self, workspace: Option<&str>) -> Option<Rc<Theme>> {
        let name = settings::theme_name(
            workspace,
            self.session_theme.borrow().as_deref(),
            self.config.borrow().appearance.theme.as_deref(),
            self.state.borrow().theme.as_deref(),
        );
        if *self.theme_name.borrow() == name {
            return None;
        }
        let (theme, problem) = self.paths.theme(&name);
        if let Some(problem) = problem {
            self.problems.borrow_mut().push(problem);
        }
        let theme = Rc::new(theme);
        // Before the new theme is stored: the still must show the old colours. Painting it
        // draws the canvas now, which needs the workspaces and config; inside a workspace
        // switch they are borrowed, and the switch's own fade covers the change.
        let free = self.workspaces.try_borrow().is_ok_and(|w| w.current().try_borrow().is_ok()) && self.config.try_borrow().is_ok();
        if free {
            motion::crossfade(&self.cover);
        }
        *self.theme_name.borrow_mut() = name;
        *self.theme.borrow_mut() = theme.clone();
        Some(theme)
    }

    /// Full-screen only. Command-line flags win over `tileide.background`; a workspace's
    /// image wins over both.
    fn refresh_background(&self, image: Option<&str>) {
        if self.options.mode != Mode::Fullscreen {
            return;
        }
        let settings = match (&self.options.background, image) {
            (Some(cli), None) => Ok(cli.clone()),
            (_, image) => settings::background(&self.config.borrow().background, image),
        };
        let settings = settings.unwrap_or_else(|e| {
            self.problems.borrow_mut().push(e);
            background::Settings::default()
        });
        let key = Some((settings.clone(), self.theme_name.borrow().clone()));
        if *self.background_key.borrow() == key {
            return;
        }
        *self.background_key.borrow_mut() = key;
        *self.background.borrow_mut() = background::Background::new(settings, (**self.theme.borrow()).clone());
        load_background(self);
    }

    fn tile_opacity(&self) -> f64 {
        self.background_key.borrow().as_ref().map_or(token::TILE_OPACITY, |(s, _)| s.tile_opacity)
    }

    fn reload_css(&self) {
        let theme = self.theme.borrow();
        let t = &**theme;
        self.css.load_from_string(&format!(
            "window, window.background {{ background: transparent; font-family: \"{font}\"; }}
.banner {{ background-color: {bg}; color: {text}; border: 1px solid {edge}; border-radius: 10px; padding: 8px 14px; font-size: 13px; }}
{tiles}\n{files}\n{overview}\n{quick_open}\n{dialog}",
            font = token::UI_FONT,
            bg = theme::css(t.surface, 0.96),
            text = theme::css(t.text, 1.0),
            edge = theme::css(t.error, 0.8),
            tiles = tile_view::css(t, self.geometry(), self.tile_opacity(), self.background.borrow().is_some()),
            files = file_tree::css(t),
            overview = overview::css(t),
            quick_open = quick_open::css(t),
            dialog = dialog::css(t),
        ));
    }

    fn show_problems(&self) {
        let problems = self.problems.borrow();
        problems.iter().for_each(|p| eprintln!("{p}"));
        match problems.first() {
            None => self.banner.set_visible(false),
            Some(first) => {
                let more = match problems.len() {
                    1 => String::new(),
                    n => format!("\n(+{} more; run tileide --check-config)", n - 1),
                };
                self.banner.set_text(&format!("{first}{more}"));
                self.banner.set_tooltip_text(Some("Click to dismiss"));
                self.banner.set_visible(true);
            }
        }
    }

    /// The tray applied `key`. Saved to `state.lua` unless the config pins the theme,
    /// in which case it lasts for this session only (Q15).
    fn pick_theme(&self, key: &str) {
        let pinned = self.config.borrow().appearance.theme.clone();
        let mut notes = Vec::new();
        match pinned {
            Some(pinned) if pinned != key => {
                *self.session_theme.borrow_mut() = Some(key.into());
                notes.push(format!("tileide.lua sets theme = \"{pinned}\", so \"{key}\" applies until TileIDE restarts."));
            }
            _ => {
                *self.session_theme.borrow_mut() = None;
                self.state.borrow_mut().theme = Some(key.into());
                if let Err(e) = tileide_config::save_state(&self.paths.state, &self.state.borrow()) {
                    notes.push(format!("cannot save the theme to {}: {e}", self.paths.state.display()));
                }
            }
        }
        if let (Some(own), Active::Number(n)) = (self.workspaces.borrow().overrides().0, self.workspaces.borrow().active()) {
            notes.push(format!("workspace {} uses its own theme (\"{own}\"); the pick shows in other workspaces.", workspaces::display(n)));
        }
        *self.problems.borrow_mut() = notes;
        self.refresh();
    }

    fn open_tray(&self) {
        if self.tray.is_open() {
            self.tray.close();
        } else {
            self.overview.close();
            self.tray.open(self.paths.all_themes(), &self.theme_name.borrow(), self.theme.borrow().clone());
        }
        self.area.queue_draw();
    }

    /// Alt+E (new tile) / Alt+F (focused tile); the same key again closes it.
    fn open_quick(&self, target: quick_open::Target) {
        if self.quick_open.is_open() {
            self.quick_open.close();
            self.workspaces.borrow().current().borrow().grab_focused();
            return;
        }
        self.tray.close();
        self.overview.close();
        let folder = {
            let workspaces = self.workspaces.borrow();
            workspaces.root(workspaces.active())
        };
        self.quick_open.open(target, &folder);
        self.area.queue_draw();
    }

    fn open_picked(&self, target: quick_open::Target, path: Option<PathBuf>) {
        let view = self.workspaces.borrow().current();
        match (target, path) {
            (quick_open::Target::Focused, Some(path)) => view.borrow_mut().open_in_focused(&path, self.window.upcast_ref()),
            (_, path) => view.borrow_mut().add(Kind::Editor(path.as_deref())),
        }
        self.area.queue_draw();
    }

    fn open_overview(&self, me: &Rc<App>) {
        if self.overview.is_open() {
            self.overview.close();
            self.workspaces.borrow().current().borrow().grab_focused();
            return;
        }
        self.tray.close();
        let me = Rc::downgrade(me);
        let on_pick: Rc<dyn Fn(u8)> = Rc::new(move |n| {
            if let Some(app) = me.upgrade() {
                app.workspaces.borrow_mut().go(n);
            }
        });
        self.overview.open(&self.workspaces.borrow().summaries(), on_pick);
    }

    /// Places tiles and the Files panel for the current drawer extension and window size.
    fn sync(&self) {
        let numbers = self.workspaces.borrow().numbers();
        let clock = self.clock.borrow();
        let scene = self.scene(&numbers, "", &clock);
        self.workspaces.borrow_mut().set_area(draw::tile_area(&scene), draw::stage_rect(&scene));

        // The panel appears once the drawer is wide enough for its content (it fades
        // in during the last part of the expansion, design.md §8).
        let drawer = self.drawer.borrow();
        let extension = drawer.extension();
        let shown = extension >= token::DRAWER_MIN_WIDTH * 0.9;
        self.panel.set_visible(shown);
        if shown {
            self.panel.set_size_request(extension as i32, -1);
            self.panel.set_opacity(((extension / drawer.width - 0.6) / 0.4).clamp(0.0, 1.0));
        }
    }

    fn toggle_drawer(&self) {
        let config = self.config.borrow();
        if !config.sidebar.show_files && !self.drawer.borrow().is_open() {
            return;
        }
        drop(config);
        self.drawer.borrow_mut().toggle();
        self.area.queue_draw();
    }

    fn run(&self, me: &Rc<App>, action: &Action) {
        let view = self.workspaces.borrow().current();
        match action {
            Action::Workspace(n) => {
                let started = Instant::now();
                self.workspaces.borrow_mut().go(*n);
                self.report_after_paint(started, "workspace switch");
            }
            Action::ConfigWorkspace => self.workspaces.borrow_mut().toggle_config(),
            Action::Sidebar => self.toggle_drawer(),
            Action::Layout(kind) => view.borrow_mut().set_layout(*kind),
            Action::Split(axis) => view.borrow_mut().split_next(*axis),
            Action::Focus(d) => view.borrow_mut().focus_direction(*d),
            Action::Move(d) => view.borrow_mut().move_focused(*d),
            Action::Resize(d) => view.borrow_mut().resize(*d),
            Action::Close => view.borrow_mut().close_focused(self.window.upcast_ref()),
            Action::Terminal => view.borrow_mut().add(Kind::Terminal),
            Action::Editor => self.open_quick(quick_open::Target::NewTile),
            Action::Search => self.open_quick(quick_open::Target::Focused),
            Action::Wrap => self.workspaces.borrow_mut().toggle_wrap(),
            Action::Save => view.borrow_mut().save_focused(self.window.upcast_ref()),
            Action::Quit => self.window.close(),
            Action::Overview => self.open_overview(me),
            Action::Themes => self.open_tray(),
            Action::Unavailable(_) => {}
        }
    }

    /// Shows workspace `n`'s files next to its dot (a click on the dot).
    fn show_peek(&self, n: u8, dot_y: f64) {
        let peek = &self.peek;
        peek.tree.borrow_mut().set_root(&self.workspaces.borrow().root(Active::Number(n)));
        peek.go.set_label(&format!("Workspace {}  →", workspaces::display(n)));
        let max_top = (self.area.height() - PEEK_HEIGHT - token::FRAME_BORDER as i32).max(0);
        peek.widget.set_margin_top(((dot_y - 20.0) as i32).clamp(0, max_top));
        peek.widget.set_visible(true);
        peek.target.set(Some((n, dot_y)));
    }

    /// Closes the peek once the pointer is neither on its dot nor over the panel.
    fn track_peek(&self, x: f64, y: f64) {
        let Some((_, dot_y)) = self.peek.target.get() else { return };
        let on_dot = x <= token::RAIL_WIDTH + PEEK_GAP && (y - dot_y).abs() <= 14.0;
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
        drawer.is_open() && (x - (token::RAIL_WIDTH + drawer.width)).abs() <= RESIZE_GRAB
    }
}

/// Hot reload: watches the config dir, `rices/` and `themes/`. `state.lua` is ours.
fn watch_config(state: &Rc<App>) {
    let dirs = [state.paths.dir.clone(), state.paths.dir.join("rices"), state.paths.dir.join("themes")];
    for dir in dirs.iter().filter(|d| d.is_dir()) {
        let monitor = match gio::File::for_path(dir).monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, None::<&gio::Cancellable>) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("config: cannot watch {}: {e}", dir.display());
                continue;
            }
        };
        let me = Rc::downgrade(state);
        monitor.connect_changed(move |_, file, other, event| {
            use gio::FileMonitorEvent as E;
            if !matches!(event, E::ChangesDoneHint | E::Created | E::Deleted | E::MovedIn | E::MovedOut | E::Renamed) {
                return;
            }
            let relevant = [Some(file), other].into_iter().flatten().filter_map(|f| f.basename()).any(|name| {
                name.extension().is_some_and(|e| e == "lua") && name != Path::new("state.lua")
            });
            let Some(app) = me.upgrade() else { return };
            if !relevant {
                return;
            }
            if let Some(timer) = app.reload_timer.borrow_mut().take() {
                timer.remove();
            }
            let me = Rc::downgrade(&app);
            let timer = glib::timeout_add_local_once(RELOAD_DEBOUNCE, move || {
                if let Some(app) = me.upgrade() {
                    app.reload_timer.borrow_mut().take();
                    app.reload();
                }
            });
            *app.reload_timer.borrow_mut() = Some(timer);
        });
        state._monitors.borrow_mut().push(monitor);
    }
}

/// Decodes an image background on a worker thread, then redraws with it.
fn load_background(state: &App) {
    let Some(path) = state.background.borrow().as_ref().and_then(background::Background::image_path) else {
        state.area.queue_draw();
        return;
    };
    let area = state.area.clone();
    let key = state.background_key.borrow().clone();
    let target = Rc::new(RefCell::new(None::<background::Decoded>));
    let t = target.clone();
    glib::spawn_future_local(async move {
        let started = Instant::now();
        let decoded = gio::spawn_blocking(move || background::decode(&path)).await;
        match decoded {
            Ok(Ok(decoded)) => {
                println!("background: decoded in {:.1} ms (off the main thread)", started.elapsed().as_secs_f64() * 1000.0);
                *t.borrow_mut() = Some(decoded);
            }
            Ok(Err(e)) => eprintln!("background: {e}; staying see-through"),
            Err(_) => eprintln!("background: decoder thread panicked"),
        }
        area.queue_draw();
    });
    PENDING.with(|p| p.borrow_mut().push((key, target)));
}

thread_local! {
    /// Decoded images waiting to be handed to the background they were decoded for.
    static PENDING: RefCell<Vec<(Option<(background::Settings, String)>, Rc<RefCell<Option<background::Decoded>>>)>> = RefCell::default();
}

/// Hands finished decodes to the background, if it is still the one they were for.
fn take_decoded(state: &App) {
    PENDING.with(|pending| {
        pending.borrow_mut().retain(|(key, slot)| {
            let Some(decoded) = slot.borrow_mut().take() else { return true };
            if *key == *state.background_key.borrow()
                && let Some(b) = state.background.borrow_mut().as_mut()
                && let Err(e) = b.set_image(decoded)
            {
                eprintln!("background: {e}");
            }
            false
        });
    });
}

fn connect_drawing(state: &Rc<App>, started: Instant) {
    let first_frame = Cell::new(true);
    let s = state.clone();
    state.area.set_draw_func(move |area, cr, width, height| {
        let begin = Instant::now();
        take_decoded(&s);
        // Cheap clone (refcounted); baked only when the size changes.
        // The fractional scale (e.g. 1.5), so the paint is a 1:1 copy, not a resample.
        let scale = area.native().and_then(|n| n.surface()).map_or(area.scale_factor() as f64, |s| s.scale());
        let background = s.background.borrow_mut().as_mut().and_then(|b| b.surface(width, height, scale).cloned());
        let frosted = s.background.borrow().as_ref().and_then(|b| b.frosted().cloned());
        let tile_rects = if frosted.is_some() { s.workspaces.borrow().current().borrow().tile_rects() } else { Vec::new() };
        let numbers = s.workspaces.borrow().numbers();
        let label = s.label();
        let clock = s.clock.borrow();
        let mut scene = s.scene(&numbers, &label, &clock);
        scene.background = background.as_ref();
        scene.frosted = frosted.as_ref();
        scene.tile_rects = &tile_rects;
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
        // The tray and the overview handle their own clicks.
        if s.tray.is_open() || s.overview.is_open() || s.quick_open.is_open() || dialog::is_open() {
            return;
        }
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
            Some(draw::RailHit::Themes) => s.open_tray(),
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
    widget.set_margin_start((token::RAIL_WIDTH + PEEK_GAP) as i32);
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
        view.borrow_mut().add(Kind::Editor(Some(path)));
    });
    let tree = file_tree::FileTree::new(PathBuf::new(), open_there);
    let tree_widget = tree.borrow().widget().clone();
    tree_widget.set_vexpand(true);
    widget.append(&tree_widget);
    overlay.add_overlay(&widget);
    Peek { widget, go, tree, target }
}

fn connect_keys(state: &Rc<App>) {
    // Capture phase: TileIDE's bindings win over the focused editor or terminal.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let s = state.clone();
    keys.connect_key_pressed(move |controller, key, keycode, modifiers| {
        // Match on the unshifted key, so `Alt+Shift+1` is "1" with SHIFT, not "exclam".
        let base = WidgetExt::display(&s.window)
            .translate_key(keycode, gdk::ModifierType::empty(), controller.group() as i32)
            .map_or(key, |(k, ..)| k)
            .to_lower();
        let mods = modifiers & settings::BIND_MODS;
        let action = s.binds.borrow().iter().find(|b| b.mods == mods && (b.key == base || b.key == key.to_lower())).map(|b| b.action.clone());
        let Some(action) = action else { return glib::Propagation::Proceed };
        // A confirmation takes every key until it is answered.
        if dialog::is_open() {
            return glib::Propagation::Proceed;
        }
        // While the tray or overview is open, only their own toggles act.
        if (s.tray.is_open() || s.overview.is_open() || s.quick_open.is_open()) && !matches!(action, Action::Themes | Action::Overview | Action::Editor | Action::Search | Action::Quit) {
            return glib::Propagation::Proceed;
        }
        s.run(&s, &action);
        glib::Propagation::Stop
    });
    state.window.add_controller(keys);
}

/// Closing the window (Alt+Escape, or the compositor) asks first while any editor in
/// any workspace has unsaved changes.
fn connect_close(state: &Rc<App>) {
    let confirmed = Rc::new(Cell::new(false));
    let s = state.clone();
    state.window.connect_close_request(move |window| {
        let dirty = s.workspaces.borrow().dirty_count();
        if dirty == 0 || confirmed.get() {
            return glib::Propagation::Proceed;
        }
        let files = if dirty == 1 { "1 file has".to_string() } else { format!("{dirty} files have") };
        let (window, confirmed) = (window.clone(), confirmed.clone());
        dialog::ask(
            "Quit TileIDE?",
            &format!("{files} unsaved changes. They are lost if you quit now."),
            &[("Cancel", Role::Cancel), ("Quit Without Saving", Role::Destructive)],
            0,
            move |choice| {
                if choice == 1 {
                    confirmed.set(true);
                    window.close();
                }
            },
        );
        glib::Propagation::Stop
    });
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
