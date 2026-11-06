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
mod alerts;
mod audio;
mod bench;
mod cli;
mod completion;
mod debug;
mod dialog;
mod draw;
mod editing;
mod extensions;
mod extensions_panel;
mod interpreter;
mod keybinds_card;
mod file_tree;
mod logo;
mod lsp;
mod motion;
mod notebook_view;
mod overview;
mod pick_card;
mod py_settings;
mod pytest;
mod quick_open;
mod run_card;
mod settings;
mod settings_ui;
mod setup_export;
mod theme_editor;
mod themes_panel;
mod tile_view;
mod toolchains;
mod tests_view;
mod tray;
mod variables_view;
mod wiki;
mod workspaces;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
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
use tileide_core::motion::Presence;
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
/// Settings changes are written to `state.lua` once a slider rests this long.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(400);

/// The panel a workspace dot opens: that workspace's file tree, without switching to it.
struct Peek {
    widget: gtk::Box,
    go: gtk::Button,
    /// Removes the peeked workspace.
    close: gtk::Button,
    tree: Rc<RefCell<file_tree::FileTree>>,
    /// The workspace shown, and its dot's centre y; `None` while hidden.
    target: Rc<Cell<Option<(u8, f64)>>>,
}

struct App {
    /// For callbacks that must run after the current borrow ends (idle).
    me: Weak<App>,
    /// The Wikipedia rice's top bar and workspace list.
    bars: Rc<wiki::Bars>,
    /// Wikipedia rice: the workspace list shows (its "hide" and the top bar's toggle).
    workspace_list: Cell<bool>,
    options: Options,
    window: gtk::ApplicationWindow,
    area: gtk::DrawingArea,
    drawer: RefCell<Drawer>,
    /// The drawer's slide has a tick callback running (`slide_drawer`).
    drawer_tick: Cell<bool>,
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
    /// `editor.theme_syntax` as the current theme was built with it.
    theme_syntax: Cell<bool>,
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
    settings_card: Rc<settings_ui::SettingsCard>,
    keybinds_card: Rc<keybinds_card::KeybindsCard>,
    run_card: Rc<run_card::RunCard>,
    /// The launch config Alt+R and F5 use, by workspace number (Alt+L); none = the focused file.
    launch_pick: RefCell<HashMap<u8, String>>,
    visualizer: Rc<audio::Visualizer>,
    /// The visualizer under the file tree (drawer open).
    wave: gtk::DrawingArea,
    /// The visualizer on the rail (drawer closed), and how shown it was last frame.
    rail_wave: gtk::DrawingArea,
    rail_wave_k: Cell<f64>,
    /// The config as `tileide.lua` gives it; `config` is it with the GUI settings applied.
    raw_config: RefCell<Config>,
    save_timer: RefCell<Option<glib::SourceId>>,
    themes_panel: Rc<themes_panel::ThemesPanel>,
    theme_editor: Rc<theme_editor::ThemeEditor>,
    /// The Tests tile (made at start-up, shown on the first test run).
    tests: RefCell<Option<Rc<tests_view::TestsView>>>,
    extensions_panel: Rc<extensions_panel::ExtensionsPanel>,
    /// The workspace the Themes section was last filled for.
    panel_for: Cell<Option<Option<u8>>>,
    /// A Themes section row shown over the applied theme while hovered or selected.
    preview_theme: RefCell<Option<String>>,
    /// The theme editor's draft, shown over every other theme while its page is open.
    draft_theme: RefCell<Option<Rc<Theme>>>,
    /// Bumped on every draft change, so `refresh_theme` sees a new name each time.
    draft_count: Cell<u32>,
    /// The background picked in Settings this session (also saved in `state.lua`); it
    /// shows over `--bg`.
    session_background: RefCell<Option<tileide_config::model::Background>>,
    /// The rail's workspace dots, fading in and out as workspaces come and go.
    dots: RefCell<Presence>,
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
        Ok(cli::Command::DapJdb) => {
            tileide_dap::jdb::serve(std::io::stdin(), std::io::stdout());
            return glib::ExitCode::SUCCESS;
        }
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
    // Before the first workspace fades in: with animations off it must just appear.
    motion::set_enabled(tileide_config::settings::apply(config.clone(), &state.settings).appearance.animations);
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
    // Fill: its width is set exactly by the overlay (`connect_get_child_position`).
    panel.set_halign(gtk::Align::Fill);
    panel.set_valign(gtk::Align::Fill);
    panel.set_margin_start(token::rail_width() as i32);
    panel.set_margin_top(token::FRAME_BORDER as i32);
    panel.set_margin_bottom(token::FRAME_BORDER as i32);
    panel.set_overflow(gtk::Overflow::Hidden);
    panel.set_visible(false);
    panel.add_css_class("drawer");
    let tree_widget = files.borrow().widget().clone();
    tree_widget.set_vexpand(true);
    panel.append(&tree_widget);
    let wave = gtk::DrawingArea::new();
    wave.set_content_height(44);
    wave.set_margin_start(14);
    wave.set_margin_end(14);
    wave.set_margin_bottom(10);
    wave.set_margin_top(6);
    wave.set_visible(false);
    panel.append(&wave);
    overlay.add_overlay(&panel);
    // The visualizer on the rail (drawer closed): placed by margins over the logo.
    let rail_wave = gtk::DrawingArea::new();
    rail_wave.set_halign(gtk::Align::Start);
    rail_wave.set_valign(gtk::Align::Start);
    rail_wave.set_can_target(false);
    rail_wave.set_visible(false);
    overlay.add_overlay(&rail_wave);
    let peek = build_peek(&overlay, &workspaces);
    // The Wikipedia rice's bars: above the tiles and the drawer, below every card.
    // Built before the app, so their callbacks reach it through `link`.
    let link: Rc<std::cell::OnceCell<Weak<App>>> = Rc::default();
    let call = |f: fn(&App)| -> Rc<dyn Fn()> {
        let link = link.clone();
        Rc::new(move || {
            if let Some(app) = link.get().and_then(Weak::upgrade) {
                f(&app);
            }
        })
    };
    let go_link = link.clone();
    let bars = wiki::Bars::new(
        &overlay,
        wiki::Callbacks {
            toggle_files: call(|app| app.toggle_drawer()),
            search: call(|app| app.open_quick(quick_open::Target::NewTile)),
            settings: call(|app| app.toggle_settings(settings_ui::Page::General)),
            keybinds: call(|app| app.run(&Action::Keybinds)),
            toggle_list: call(|app| app.toggle_workspace_list()),
            go: Rc::new(move |n| {
                if let Some(app) = go_link.get().and_then(Weak::upgrade) {
                    app.workspaces.borrow_mut().go(n);
                }
            }),
            new_workspace: call(|app| app.workspaces.borrow_mut().create_next()),
            themes: call(|app| app.open_tray()),
        },
    );

    let banner = gtk::Label::new(None);
    banner.add_css_class("banner");
    banner.set_halign(gtk::Align::Center);
    banner.set_valign(gtk::Align::Start);
    banner.set_margin_top((token::FRAME_BORDER + 10.0) as i32);
    // Over the stage, never over the rail's buttons.
    banner.set_margin_start((token::rail_width() + 16.0) as i32);
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
        let _ = link.set(me.clone());
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
        let quick_open = QuickOpen::new(&overlay, on_file, on_close.clone());
        let (m5, m6, m7, m8, m_surface) = (me.clone(), me.clone(), me.clone(), me.clone(), me.clone());
        // Dragging a slider re-bakes the backdrops: apply once it rests a moment.
        let surface_timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
        let themes_panel = themes_panel::ThemesPanel::new(themes_panel::Callbacks {
            preview: Rc::new(move |key| {
                if let Some(app) = m5.upgrade() {
                    app.preview(key);
                }
            }),
            apply: Rc::new(move |key, scope| {
                if let Some(app) = m6.upgrade() {
                    app.apply_theme(key, scope);
                }
            }),
            background: Rc::new(move |bg, scope| {
                if let Some(app) = m7.upgrade() {
                    app.apply_background(bg, scope);
                }
            }),
            import: Rc::new(move || {
                if let Some(app) = m8.upgrade() {
                    app.import_theme();
                }
            }),
            surface: Rc::new(move |key, value| {
                if let Some(t) = surface_timer.borrow_mut().take() {
                    t.remove();
                }
                let (me, timer) = (m_surface.clone(), surface_timer.clone());
                *surface_timer.borrow_mut() = Some(glib::timeout_add_local_once(Duration::from_millis(60), move || {
                    timer.borrow_mut().take();
                    if let Some(app) = me.upgrade() {
                        app.change_setting(key, tileide_config::settings::Value::Number(value));
                    }
                }));
            }),
        });
        let (m9, m10) = (me.clone(), me.clone());
        let extensions_panel = extensions_panel::ExtensionsPanel::new(
            window.upcast_ref(),
            extensions_panel::Callbacks {
                run: Rc::new(move |command| {
                    if let Some(app) = m9.upgrade() {
                        app.run_extension_command(&command);
                    }
                }),
                save: Rc::new(move || {
                    if let Some(app) = m10.upgrade() {
                        app.state.borrow_mut().extensions = extensions::picks();
                        app.save_state();
                    }
                }),
            },
        );
        let (e1, e2, e3, e4) = (me.clone(), me.clone(), me.clone(), me.clone());
        let theme_editor = theme_editor::ThemeEditor::new(theme_editor::Callbacks {
            current: Rc::new(move || e1.upgrade().map_or_else(Theme::builtin, |app| app.theme_in_use())),
            preview: Rc::new(move |draft| {
                if let Some(app) = e2.upgrade() {
                    app.preview_draft(draft);
                }
            }),
            save: Rc::new(move |theme| {
                if let Some(app) = e3.upgrade() {
                    app.save_draft_theme(&theme);
                }
            }),
            export: Rc::new(move |theme| {
                if let Some(app) = e4.upgrade() {
                    app.export_theme(theme);
                }
            }),
        });
        let (m11, m12, m13, m14, m16, m17, m18, m19) = (me.clone(), me.clone(), me.clone(), me.clone(), me.clone(), me.clone(), me.clone(), me.clone());
        let settings_card = settings_ui::SettingsCard::new(
            &overlay,
            themes_panel.widget(),
            theme_editor.widget(),
            extensions_panel.widget(),
            settings_ui::Callbacks {
                change: Rc::new(move |key, value| {
                    if let Some(app) = m11.upgrade() {
                        app.change_setting(key, value);
                    }
                }),
                open_config: Rc::new(move || {
                    if let Some(app) = m12.upgrade()
                        && app.workspaces.borrow().active() != Active::Config
                    {
                        app.workspaces.borrow_mut().toggle_config();
                    }
                }),
                unlock: Rc::new(move |key| {
                    if let Some(app) = m16.upgrade() {
                        app.unlock_setting(key);
                    }
                }),
                show_keybinds: Rc::new(move || {
                    if let Some(app) = m13.upgrade() {
                        app.open_keybinds();
                    }
                }),
                export_setup: Rc::new(move || {
                    if let Some(app) = m17.upgrade() {
                        app.export_setup();
                    }
                }),
                project: Rc::new(move || {
                    let app = m19.upgrade()?;
                    let folder = {
                        let w = app.workspaces.borrow();
                        w.folder(w.active())
                    };
                    let file = app.workspaces.borrow().current().borrow().focused_file();
                    let start = file.as_deref().and_then(Path::parent).map(Path::to_path_buf).or_else(|| folder.clone())?;
                    Some((start, folder))
                }),
                page_shown: Rc::new(move |page| {
                    let me = m14.clone();
                    // After the open/selection call returns (the panels read the app).
                    glib::idle_add_local_once(move || {
                        let Some(app) = me.upgrade() else { return };
                        if page != settings_ui::Page::ThemeEditor {
                            app.theme_editor.hidden();
                        }
                        match page {
                            settings_ui::Page::Themes => {
                                app.refresh_themes_panel();
                                app.themes_panel.focus();
                            }
                            settings_ui::Page::Extensions => {
                                app.extensions_panel.refresh();
                                app.extensions_panel.focus();
                            }
                            settings_ui::Page::ThemeEditor => app.theme_editor.shown(),
                            settings_ui::Page::General | settings_ui::Page::Toolchains => {}
                        }
                        app.area.queue_draw();
                    });
                }),
                on_close: {
                    let on_close = on_close.clone();
                    Rc::new(move || {
                        if let Some(app) = m18.upgrade() {
                            app.theme_editor.hidden();
                        }
                        on_close();
                    })
                },
            },
        );
        let keybinds_card = keybinds_card::KeybindsCard::new(&overlay, on_close.clone());
        let run_card = run_card::RunCard::new(&overlay, on_close.clone());
        let m15 = me.clone();
        let visualizer = audio::Visualizer::new(Rc::new(move || {
            let Some(app) = m15.upgrade() else { return };
            let shown = app.visualizer.shown().filter(|_| app.config.borrow().sidebar.show_visualizer);
            let k = shown.as_ref().map_or(0.0, |(_, k)| *k);
            // Under the file tree while the drawer is open, else on the rail.
            let closed = app.drawer.try_borrow().is_ok_and(|d| d.extension() <= 0.0);
            app.wave.set_visible(shown.is_some());
            if shown.is_some() && !closed {
                app.wave.queue_draw();
            }
            let on_rail = shown.as_ref().filter(|_| closed && token::rice() == token::Rice::Default);
            app.rail_wave.set_visible(on_rail.is_some());
            if let Some((bars, _)) = on_rail {
                let r = draw::rail_wave_rect(app.area.height() as f64, bars.len());
                let (x, y, w, h) = (r.x.round() as i32, r.y.round() as i32, r.w.round() as i32, r.h.round() as i32);
                if (app.rail_wave.margin_start(), app.rail_wave.margin_top(), app.rail_wave.width_request(), app.rail_wave.height_request()) != (x, y, w, h) {
                    app.rail_wave.set_margin_start(x);
                    app.rail_wave.set_margin_top(y);
                    app.rail_wave.set_size_request(w, h);
                }
                app.rail_wave.queue_draw();
            }
            // The canvas only for the logo fading out and back in.
            let before = app.rail_wave_k.replace(k);
            if before != k && (k < 1.0 || before < 1.0) {
                app.area.queue_draw();
            }
        }));
        // Last, so confirmations sit above every other layer.
        dialog::install(&overlay);
        let cover = gtk::Picture::new();
        cover.set_can_target(false);
        cover.set_can_shrink(true);
        cover.set_content_fit(gtk::ContentFit::Fill);
        cover.set_visible(false);
        overlay.add_overlay(&cover);
        App {
            me: me.clone(),
            options: options.clone(),
            window: window.clone(),
            area: area.clone(),
            drawer: RefCell::new(Drawer::default()),
            drawer_tick: Cell::new(false),
            panel,
            files,
            peek,
            bars,
            workspace_list: Cell::new(true),
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
            theme_syntax: Cell::new(true),
            theme: RefCell::new(theme),
            binds: RefCell::new(Vec::new()),
            css,
            banner,
            problems: RefCell::new(Vec::new()),
            tray,
            overview,
            quick_open,
            cover,
            settings_card,
            keybinds_card,
            run_card,
            launch_pick: RefCell::new(HashMap::new()),
            visualizer,
            wave: wave.clone(),
            rail_wave: rail_wave.clone(),
            rail_wave_k: Cell::new(0.0),
            raw_config: RefCell::new(Config::default()),
            save_timer: RefCell::new(None),
            themes_panel,
            theme_editor,
            tests: RefCell::new(None),
            extensions_panel,
            panel_for: Cell::new(None),
            preview_theme: RefCell::new(None),
            draft_theme: RefCell::new(None),
            draft_count: Cell::new(0),
            session_background: RefCell::new(None),
            dots: RefCell::new(Presence::default()),
            reload_timer: RefCell::new(None),
            _monitors: RefCell::new(Vec::new()),
        }
    });

    {
        // The drawer panel is exactly as wide as the drawer: a size request is only a
        // minimum, and wide rows would push it over the code region.
        let me = Rc::downgrade(&app_state);
        overlay.connect_get_child_position(move |overlay, child| {
            let app = me.upgrade()?;
            if child != app.panel.upcast_ref::<gtk::Widget>() {
                return None;
            }
            let extension = app.drawer.try_borrow().map_or(0.0, |d| d.extension());
            Some(gdk::Rectangle::new(0, 0, (token::left_inset() + extension).round() as i32, overlay.height()))
        });
        // The peek's ×: removes that workspace (asks first if it holds work).
        let me = Rc::downgrade(&app_state);
        app_state.peek.close.connect_clicked(move |_| {
            let Some(app) = me.upgrade() else { return };
            if let Some((n, _)) = app.peek.target.get() {
                hide_peek(&app.peek.widget, &app.peek.target);
                app.close_workspace(n);
            }
        });
    }
    {
        let me = Rc::downgrade(&app_state);
        app_state.workspaces.borrow_mut().set_on_switch(Rc::new(move |info: &workspaces::Switch| me.upgrade().and_then(|app| app.on_switch(info))));
    }
    let start_expanded = config.sidebar.start_expanded && config.sidebar.show_files;
    problems.extend(init_extensions(&app_state));
    init_debug(&app_state);
    init_tests(&app_state);
    let alerts = Rc::downgrade(&app_state);
    alerts::set_hook(Rc::new(move |message| {
        if let Some(app) = alerts.upgrade()
            && app.config.borrow().alerts.missing_tools
        {
            app.notice(message);
        }
    }));
    let (terminal, notice) = (Rc::downgrade(&app_state), Rc::downgrade(&app_state));
    interpreter::set_hooks(interpreter::Hooks {
        terminal: Box::new(move |dir, command| {
            if let Some(app) = terminal.upgrade() {
                let view = app.workspaces.borrow().current();
                view.borrow_mut().add(Kind::Command(dir, command));
            }
        }),
        notice: Box::new(move |message| {
            if let Some(app) = notice.upgrade() {
                app.notice(message);
            }
        }),
    });
    app_state.apply_config(config, problems);
    extensions::event(tileide_extensions::Event::Startup);
    if start_expanded {
        app_state.toggle_drawer();
    }
    if let Some(file) = &options.open {
        let view = app_state.workspaces.borrow().current();
        view.borrow_mut().add(Kind::Editor(Some(file)));
    }

    watch_config(&app_state);
    connect_drawing(&app_state, started);
    trace_frames(window.upcast_ref());
    connect_wave(&app_state);
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
    for (name, at) in options.test_actions.clone() {
        let (arg, name) = match name.split_once(':') {
            Some((n, a)) => (Some(a.to_string()), n.to_string()),
            None => (None, name),
        };
        let me = Rc::downgrade(&app_state);
        let window = window.clone();
        glib::timeout_add_local_once(Duration::from_secs_f64(at), move || match (me.upgrade(), tileide_config::api::parse_action(&name, arg.as_deref())) {
            // `open-here:<path>`: as if picked with Alt+F (replaces the focused editor).
            (Some(app), _) if name == "open-here" => {
                app.open_picked(quick_open::Target::Focused, arg.as_ref().map(PathBuf::from));
            }
            // `open:<path>`: as if picked in quick open (Alt+E, Enter).
            (Some(app), _) if name == "open" => {
                app.open_picked(quick_open::Target::NewTile, arg.as_ref().map(PathBuf::from));
            }
            // `type:<text>`: typed into the focused editor or cell, one character at a time.
            (Some(app), _) if name == "type" => {
                if let Some(editor) = app.workspaces.borrow().current().borrow().focused_code() {
                    for c in arg.as_deref().unwrap_or("").replace("\\n", "\n").chars() {
                        editor.buffer().insert_at_cursor(&c.to_string());
                    }
                }
            }
            // `keys:<text>`: typed through the typing aids (`\b` = Backspace), then the
            // line printed with `|` at the cursor.
            (Some(app), _) if name == "keys" => {
                if let Some(buffer) = app.workspaces.borrow().current().borrow().focused_code().and_then(|e| e.buffer().downcast::<sourceview5::Buffer>().ok()) {
                    for c in arg.as_deref().unwrap_or("").replace("\\b", "\x08").chars() {
                        editing::type_char(&buffer, c);
                    }
                    let at = buffer.iter_at_mark(&buffer.get_insert());
                    let line = buffer.iter_at_line(at.line()).unwrap_or(at);
                    let mut end = at;
                    end.forward_to_line_end();
                    println!("test action: keys → {}|{}", buffer.text(&line, &at, false), buffer.text(&at, &end, false));
                }
            }
            // `where`: the focused file, its cursor (1-based line:column) and its text.
            (Some(app), _) if name == "where" => {
                let current = app.workspaces.borrow().current();
                let tiles = current.borrow();
                if let Some(editor) = tiles.focused_code() {
                    let buffer = editor.buffer();
                    let at = buffer.iter_at_mark(&buffer.get_insert());
                    let file = tiles.focused_file().map(|f| f.display().to_string()).unwrap_or_default();
                    println!("where: {file}:{}:{}\n{}", at.line() + 1, at.line_offset() + 1, buffer.text(&buffer.start_iter(), &buffer.end_iter(), false));
                }
            }
            // `completions`: the open list's first 12 labels, best first.
            (Some(app), _) if name == "completions" => {
                if let Some(editor) = app.workspaces.borrow().current().borrow().focused_code() {
                    println!("completions: {}", lsp::test_completions(&editor).into_iter().take(12).collect::<Vec<_>>().join(" "));
                }
            }
            // `accept`: Enter on the open completion list, then the editor's text.
            (Some(app), _) if name == "accept" => {
                if let Some(editor) = app.workspaces.borrow().current().borrow().focused_code() {
                    lsp::test_accept(&editor);
                    let buffer = editor.buffer();
                    println!("test action: accept →\n{}", buffer.text(&buffer.start_iter(), &buffer.end_iter(), false));
                }
            }
            // `tab`: the Tab key in the focused editor or cell (completes after a name or `.`).
            (Some(app), _) if name == "tab" => {
                if let Some(editor) = app.workspaces.borrow().current().borrow().focused_code() {
                    let completed = lsp::tab_key(&editor);
                    println!("test action: tab {}", if completed { "completes" } else { "indents" });
                }
            }
            // `rename:<name>`: F2 at the cursor, answered with <name>.
            (Some(app), _) if name == "rename" => {
                if let (Some(editor), Some(new)) = (app.workspaces.borrow().current().borrow().focused_code(), arg.as_deref()) {
                    let buffer = editor.buffer();
                    lsp::rename_to(&editor, &buffer.iter_at_mark(&buffer.get_insert()), new);
                }
            }
            // `cell:add` / `cell:delete`: the notebook's "+ Code" / the current cell's delete.
            (Some(app), _) if name == "cell" => {
                let nb = app.workspaces.borrow().current().borrow().some_notebook();
                if let Some(nb) = nb {
                    nb.test_action(arg.as_deref().unwrap_or(""));
                }
            }
            // `cursor:<line>[:<column>]`: the focused editor's cursor, both 1-based.
            (Some(app), _) if name == "cursor" => {
                let editor = app.workspaces.borrow().current().borrow().focused_code();
                let mut parts = arg.as_deref().unwrap_or("").split(':').map(|n| n.parse::<i32>().ok());
                if let (Some(editor), Some(Some(line))) = (editor, parts.next()) {
                    let column = parts.next().flatten().unwrap_or(1);
                    let buffer = editor.buffer();
                    if let Some(it) = buffer.iter_at_line_offset(line - 1, column - 1).or_else(|| buffer.iter_at_line(line - 1)) {
                        buffer.place_cursor(&it);
                    }
                }
            }
            // `set:<key>=<number|true|false>`: a Settings control changed to that value.
            (Some(app), _) if name == "set" => {
                use tileide_config::settings::Value;
                if let Some((key, value)) = arg.as_deref().and_then(|a| a.split_once('=')) {
                    let value = match value {
                        "true" | "false" => Value::Bool(value == "true"),
                        v => v.parse().map_or_else(|_| Value::Text(v.into()), Value::Number),
                    };
                    app.change_setting(key, value);
                }
            }
            // `syntax`: the current theme's keyword and string colours.
            (Some(app), _) if name == "syntax" => {
                let s = app.theme.borrow().syntax();
                println!("syntax: keyword {:?} string {:?}", s.keyword, s.string);
            }
            // `pick-folder:<path>`: the folder search's pick; prints whether the file tree shows.
            (Some(app), _) if name == "pick-folder" => {
                app.open_picked(quick_open::Target::Folder, arg.map(PathBuf::from));
                println!("pick-folder: drawer opens = {}", app.drawer.borrow().opens());
            }
            // `background-state`: the background in effect for all workspaces.
            (Some(app), _) if name == "background-state" => println!("background-state: {:?}", app.background_choice(None, None).map(|b| format!("{:?}", b.source))),
            // `background:<mode>[:<source>]`: a pick for all workspaces, as the Themes page makes it.
            (Some(app), _) if name == "background" => {
                let (mode, source) = arg.as_deref().unwrap_or("see-through").split_once(':').map_or((arg.as_deref().unwrap_or("see-through"), None), |(m, s)| (m, Some(s.to_string())));
                let bg = tileide_config::model::Background { mode: mode.into(), source, ..app.config.borrow().background.clone() };
                app.apply_background(bg, themes_panel::Scope::All);
                println!("background: {:?}", app.background_choice(None, None).map(|b| format!("{:?}", b.source)));
            }
            // `settings-page`: the page Settings shows, or None when closed.
            (Some(app), _) if name == "settings-page" => println!("settings-page: {:?}", app.settings_card.page()),
            // `tiles`: the workspace's tile rects, printed.
            (Some(app), _) if name == "files" => app.files.borrow_mut().test_action(arg.as_deref().unwrap_or("rows")),
            (Some(app), _) if name == "themes-select" => app.themes_panel.test_select(arg.and_then(|a| a.parse().ok()).unwrap_or(0)),
            (Some(app), _) if name == "tiles" => println!("tiles: {}", app.workspaces.borrow().current().borrow().describe()),
            // `diagnostics`: the focused editor's or cell's diagnostics, printed.
            (Some(app), _) if name == "hints" => {
                if let Some(buffer) = app.workspaces.borrow().current().borrow().focused_code().and_then(|e| e.buffer().downcast::<sourceview5::Buffer>().ok()) {
                    println!("hints: {}", lsp::hint_lines(&buffer).join(" | "));
                }
            }
            _ if name == "debug-state" => println!("debug-state: {}", debug::describe()),
            (Some(app), _) if name == "diagnostics" => {
                if let Some(editor) = app.workspaces.borrow().current().borrow().focused_code() {
                    let buffer = editor.buffer().downcast::<sourceview5::Buffer>().ok();
                    for message in buffer.map(|b| lsp::all_messages(&b)).unwrap_or_default() {
                        println!("diagnostic {message}");
                    }
                }
            }
            // `unlock:<key>`: Settings' Unlock button for that key.
            (Some(app), _) if name == "unlock" => {
                if let Some(key) = tileide_config::settings::KEYS.iter().map(|(k, _)| *k).find(|k| Some(*k) == arg.as_deref()) {
                    app.unlock_setting(key);
                }
            }
            (Some(app), _) if name == "tests" => {
                if let Some(t) = app.tests.borrow().clone() {
                    t.test_action(arg.as_deref().unwrap_or(""));
                }
            }
            // `tests-state`: the Tests tile's summary and rows.
            (Some(app), _) if name == "tests-state" => {
                if let Some(t) = app.tests.borrow().as_ref() {
                    println!("tests-state: {}", t.describe());
                }
            }
            // `pick:<letters>[|N]`: typed into the open filtered list (the outline), its
            // rows printed, then row N of them picked.
            _ if name == "pick" => {
                let (query, n) = arg.as_deref().unwrap_or("").split_once('|').map_or((arg.as_deref().unwrap_or(""), None), |(q, n)| (q, n.parse().ok()));
                println!("pick: {}", pick_card::test(query, n).join(" | "));
            }
            // `pythons[:N]`: the Python picker's rows for the focused file, then row N picked.
            (Some(app), _) if name == "pythons" => {
                let folder = {
                    let w = app.workspaces.borrow();
                    w.folder(w.active())
                };
                if let Some(file) = app.workspaces.borrow().current().borrow().focused_file() {
                    interpreter::test(&file, folder.as_deref(), arg.as_deref().and_then(|n| n.parse().ok()));
                }
            }
            (Some(app), _) if name == "editor" => println!("editor: {} · app accent {:?}", app.theme_editor.test(arg.as_deref().unwrap_or("")), app.theme.borrow().accent),
            (Some(app), _) if name == "export-setup" => app.export_setup_to(Path::new(arg.as_deref().unwrap_or("tileide-setup.tar.gz"))),
            (Some(app), _) if name == "settings-nav" => {
                println!("settings-nav: {}", {
                    let (row, words) = arg.as_deref().unwrap_or("0").split_once(' ').unwrap_or((arg.as_deref().unwrap_or("0"), ""));
                    app.settings_card.test_nav(row.parse().unwrap_or(0), words)
                });
            }
            // `screenshot:<file.png>`: the window as rendered (headless checks).
            _ if name == "screenshot" => {
                if let Err(e) = screenshot(window.upcast_ref(), arg.as_deref().unwrap_or("tileide.png")) {
                    eprintln!("test action: screenshot: {e}");
                }
            }
            (Some(app), Ok(action)) => {
                println!("test action: {name}");
                app.run(&action);
            }
            (_, Err(e)) => eprintln!("test action: {e}"),
            _ => {}
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

/// The first line of theme files the editor writes; Save replaces only those.
const THEME_EDITOR_HEADER: &str = "-- Made in TileIDE's theme editor.";

fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(|| "/".into(), |n| n.to_string_lossy().into_owned())
}

impl App {
    fn scene<'a>(&self, numbers: &'a [(u8, f64)], clock: &'a (String, String)) -> draw::Scene<'a> {
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
                icon_backgrounds: s.icon_backgrounds,
                pills: s.pills,
            },
            themes_open: self.tray.is_open(),
            visualizer: None,
            settings_open: self.settings_card.is_open(),
            solid: self.options.solid,
            drawer: self.drawer.borrow().extension(),
            push: self.options.push,
            workspaces: numbers,
            active: match workspaces.active() {
                Active::Number(n) => Some(n),
                Active::Config => None,
            },
            clock: (&clock.0, &clock.1),
            empty: workspaces.current().borrow().is_empty(),
            placeholder_tiles: false,
            background: None,
            frosted: None,
            frosted_frame: None,
            frame_opacity: self.surfaces().sidebar.opacity,
            tile_rects: &[],
            tile_opacity: self.tile_opacity(),
        }
    }

    fn geometry(&self) -> Geometry {
        let config = self.config.borrow();
        Geometry {
            tile_radius: config.appearance.window_radius as f64,
            gaps_in: config.tiling.gaps_in as f64,
            gaps_out: config.tiling.gaps_out as f64,
            tile_border: config.appearance.tile_border,
            tile_edge: config.appearance.tile_border_color,
            frame_border: config.appearance.frame_border,
            frame_edge: config.appearance.frame_border_color,
        }
    }

    /// The rail's dots with their presence now; follows the workspaces that exist.
    fn rail_dots(&self) -> Vec<(u8, f64)> {
        let numbers = self.workspaces.borrow().numbers();
        let now = self.area.frame_clock().map_or(0, |c| c.frame_time());
        let mut dots = self.dots.borrow_mut();
        dots.sync(&numbers, now, motion::enabled());
        dots.at(now)
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
        // Editing `tileide.background` replaces this session's Settings pick (the saved one
        // yields too: `settings::saved_background`).
        if config.background != self.raw_config.borrow().background {
            *self.session_background.borrow_mut() = None;
        }
        *self.binds.borrow_mut() = binds;
        let me = self.me.clone();
        let notice: Rc<dyn Fn(String)> = Rc::new(move |message| {
            if let Some(app) = me.upgrade() {
                app.notice(message);
            }
        });
        tile_view::set_notice(notice.clone());
        problems.extend(lsp::configure(&config.lsp, notice));
        *self.raw_config.borrow_mut() = config;
        *self.problems.borrow_mut() = problems;
        self.refresh_config();
    }

    /// The config with the GUI's settings applied (`state.lua`), made live.
    fn refresh_config(&self) {
        let config = tileide_config::settings::apply(self.raw_config.borrow().clone(), &self.state.borrow().settings);
        let a = &config.appearance;
        motion::set_enabled(a.animations);
        editing::configure(config.editor.auto_close, config.editor.match_brackets);
        tile_view::set_limit_alert(config.alerts.tile_limit);
        tile_view::set_format_on_save(config.editor.format_on_save);
        lsp::set_inlay_hints(config.editor.inlay_hints);
        tileide_core::motion::set_profile(a.motion, a.motion_speed);
        token::set_ui_font(&a.ui_font, a.ui_scale);
        token::set_rail(config.sidebar.scale, config.sidebar.icon_scale);
        let entering_wiki = a.rice == token::Rice::Wikipedia && token::rice() != a.rice;
        token::set_rice(a.rice, self.workspace_list.get());
        self.bars.show(a.rice == token::Rice::Wikipedia, self.workspace_list.get());
        self.workspaces.borrow_mut().set_rice(a.rice);
        // The Wikipedia rice's left sidebar is Files: open it on the way in.
        if entering_wiki && !self.drawer.borrow().is_open() {
            self.slide_drawer();
        }
        self.visualizer.set_enabled(config.sidebar.show_visualizer);
        // Widgets placed beside the rail (or the Wikipedia rice's bars) follow its size.
        let left = token::left_inset();
        self.panel.set_margin_start(left as i32);
        self.panel.set_margin_top(token::top_inset() as i32);
        self.banner.set_margin_start((left + 16.0) as i32);
        self.banner.set_margin_top((token::top_inset() + 10.0) as i32);
        self.peek.widget.set_margin_start((token::rail_width() + PEEK_GAP) as i32);
        let (tiling, per_workspace) = (config.tiling.clone(), config.workspaces.clone());
        if self.settings_card.is_open() {
            self.settings_card.refresh(&config);
        }
        *self.config.borrow_mut() = config;
        // Re-roots the tree and refreshes theme, background, CSS and banner (`on_switch`).
        self.workspaces.borrow_mut().configure(tiling, per_workspace);
        self.sync();
        self.area.queue_draw();
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
        self.files.borrow_mut().set_root(info.root.as_deref().unwrap_or(Path::new("")));
        extensions::set_workspace(info.number, info.root.clone());
        if info.ask_folder {
            // After the switch's borrow ends; the new workspace's fade-in runs under it.
            let me = self.me.clone();
            glib::idle_add_local_once(move || {
                if let Some(app) = me.upgrade() {
                    app.open_folder_search();
                }
            });
        }
        let theme = self.refresh_theme(info.theme.as_deref(), info.number);
        self.refresh_background(info.background.as_deref(), info.number);
        // A workspace change shows that workspace's picks; a preview must not rebuild the
        // list it is browsing.
        if self.themes_shown() && self.panel_for.get() != Some(info.number) {
            // After the switch's borrow ends: the snapshot reads the workspaces.
            let me = self.me.clone();
            glib::idle_add_local_once(move || {
                if let Some(app) = me.upgrade() {
                    app.refresh_themes_panel();
                }
            });
        }
        self.reload_css();
        self.show_problems();
        self.area.queue_draw();
        theme
    }

    /// `on_switch` from outside a `Workspaces` borrow (e.g. after a tray pick).
    /// Refreshes the Themes & background page after the current borrows end.
    fn refresh_themes_panel_soon(&self) {
        if !self.themes_shown() {
            return;
        }
        let me = self.me.clone();
        glib::idle_add_local_once(move || {
            if let Some(app) = me.upgrade() {
                app.refresh_themes_panel();
            }
        });
    }

    fn refresh(&self) {
        let info = self.workspaces.borrow().switch_info();
        if let Some(theme) = self.on_switch(&info) {
            self.workspaces.borrow_mut().set_theme(theme);
        }
    }

    /// The theme for the active workspace; `Some` when it changed.
    fn refresh_theme(&self, workspace: Option<&str>, number: Option<u8>) -> Option<Rc<Theme>> {
        let draft = self.draft_theme.borrow().clone();
        let preview = self.preview_theme.borrow().clone();
        let name = match &draft {
            // Not a file stem: no theme file can have it.
            Some(_) => format!("\u{1}draft {}", self.draft_count.get()),
            None => self.theme_for(preview.as_deref(), workspace, number),
        };
        let match_syntax = self.config.try_borrow().map_or(self.theme_syntax.get(), |c| c.editor.theme_syntax);
        if *self.theme_name.borrow() == name && self.theme_syntax.get() == match_syntax {
            return None;
        }
        let mut theme = match draft {
            Some(draft) => (*draft).clone(),
            None => {
                let (theme, problem) = self.paths.theme(&name);
                if let Some(problem) = problem {
                    self.problems.borrow_mut().push(problem);
                }
                theme
            }
        };
        theme.match_syntax = match_syntax;
        self.theme_syntax.set(match_syntax);
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

    /// The theme name for workspace `number` (its config theme is `workspace`).
    fn theme_for(&self, preview: Option<&str>, workspace: Option<&str>, number: Option<u8>) -> String {
        let state = self.state.borrow();
        let config = self.config.borrow();
        let session = self.session_theme.borrow();
        settings::theme_name(&settings::ThemeSources {
            preview,
            config_workspace: workspace,
            picked_workspace: number.and_then(|n| state.workspaces.get(&n)?.theme.as_deref()),
            session: session.as_deref(),
            config: config.appearance.theme.as_deref(),
            state: state.theme.as_deref(),
        })
    }

    /// The background model in effect for workspace `number` (`settings::background_from`).
    fn background_choice(&self, image: Option<&str>, number: Option<u8>) -> Result<background::Settings, String> {
        let state = self.state.borrow();
        let config = self.config.borrow();
        let session = self.session_background.borrow();
        let picked = number.and_then(|n| state.workspaces.get(&n)?.background.as_ref());
        match settings::background_from(image, picked, session.as_ref(), self.options.background.is_some(), settings::saved_background(&state, &self.raw_config.borrow())) {
            settings::BackgroundFrom::Config(image) => settings::background(&config.background, image),
            settings::BackgroundFrom::Picked(b) => settings::background(b, None),
            settings::BackgroundFrom::CommandLine => Ok(self.options.background.clone().expect("checked")),
        }
    }

    /// Full-screen only (`background_choice` decides which one).
    fn refresh_background(&self, image: Option<&str>, number: Option<u8>) {
        if self.options.mode != Mode::Fullscreen {
            return;
        }
        let settings = self.background_choice(image, number);
        let mut settings = settings.unwrap_or_else(|e| {
            self.problems.borrow_mut().push(e);
            background::Settings::default()
        });
        // Sidebar and tiles: their opacity drives legibility, their blur the backdrops.
        let surfaces = self.surfaces_with(settings.source != background::Source::SeeThrough, settings.tile_opacity);
        settings.tile_opacity = surfaces.tiles.opacity;
        settings.tile_blur = surfaces.tiles.blur;
        settings.frame_blur = surfaces.sidebar.blur;
        // See-through with blur: the backdrops come from the desktop's wallpaper, the one
        // thing behind a full-screen TileIDE.
        if settings.source == background::Source::SeeThrough
            && (settings.tile_blur >= 1.0 || settings.frame_blur >= 1.0)
            && let Some(wallpaper) = background::wallpaper()
        {
            settings.source = background::Source::Wallpaper(wallpaper);
        }
        let key = Some((settings.clone(), self.theme_name.borrow().clone()));
        if *self.background_key.borrow() == key {
            return;
        }
        *self.background_key.borrow_mut() = key;
        draw::set_tile_backdrop(settings.source != background::Source::SeeThrough && settings.tile_blur >= 1.0);
        let old = self.background.borrow_mut().take();
        let mut new = background::Background::new(settings, (**self.theme.borrow()).clone());
        if let Some(b) = &mut new {
            b.keep_image(old);
        }
        *self.background.borrow_mut() = new;
        load_background(self);
    }

    fn tile_opacity(&self) -> f64 {
        self.surfaces().tiles.opacity
    }

    /// Settings → Themes & background → Sidebar / Tiles & terminals, in effect now.
    fn surfaces(&self) -> tileide_core::theme::Surfaces {
        let legacy = self.background_key.borrow().as_ref().map_or(self.config.borrow().background.tile_opacity, |(s, _)| s.tile_opacity);
        let see_behind = self.background.borrow().as_ref().is_some_and(background::Background::paints);
        self.surfaces_with(see_behind, legacy)
    }

    /// `see_behind`: TileIDE paints the background; `legacy`: `background.tile_opacity`.
    fn surfaces_with(&self, see_behind: bool, legacy: f64) -> tileide_core::theme::Surfaces {
        let config = self.config.borrow();
        let choice = tileide_core::theme::SurfaceChoice {
            sidebar_transparency: config.sidebar.transparency,
            sidebar_blur: config.sidebar.blur,
            tile_transparency: config.appearance.tile_transparency,
            tile_blur: config.appearance.tile_blur,
        };
        self.theme.borrow().surfaces(choice, legacy, see_behind)
    }

    fn reload_css(&self) {
        let theme = self.theme.borrow();
        let t = &**theme;
        let config = self.config.borrow();
        let css = format!(
            "window, window.background {{ background: transparent; font-family: \"{font}\"; }}
.banner {{ background-color: {bg}; color: {text}; border: 1px solid {edge}; border-radius: 10px; padding: 8px 14px; font-size: 13px; }}
{tiles}\n{files}\n{wiki}\n{overview}\n{quick_open}\n{dialog}\n{lsp}\n{completion}\n{themes}\n{extensions}\n{settings}\n{run}\n{notebook}\n{tests}\n{debug}
.tile textview, .tile vte-terminal, .notebook .output, .notebook .frame-table label {{ font-family: \"{code_font}\"; font-size: {code_size}pt; }}
.notebook .cell .rendered {{ font-size: {code_size}pt; }}",
            font = token::UI_FONT,
            bg = theme::css(t.surface, 0.96),
            text = theme::css(t.text, 1.0),
            edge = theme::css(t.error, 0.8),
            tiles = tile_view::css(t, self.geometry(), self.tile_opacity()),
            files = file_tree::css(t),
            wiki = wiki::css(t),
            overview = overview::css(t),
            quick_open = quick_open::css(t),
            dialog = dialog::css(t),
            lsp = lsp::css(t),
            completion = completion::css(t),
            themes = themes_panel::css(t),
            extensions = extensions_panel::css(t),
            settings = settings_ui::css(t),
            run = run_card::css(t),
            notebook = notebook_view::css(t),
            tests = tests_view::css(t),
            debug = debug::css(t),
            code_font = config.appearance.code_font.replace('"', ""),
            code_size = config.appearance.code_size,
        );
        self.css.load_from_string(&interface_css(&css, &config.appearance.ui_font, config.appearance.ui_scale));
        self.run_card.set_theme(t);
        debug::set_theme(t);
    }

    /// `Alt+Shift+R`: in a test file, the test at the cursor (or the whole file);
    /// elsewhere every test of the project. The Tests tile shows the results.
    fn run_tests(&self) {
        let Some(tests) = self.tests.borrow().clone() else { return };
        let folder = {
            let w = self.workspaces.borrow();
            w.folder(w.active())
        };
        let view = self.workspaces.borrow().current();
        let (file, editor) = (view.borrow().focused_file(), view.borrow().focused_code());
        let project = match (&file, &folder) {
            (Some(f), _) => tileide_core::python::project(f, folder.as_deref()),
            (None, Some(f)) => f.clone(),
            (None, None) => {
                self.notice("Open a project folder (Alt+D) or a test file to run its tests.".into());
                return;
            }
        };
        let dir = pytest::run_dir(file.as_deref(), &project);
        let ids = match (&file, editor) {
            (Some(f), Some(editor)) if pytest::is_test_file(f) => {
                let rel = f.strip_prefix(&dir).unwrap_or(f).to_string_lossy().into_owned();
                let buffer = editor.buffer();
                let line = buffer.iter_at_mark(&buffer.get_insert()).line() as usize;
                let source = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
                vec![pytest::test_at(&source, line, &rel).unwrap_or(rel)]
            }
            _ => Vec::new(),
        };
        let widget: gtk::Widget = tests.widget().clone().upcast();
        if !view.borrow().shows(&widget) {
            view.borrow_mut().add(Kind::Panel(widget));
        }
        tests.start(&dir, folder.as_deref(), ids);
    }

    /// A message on the banner (kept with the config problems until clicked away).
    fn notice(&self, message: String) {
        let mut problems = self.problems.borrow_mut();
        if !problems.contains(&message) {
            problems.push(message);
        }
        drop(problems);
        self.show_problems();
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

    fn themes_shown(&self) -> bool {
        self.settings_card.page() == Some(settings_ui::Page::Themes)
    }

    /// Settings on `page`; closes it when it already shows that page.
    fn toggle_settings(&self, page: settings_ui::Page) {
        if self.settings_card.page() == Some(page) {
            self.settings_card.close();
            return;
        }
        self.settings_card.open(page, &self.config.borrow());
        self.area.queue_draw();
    }

    fn toggle_extensions_section(&self) {
        self.toggle_settings(settings_ui::Page::Extensions);
    }

    fn open_keybinds(&self) {
        let binds = self.config.borrow().effective_binds();
        self.keybinds_card.open(&binds);
    }

    /// A Settings control changed: applied at once, saved shortly after.
    fn change_setting(&self, key: &str, value: tileide_config::settings::Value) {
        self.state.borrow_mut().settings.insert(key.to_string(), value);
        self.schedule_save();
        self.refresh_config();
    }

    /// Settings' "Unlock": comments the key's line out in `tileide.lua`, keeping its
    /// value as a GUI setting (so nothing jumps), then reloads.
    fn unlock_setting(&self, key: &'static str) {
        let file = self.paths.file.clone();
        let source = match std::fs::read_to_string(&file) {
            Ok(s) => s,
            Err(e) => return self.notice(format!("cannot read {}: {e}", file.display())),
        };
        let Some(released) = tileide_config::settings::release(&source, key) else {
            return self.notice(format!("Settings can't unlock {key} by itself (it shares a line with other keys, or a rice sets it). Remove it from {} to change it here.", file.display()));
        };
        let value = tileide_config::settings::get(&self.config.borrow(), key);
        if let Some(value) = value {
            self.state.borrow_mut().settings.insert(key.to_string(), value);
        }
        self.save_state();
        if let Err(e) = std::fs::write(&file, released) {
            return self.notice(format!("cannot write {}: {e}", file.display()));
        }
        self.reload();
    }

    fn schedule_save(&self) {
        if let Some(t) = self.save_timer.borrow_mut().take() {
            t.remove();
        }
        let me = self.me.clone();
        let timer = glib::timeout_add_local_once(SAVE_DEBOUNCE, move || {
            if let Some(app) = me.upgrade() {
                app.save_timer.borrow_mut().take();
                app.save_state();
            }
        });
        *self.save_timer.borrow_mut() = Some(timer);
    }

    /// `Alt+R`: runs the focused file (saved first), or the workspace's Cargo project,
    /// on the run card. With nothing to run, runs the last plan again.
    fn run_focused(&self) {
        let view = self.workspaces.borrow().current();
        // A notebook runs its cells in its own kernel.
        let notebook = view.borrow().focused_notebook();
        if let Some(nb) = notebook {
            nb.run_all();
            return;
        }
        let file = view.borrow().focused_file();
        if file.is_some() && view.borrow().focused_dirty() {
            view.borrow_mut().save_focused(self.window.upcast_ref());
        }
        let folder = {
            let w = self.workspaces.borrow();
            w.folder(w.active())
        };
        let out = glib::user_runtime_dir().join("tileide-run");
        let _ = std::fs::create_dir_all(&out);
        let plan = match self.picked_launch(file.as_deref(), folder.as_deref()) {
            Some(Ok(launch)) => tileide_core::run::plan_launch(&launch, folder.as_deref(), &self.config.borrow().runners, &out, tileide_core::run::on_path),
            Some(Err(e)) => Err(e),
            None => tileide_core::run::plan(file.as_deref(), folder.as_deref(), &self.config.borrow().runners, &out, tileide_core::run::on_path),
        };
        match plan {
            Ok(plan) => self.run_card.run(plan),
            Err(_) if file.is_none() && self.run_card.rerun() => {}
            Err(e) => self.run_card.show_message("Run", &e),
        }
    }

    /// The launch configs of the active workspace: `tileide.lua`'s, then its folder's
    /// `.vscode/launch.json` (with what that file has that can't be used, or its error).
    fn launch_configs(&self, folder: Option<&Path>) -> (Vec<(tileide_core::launch::Launch, &'static str)>, Option<String>) {
        let mut configs: Vec<_> = self.config.borrow().launches.iter().map(|l| (l.clone(), "tileide.lua")).collect();
        let Some(text) = folder.and_then(|f| std::fs::read_to_string(f.join(".vscode/launch.json")).ok()) else { return (configs, None) };
        match tileide_config::vscode::launch_configs(&text) {
            Ok((found, skipped)) => {
                configs.extend(found.into_iter().map(|l| (l, "launch.json")));
                let note = (!skipped.is_empty()).then(|| format!("launch.json: skipped {} (attach, module or no program)", skipped.join(", ")));
                (configs, note)
            }
            Err(e) => (configs, Some(e)),
        }
    }

    /// The launch config picked for this workspace (Alt+L), resolved; `None` = none picked.
    fn picked_launch(&self, file: Option<&Path>, folder: Option<&Path>) -> Option<Result<tileide_core::launch::Resolved, String>> {
        let Active::Number(n) = self.workspaces.borrow().active() else { return None };
        let name = self.launch_pick.borrow().get(&n).cloned()?;
        let Some((launch, _)) = self.launch_configs(folder).0.into_iter().find(|(l, _)| l.name == name) else {
            return Some(Err(format!("The launch config \"{name}\" is gone; Alt+L picks another.")));
        };
        Some(launch.resolve(&tileide_core::launch::Context { folder, file }))
    }

    /// `Alt+L`: picks what Alt+R and F5 launch in this workspace: a launch config, or the
    /// focused file (the default).
    fn pick_launch(&self) {
        let Active::Number(n) = self.workspaces.borrow().active() else { return };
        let folder = self.workspaces.borrow().folder(Active::Number(n));
        let (configs, note) = self.launch_configs(folder.as_deref());
        if let Some(note) = note {
            self.notice(note);
        }
        if configs.is_empty() {
            self.notice("No launch configs: add tileide.launch { name = …, program = … } to tileide.lua, or a .vscode/launch.json to the workspace folder.".into());
            return;
        }
        let Some(editor) = self.workspaces.borrow().current().borrow().focused_code() else {
            self.notice("Focus an editor, then Alt+L to pick a launch config.".into());
            return;
        };
        let current = self.launch_pick.borrow().get(&n).cloned();
        let mark = |on: bool| if on { "● " } else { "   " };
        let mut rows = vec![format!("{}Focused file (no config)", mark(current.is_none()))];
        rows.extend(configs.iter().map(|(l, from)| format!("{}{} · {} ({from})", mark(current.as_deref() == Some(l.name.as_str())), l.name, l.program)));
        let names: Vec<String> = configs.into_iter().map(|(l, _)| l.name).collect();
        let me = self.me.clone();
        pick_card::show(&editor, "Alt+R and F5 launch", &rows, move |i| {
            let Some(app) = me.upgrade() else { return };
            match i.checked_sub(1).and_then(|i| names.get(i)) {
                Some(name) => {
                    app.launch_pick.borrow_mut().insert(n, name.clone());
                    app.notice(format!("Alt+R and F5 launch \"{name}\" in this workspace."));
                }
                None => {
                    app.launch_pick.borrow_mut().remove(&n);
                    app.notice("Alt+R and F5 launch the focused file.".into());
                }
            }
        });
    }

    /// `Alt+=` / `Alt+-`: the code font of every tile, one point at a time.
    fn step_font_size(&self, by: i8) {
        if self.config.borrow().explicit.contains("appearance.code_size") {
            self.notice("tileide.lua sets appearance.code_size: change it there, or Unlock it in Settings (Alt+,).".into());
            return;
        }
        let (min, max) = tileide_config::settings::CODE_SIZE;
        let size = (self.config.borrow().appearance.code_size + by as f64).clamp(min, max);
        self.change_setting("appearance.code_size", tileide_config::settings::Value::Number(size));
    }

    /// Runs an extension command on the focused editor (its file, language and text).
    fn run_extension_command(&self, command: &str) {
        let view = self.workspaces.borrow().current();
        let view = view.borrow();
        let mut context = extensions::Context { file: view.focused_file(), ..Default::default() };
        if let Some(editor) = view.focused_editor() {
            let buffer = editor.buffer();
            context.text = Some(buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string());
            context.language = buffer.downcast_ref::<sourceview5::Buffer>().and_then(sourceview5::prelude::BufferExt::language).map(|l| l.id().to_string());
        }
        drop(view);
        extensions::run_command(command, context);
    }

    /// Alt+Shift+T: Settings on Themes & background.
    fn toggle_themes_section(&self) {
        self.toggle_settings(settings_ui::Page::Themes);
    }

    fn refresh_themes_panel(&self) {
        let (number, workspace_theme, image) = {
            let w = self.workspaces.borrow();
            let number = match w.active() {
                Active::Number(n) => Some(n),
                Active::Config => None,
            };
            let (theme, image) = w.overrides();
            (number, theme, image)
        };
        self.panel_for.set(Some(number));
        let state = self.state.borrow().clone();
        let config = self.config.borrow().clone();
        let picks = number.and_then(|n| state.workspaces.get(&n)).filter(|p| p.theme.is_some() || p.background.is_some());
        let scope = match (number, picks) {
            (Some(n), Some(_)) => themes_panel::Scope::Workspace(n),
            _ => themes_panel::Scope::All,
        };
        let session = self.session_background.borrow().clone();
        let background = match settings::background_from(image.as_deref(), picks.and_then(|p| p.background.as_ref()), session.as_ref(), self.options.background.is_some(), settings::saved_background(&state, &self.raw_config.borrow())) {
            settings::BackgroundFrom::Picked(b) => b.clone(),
            _ => config.background.clone(),
        };
        let mut notes = Vec::new();
        if let Some(own) = &workspace_theme {
            notes.push(format!("tileide.lua gives this workspace the theme \"{own}\"; picks show in other workspaces."));
        } else if let Some(pinned) = &config.appearance.theme {
            notes.push(format!("tileide.lua sets theme = \"{pinned}\": a pick for all workspaces lasts until restart."));
        }
        if image.is_some() {
            notes.push("tileide.lua gives this workspace a background image.".into());
        } else if config.background_set {
            notes.push("tileide.lua sets a background: a pick for all workspaces lasts until restart.".into());
        }
        self.themes_panel.refresh(themes_panel::Snapshot {
            themes: self.paths.all_themes(),
            current: self.theme_for(None, workspace_theme.as_deref(), number),
            workspace: number,
            scope,
            background,
            fullscreen: self.options.mode == Mode::Fullscreen,
            note: (!notes.is_empty()).then(|| notes.join(" ")),
            surfaces: self.surfaces(),
            can_blur: self.options.mode == Mode::Fullscreen && (self.background.borrow().is_some() || background::wallpaper().is_some()),
            locked: themes_panel::SURFACE_KEYS.into_iter().filter(|k| self.config.borrow().explicit.contains(*k)).collect(),
        });
    }

    fn preview(&self, key: Option<&str>) {
        if self.preview_theme.borrow().as_deref() == key {
            return;
        }
        *self.preview_theme.borrow_mut() = key.map(String::from);
        self.refresh();
    }

    /// The theme the active workspace uses, without a preview or the editor's draft.
    fn theme_in_use(&self) -> Theme {
        let (number, workspace_theme) = {
            let w = self.workspaces.borrow();
            let number = match w.active() {
                Active::Number(n) => Some(n),
                Active::Config => None,
            };
            (number, w.overrides().0)
        };
        self.paths.theme(&self.theme_for(None, workspace_theme.as_deref(), number)).0
    }

    /// The theme editor's draft shown live; `None` goes back to the theme in use.
    fn preview_draft(&self, draft: Option<Theme>) {
        if draft.is_none() && self.draft_theme.borrow().is_none() {
            return;
        }
        self.draft_count.set(self.draft_count.get().wrapping_add(1));
        *self.draft_theme.borrow_mut() = draft.map(Rc::new);
        self.refresh();
    }

    /// Theme editor → Save: writes `themes/<slug>.lua` (never over a file the editor
    /// didn't write) and picks it for all workspaces.
    fn save_draft_theme(&self, theme: &Theme) {
        let dir = self.paths.dir.join("themes");
        let stem = tileide_config::vscode::slug(&theme.name);
        let ours = |p: &Path| std::fs::read_to_string(p).is_ok_and(|t| t.starts_with(THEME_EDITOR_HEADER));
        let mut target = dir.join(format!("{stem}.lua"));
        let mut n = 2;
        while target.exists() && !ours(&target) {
            target = dir.join(format!("{stem}-{n}.lua"));
            n += 1;
        }
        let written = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&target, tileide_config::theme_lua(theme, &THEME_EDITOR_HEADER[3..])));
        let file = target.file_name().map_or_else(String::new, |f| f.to_string_lossy().into_owned());
        if let Err(e) = written {
            self.theme_editor.set_status(&format!("Cannot save {}: {e}", target.display()));
            return;
        }
        *self.draft_theme.borrow_mut() = None;
        self.apply_theme(file.trim_end_matches(".lua"), themes_panel::Scope::All);
        self.theme_editor.saved(theme.clone(), &format!("Saved as themes/{file} and in use. Pick it any time in Themes & background."));
    }

    /// Theme editor → Export…: the theme file wherever the user picks.
    fn export_theme(&self, theme: Theme) {
        let name = format!("{}.lua", tileide_config::vscode::slug(&theme.name));
        let me = self.me.clone();
        gtk::FileDialog::builder().title("Export the theme").initial_name(name).build().save(Some(&self.window), None::<&gio::Cancellable>, move |result| {
            let (Some(app), Some(path)) = (me.upgrade(), result.ok().and_then(|f| f.path())) else { return };
            let message = match std::fs::write(&path, tileide_config::theme_lua(&theme, &THEME_EDITOR_HEADER[3..])) {
                Ok(()) => format!("Exported to {}. To use it elsewhere, put it in ~/.config/tileide/themes/.", path.display()),
                Err(e) => format!("Cannot export to {}: {e}", path.display()),
            };
            app.theme_editor.set_status(&message);
        });
    }

    /// Settings → Share → Export setup.
    fn export_setup(&self) {
        let me = self.me.clone();
        let name = format!("tileide-setup-{}.tar.gz", glib::DateTime::now_local().ok().and_then(|d| d.format("%Y-%m-%d").ok()).map_or_else(String::new, |d| d.to_string()));
        gtk::FileDialog::builder().title("Export your setup").initial_name(name).build().save(Some(&self.window), None::<&gio::Cancellable>, move |result| {
            if let (Some(app), Some(path)) = (me.upgrade(), result.ok().and_then(|f| f.path())) {
                app.export_setup_to(&path);
            }
        });
    }

    fn export_setup_to(&self, out: &Path) {
        let mut images: Vec<PathBuf> = Vec::new();
        {
            let config = self.raw_config.borrow();
            let state = self.state.borrow();
            let picked = state.background.iter().chain(state.workspaces.values().filter_map(|w| w.background.as_ref()));
            for bg in std::iter::once(&config.background).chain(picked).filter(|b| b.mode == "image") {
                images.extend(bg.source.as_deref().map(workspaces::expand_home));
            }
            images.extend(config.workspaces.values().filter_map(|w| w.background.as_deref()).map(workspaces::expand_home));
        }
        images.dedup();
        let mut extensions: Vec<String> = std::fs::read_dir(extensions::default_root()).map(|d| d.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
        extensions.sort();
        let note = match setup_export::export(&self.paths.dir, &images, &extensions, out) {
            Ok(done) => {
                let mut note = format!("Exported your setup ({} files) to {}.", done.files, out.display());
                if !done.missing.is_empty() {
                    note += &format!(" Not found, so left out: {}.", done.missing.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "));
                }
                note
            }
            Err(e) => format!("Cannot export the setup: {e}"),
        };
        println!("export-setup: {note}");
        *self.problems.borrow_mut() = vec![note];
        self.show_problems();
    }

    /// A Themes section pick. For all workspaces it behaves like the tray's.
    fn apply_theme(&self, key: &str, scope: themes_panel::Scope) {
        *self.preview_theme.borrow_mut() = None;
        match scope {
            themes_panel::Scope::All => {
                if let Active::Number(n) = self.workspaces.borrow().active()
                    && let Some(pick) = self.state.borrow_mut().workspaces.get_mut(&n)
                {
                    pick.theme = None;
                }
                self.pick_theme(key);
            }
            themes_panel::Scope::Workspace(n) => {
                self.state.borrow_mut().workspaces.entry(n).or_default().theme = Some(key.into());
                self.save_state();
                self.refresh();
            }
        }
        self.refresh_themes_panel();
    }

    fn apply_background(&self, bg: tileide_config::model::Background, scope: themes_panel::Scope) {
        match scope {
            themes_panel::Scope::All => {
                if let Active::Number(n) = self.workspaces.borrow().active()
                    && let Some(pick) = self.state.borrow_mut().workspaces.get_mut(&n)
                {
                    pick.background = None;
                }
                // Saved, so it is there next session; also live, so it shows over `--bg`.
                *self.session_background.borrow_mut() = Some(bg.clone());
                let over = self.raw_config.borrow().background_set.then(|| self.raw_config.borrow().background.clone());
                let mut state = self.state.borrow_mut();
                state.background = Some(bg);
                state.background_over = over;
            }
            themes_panel::Scope::Workspace(n) => self.state.borrow_mut().workspaces.entry(n).or_default().background = Some(bg),
        }
        self.save_state();
        self.refresh();
        // Blur sliders follow whether TileIDE now draws the background.
        self.refresh_themes_panel_soon();
        self.reload_css();
        // The sliders show the new theme's look where the user chose nothing.
        self.refresh_themes_panel_soon();
    }

    /// "Import VS Code theme…": a theme `.json` or an extension `.vsix` becomes a theme
    /// file in the user's `themes/`, then shows in the list (hot reload).
    fn import_theme(&self) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("VS Code themes (.json, .vsix)"));
        for pattern in ["*.json", "*.vsix", "*.jsonc"] {
            filter.add_pattern(pattern);
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let me = self.me.clone();
        gtk::FileDialog::builder().title("Import a VS Code theme").filters(&filters).build().open(Some(&self.window), None::<&gio::Cancellable>, move |result| {
            let (Some(app), Some(path)) = (me.upgrade(), result.ok().and_then(|f| f.path())) else { return };
            let notes = match tileide_config::vscode::import(&path, &app.paths.dir.join("themes")) {
                Ok(written) => written
                    .into_iter()
                    .map(|(file, name, warnings)| {
                        let file = file.file_name().map_or_else(String::new, |f| f.to_string_lossy().into_owned());
                        match warnings.is_empty() {
                            true => format!("Imported \"{name}\" as themes/{file}; pick it in Themes."),
                            false => format!("Imported \"{name}\" as themes/{file}, but its {}.", warnings.join(" and ")),
                        }
                    })
                    .collect(),
                Err(e) => vec![format!("Cannot import {}: {e}", path.display())],
            };
            *app.problems.borrow_mut() = notes;
            app.show_problems();
            app.refresh_themes_panel();
        });
    }

    fn save_state(&self) {
        if let Err(e) = tileide_config::save_state(&self.paths.state, &self.state.borrow()) {
            self.problems.borrow_mut().push(format!("cannot save {}: {e}", self.paths.state.display()));
            self.show_problems();
        }
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

    /// Alt+D, and by itself on a new workspace without a folder.
    fn open_folder_search(&self) {
        if self.quick_open.is_open() || dialog::is_open() {
            return;
        }
        self.tray.close();
        self.overview.close();
        let recent = self.state.borrow().recent_folders.iter().map(PathBuf::from).filter(|p| p.is_dir()).collect();
        self.quick_open.open_folders(&workspaces::expand_home("~"), recent);
        self.area.queue_draw();
    }

    /// The folder search's pick: the workspace follows it, the file tree opens, and it
    /// goes to the top of the recent folders in `state.lua`.
    fn set_workspace_folder(&self, folder: PathBuf) {
        let mut state = self.state.borrow_mut();
        state.remember_folder(&folder.to_string_lossy());
        if let Err(e) = tileide_config::save_state(&self.paths.state, &state) {
            eprintln!("state: cannot save: {e}");
        }
        drop(state);
        self.workspaces.borrow_mut().set_folder(folder);
        // A folder was just opened: show its file tree.
        if !self.drawer.borrow().opens() {
            self.toggle_drawer();
        }
        self.workspaces.borrow().current().borrow().grab_focused();
    }

    /// Alt+Shift+W or an overview card's ×. Asks first if it would end unsaved edits or shells.
    fn close_workspace(&self, n: u8) {
        let (dirty, terminals) = self.workspaces.borrow().work(n);
        let me = self.me.clone();
        let remove = move || {
            let Some(app) = me.upgrade() else { return };
            app.workspaces.borrow_mut().remove(n);
            if app.overview.is_open() {
                app.show_overview();
            } else {
                app.workspaces.borrow().current().borrow().grab_focused();
            }
        };
        if dirty == 0 && terminals == 0 {
            remove();
            return;
        }
        let plural = |count: usize, one: &str| if count == 1 { format!("1 {one}") } else { format!("{count} {one}s") };
        let lost = match (dirty, terminals) {
            (0, t) => format!("Its {} will end.", plural(t, "terminal")),
            (d, 0) => format!("{} will be lost.", plural(d, "unsaved file")),
            (d, t) => format!("{} will be lost and {} will end.", plural(d, "unsaved file"), plural(t, "terminal")),
        };
        dialog::ask(&format!("Remove workspace {}?", workspaces::display(n)), &lost, &[("Cancel", dialog::Role::Cancel), ("Remove", dialog::Role::Destructive)], 0, move |choice| {
            if choice == 1 {
                remove();
            }
        });
    }

    fn open_picked(&self, target: quick_open::Target, path: Option<PathBuf>) {
        if target == quick_open::Target::Folder {
            if let Some(folder) = path {
                self.set_workspace_folder(folder);
            }
            return;
        }
        let view = self.workspaces.borrow().current();
        match (target, path) {
            (quick_open::Target::Focused, Some(path)) => view.borrow_mut().open_in_focused(&path, self.window.upcast_ref()),
            (_, path) => view.borrow_mut().add(Kind::Editor(path.as_deref())),
        }
        self.area.queue_draw();
    }

    fn open_overview(&self) {
        if self.overview.is_open() {
            self.overview.close();
            self.workspaces.borrow().current().borrow().grab_focused();
            return;
        }
        self.tray.close();
        self.show_overview();
    }

    /// Opens (or refreshes) the overview with the current workspaces.
    fn show_overview(&self) {
        let me = self.me.clone();
        let on_pick: Rc<dyn Fn(u8)> = Rc::new(move |n| {
            if let Some(app) = me.upgrade() {
                app.workspaces.borrow_mut().go(n);
            }
        });
        let me = self.me.clone();
        let on_remove: Rc<dyn Fn(u8)> = Rc::new(move |n| {
            if let Some(app) = me.upgrade() {
                app.close_workspace(n);
            }
        });
        let summaries = self.workspaces.borrow().summaries();
        self.overview.open(&summaries, on_pick, on_remove);
    }

    /// Places tiles and the Files panel for the current drawer extension and window size.
    fn sync(&self) {
        let numbers = self.rail_dots();
        let clock = self.clock.borrow();
        let mut scene = self.scene(&numbers, &clock);
        let area = (draw::tile_area(&scene), draw::stage_rect(&scene));
        // While the drawer slides, tile contents take the size they end with.
        let settle = {
            let d = self.drawer.borrow();
            d.animating().then(|| {
                scene.drawer = d.final_extension();
                draw::tile_area(&scene)
            })
        };
        self.workspaces.borrow_mut().set_area(area.0, area.1, settle);

        // The panel appears once the drawer is wide enough for its content (it fades
        // in during the last part of the expansion, design.md §8).
        let drawer = self.drawer.borrow();
        let extension = drawer.extension();
        let shown = extension >= token::DRAWER_MIN_WIDTH * 0.9;
        self.panel.set_visible(shown);
        if shown {
            // The overlay places the panel (`connect_get_child_position`).
            if let Some(overlay) = self.panel.parent() {
                overlay.queue_allocate();
            }
            self.panel.set_opacity(((extension / drawer.width - 0.6) / 0.4).clamp(0.0, 1.0));
        }
    }

    /// The Wikipedia rice's title, workspace cards and clock (cheap when nothing changed).
    fn refresh_bars(&self) {
        if token::rice() != token::Rice::Wikipedia {
            return;
        }
        let workspaces = self.workspaces.borrow();
        let summaries = workspaces.summaries();
        let title = match workspaces.active() {
            Active::Config => "Config".to_string(),
            Active::Number(_) => summaries.iter().find(|s| s.active).map_or_else(String::new, wiki::name),
        };
        self.bars.refresh(&summaries, &title);
        let clock = self.clock.borrow();
        self.bars.set_clock(&clock.0, &clock.1);
    }

    /// Wikipedia rice: shows or hides the workspace list on the right.
    fn toggle_workspace_list(&self) {
        let shown = !self.workspace_list.get();
        self.workspace_list.set(shown);
        token::set_rice(token::rice(), shown);
        self.bars.show(token::rice() == token::Rice::Wikipedia, shown);
        self.sync();
        self.area.queue_draw();
    }

    fn toggle_drawer(&self) {
        let config = self.config.borrow();
        if !config.sidebar.show_files && !self.drawer.borrow().is_open() {
            return;
        }
        drop(config);
        self.slide_drawer();
    }

    /// Opens or closes the drawer; the frame clock drives the slide only while it runs
    /// (an idle tick callback would keep the clock, and the compositor, busy every frame).
    fn slide_drawer(&self) {
        self.drawer.borrow_mut().toggle();
        self.area.queue_draw();
        if self.drawer_tick.get() {
            return;
        }
        self.drawer_tick.set(true);
        let me = self.me.clone();
        self.area.add_tick_callback(move |area, clock| {
            let Some(s) = me.upgrade() else { return glib::ControlFlow::Break };
            let now = clock.frame_time();
            {
                let mut d = s.drawer.borrow_mut();
                let mut stats = s.stats.borrow_mut();
                if !d.animating() {
                    stats.last_frame_us = None;
                    s.drawer_tick.set(false);
                    return glib::ControlFlow::Break;
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

    fn run(&self, action: &Action) {
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
            Action::Overview => self.open_overview(),
            Action::Themes => self.open_tray(),
            Action::Folder => self.open_folder_search(),
            Action::ThemesSection => self.toggle_themes_section(),
            Action::WorkspaceClose => {
                // Read first: the removal borrows the workspaces mutably (a borrow held
                // across this match panicked when nothing needed confirming).
                let active = self.workspaces.borrow().active();
                if let Active::Number(n) = active {
                    self.close_workspace(n);
                }
            }
            Action::Hover => {
                if let Some(editor) = view.borrow().focused_code() {
                    lsp::hover_at_cursor(&editor);
                }
            }
            Action::Definition => {
                if let Some(editor) = view.borrow().focused_code() {
                    lsp::definition_at_cursor(&editor);
                }
            }
            Action::Complete => {
                if let Some(editor) = view.borrow().focused_code() {
                    lsp::complete(&editor, lsp::Ask::Manual);
                }
            }
            Action::Format => {
                if let Some(editor) = view.borrow().focused_code() {
                    lsp::format(&editor);
                }
            }
            Action::Rename => {
                if let Some(editor) = view.borrow().focused_code() {
                    lsp::rename_at_cursor(&editor);
                }
            }
            Action::References => {
                if let Some(editor) = view.borrow().focused_code() {
                    lsp::references_at_cursor(&editor);
                }
            }
            Action::CodeAction => {
                if let Some(editor) = view.borrow().focused_code() {
                    lsp::code_actions_at_cursor(&editor);
                }
            }
            Action::Outline => {
                if let Some(editor) = view.borrow().focused_code() {
                    lsp::outline_at_cursor(&editor);
                }
            }
            Action::ExtensionsSection => self.toggle_extensions_section(),
            Action::Extension(command) => self.run_extension_command(command),
            Action::Settings => self.toggle_settings(settings_ui::Page::General),
            Action::Keybinds => {
                if self.keybinds_card.is_open() {
                    self.keybinds_card.close();
                } else {
                    self.open_keybinds();
                }
            }
            Action::FontSize(by) => self.step_font_size(*by),
            Action::Run => self.run_focused(),
            Action::Debug => {
                // In a notebook: debug the current cell (or continue a paused one).
                let notebook = view.borrow().focused_notebook();
                if let Some(notebook) = notebook.filter(|_| !debug::is_paused()) {
                    notebook.debug_cell();
                    return;
                }
                let file = view.borrow().focused_file();
                if !debug::is_paused() && file.is_some() && view.borrow().focused_dirty() {
                    view.borrow_mut().save_focused(self.window.upcast_ref());
                }
                let folder = {
                    let w = self.workspaces.borrow();
                    w.folder(w.active())
                };
                // While paused F5 continues, whatever is picked.
                let launch = if debug::is_paused() { None } else { self.picked_launch(file.as_deref(), folder.as_deref()) };
                debug::start_or_continue(file, folder, launch);
            }
            Action::DebugStop => debug::stop(),
            Action::Breakpoint => {
                let cell = view.borrow().focused_notebook().and_then(|n| n.breakpoint_target());
                if let Some((editor, key)) = cell {
                    debug::toggle_breakpoint(&editor, &key);
                    return;
                }
                let (editor, file) = (view.borrow().focused_editor(), view.borrow().focused_file());
                if let (Some(editor), Some(file)) = (editor, file) {
                    debug::toggle_breakpoint(&editor, &file);
                }
            }
            Action::StepOver => debug::step("next"),
            Action::StepInto => debug::step("stepIn"),
            Action::StepOut => debug::step("stepOut"),
            Action::Toolchains => self.toggle_settings(settings_ui::Page::Toolchains),
            Action::Test => self.run_tests(),
            Action::Launch => self.pick_launch(),
            Action::NextFailure => {
                let tests = self.tests.borrow().clone();
                if !tests.is_some_and(|t| t.next_failure()) {
                    self.notice("No failed tests (Alt+Shift+R runs them).".into());
                }
            }
            Action::Interpreter => {
                let folder = {
                    let w = self.workspaces.borrow();
                    w.folder(w.active())
                };
                let (notebook, editor, file) = (view.borrow().focused_notebook(), view.borrow().focused_code(), view.borrow().focused_file());
                match (notebook, editor, file) {
                    (Some(nb), ..) => nb.pick_kernel(),
                    (None, Some(editor), Some(file)) => interpreter::pick_for_file(&editor, &file, folder.as_deref()),
                    _ => self.notice("Open a Python file or a notebook to pick its Python.".into()),
                }
            }
            Action::Unavailable(_) => {}
        }
    }

    /// Shows workspace `n`'s files next to its dot (a click on the dot).
    fn show_peek(&self, n: u8, dot_y: f64) {
        let peek = &self.peek;
        peek.tree.borrow_mut().set_root(&self.workspaces.borrow().folder(Active::Number(n)).unwrap_or_default());
        peek.go.set_label(&format!("Workspace {}  →", workspaces::display(n)));
        let max_top = (self.area.height() - PEEK_HEIGHT - token::FRAME_BORDER as i32).max(0);
        peek.widget.set_margin_top(((dot_y - 20.0) as i32).clamp(0, max_top));
        peek.widget.set_visible(true);
        peek.target.set(Some((n, dot_y)));
    }

    /// Hovering a workspace dot (not the active one) opens its peek.
    fn hover_rail(&self, x: f64, y: f64) {
        if x > token::rail_width() || self.tray.is_open() || self.overview.is_open() || self.quick_open.is_open() || self.settings_card.is_open() || self.keybinds_card.is_open() || dialog::is_open() {
            return;
        }
        let numbers = self.rail_dots();
        let clock = self.clock.borrow().clone();
        let scene = self.scene(&numbers, &clock);
        let Some(draw::RailHit::Workspace(n)) = draw::rail_hit(&scene, x, y) else { return };
        let active = matches!(self.workspaces.borrow().active(), Active::Number(a) if a == n);
        if active || self.peek.target.get().is_some_and(|(t, _)| t == n) {
            return;
        }
        let dot_y = draw::rail_items(&scene).iter().find(|i| i.0 == draw::RailHit::Workspace(n)).map_or(y, |i| i.1);
        self.show_peek(n, dot_y);
    }

    /// Closes the peek once the pointer is neither on its dot nor over the panel.
    fn track_peek(&self, x: f64, y: f64) {
        let Some((_, dot_y)) = self.peek.target.get() else { return };
        let on_dot = x <= token::rail_width() + PEEK_GAP && (y - dot_y).abs() <= 14.0;
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
        drawer.is_open() && (x - (token::left_inset() + drawer.width)).abs() <= RESIZE_GRAB
    }
}

/// Hot reload: watches the config dir, `rices/` and `themes/`. `state.lua` is ours.
/// The generated CSS with Settings' interface font and text scale: every
/// `font-size: Npx` is scaled (code sizes are in pt and stay), and the design font is
/// swapped for the chosen one.
fn interface_css(css: &str, font: &str, scale: f64) -> String {
    let css = css.replace(&format!("\"{}\"", token::UI_FONT), &format!("\"{}\"", font.replace('"', "")));
    if (scale - 1.0).abs() < 1e-3 {
        return css;
    }
    let mut out = String::with_capacity(css.len());
    let mut rest = css.as_str();
    while let Some(i) = rest.find("font-size:") {
        let (head, tail) = rest.split_at(i + "font-size:".len());
        out.push_str(head);
        let value = tail.trim_start();
        let digits = value.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(value.len());
        match (value[..digits].parse::<f64>(), value[digits..].starts_with("px")) {
            (Ok(px), true) => {
                out.push_str(&format!(" {:.1}", px * scale));
                rest = &value[digits..];
            }
            _ => rest = tail,
        }
    }
    out.push_str(rest);
    out
}

/// Development: renders `window` to a PNG (`--test-actions screenshot:<file>@<s>`).
/// Also saves each open popover (they are surfaces of their own) as `<path>-popoverN.png`
/// and prints its text, so headless checks can read completion, hover and signature cards.
fn screenshot(window: &gtk::Window, path: &str) -> Result<(), String> {
    let render = |widget: &gtk::Widget, path: &str| -> Result<(), String> {
        let paintable = gtk::WidgetPaintable::new(Some(widget));
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(&snapshot, widget.width() as f64, widget.height() as f64);
        let node = snapshot.to_node().ok_or("nothing drawn")?;
        let renderer = window.renderer().ok_or("no renderer")?;
        renderer.render_texture(&node, None).save_to_png(path).map_err(|e| e.to_string())
    };
    // The window can come out empty for a frame (seen right after a popover opens);
    // the popovers are still worth saving.
    // Headless (Broadway with no viewer) the frame clock stalls: no layout, no paint, and
    // a widget paintable only copies what was painted. Lay out and snapshot directly.
    let direct = |path: &str| -> Result<(), String> {
        let child = window.child().ok_or("no content")?;
        // Layout is stalled too: lay the content out at the window's size first.
        let _ = child.measure(gtk::Orientation::Horizontal, -1);
        child.allocate(window.width(), window.height(), -1, None);
        let snapshot = gtk::Snapshot::new();
        window.snapshot_child(&child, &snapshot);
        let node = snapshot.to_node().ok_or("nothing drawn")?;
        let renderer = window.renderer().ok_or("no renderer")?;
        renderer.render_texture(&node, Some(&gtk::graphene::Rect::new(0.0, 0.0, window.width() as f32, window.height() as f32))).save_to_png(path).map_err(|e| e.to_string())
    };
    let window_result = render(window.upcast_ref(), path).or_else(|_| direct(path));
    fn popovers(widget: &gtk::Widget, out: &mut Vec<gtk::Popover>) {
        let mut child = widget.first_child();
        while let Some(c) = child {
            match c.downcast_ref::<gtk::Popover>() {
                Some(p) if p.is_visible() => out.push(p.clone()),
                _ => popovers(&c, out),
            }
            child = c.next_sibling();
        }
    }
    let mut open = Vec::new();
    popovers(window.upcast_ref(), &mut open);
    for (i, p) in open.iter().enumerate() {
        fn texts(widget: &gtk::Widget, out: &mut Vec<String>) {
            if let Some(l) = widget.downcast_ref::<gtk::Label>() {
                out.push(l.text().to_string());
            }
            let mut child = widget.first_child();
            while let Some(c) = child {
                texts(&c, out);
                child = c.next_sibling();
            }
        }
        let mut words = Vec::new();
        texts(p.upcast_ref(), &mut words);
        println!("popover {i} ({}): {}", p.css_classes().join(" "), words.join(" | ").replace('\n', "⏎"));
        if let Err(e) = render(p.upcast_ref(), &format!("{}-popover{i}.png", path.trim_end_matches(".png"))) {
            eprintln!("test action: popover {i}: {e}");
        }
    }
    window_result
}

/// The visualizer strip under the file tree (drawer open).
fn connect_wave(state: &Rc<App>) {
    let me = Rc::downgrade(state);
    state.rail_wave.set_draw_func(move |_, cr, w, h| {
        let Some(app) = me.upgrade() else { return };
        if let Some((bars, k)) = app.visualizer.shown() {
            let accent = app.theme.borrow().accent;
            let _ = draw::waveform(cr, tileide_core::tiles::Rect { x: 0.0, y: 0.0, w: w as f64, h: h as f64 }, &bars, accent, k, true);
        }
    });
    let me = Rc::downgrade(state);
    state.wave.set_draw_func(move |_, cr, w, h| {
        let Some(app) = me.upgrade() else { return };
        if let Some((bars, k)) = app.visualizer.shown() {
            let accent = app.theme.borrow().accent;
            let _ = draw::waveform(cr, tileide_core::tiles::Rect { x: 0.0, y: 0.0, w: w as f64, h: h as f64 }, &bars, accent, k, false);
        }
    });
}

/// The debugger opens files and its tile in the active workspace.
fn init_debug(state: &Rc<App>) {
    let (m1, m2) = (Rc::downgrade(state), Rc::downgrade(state));
    debug::init(debug::Hooks {
        show_line: Rc::new(move |path, line| {
            let app = m1.upgrade()?;
            let view = app.workspaces.borrow().current();
            view.borrow_mut().show_line(path, line)
        }),
        show_panel: Rc::new(move |widget| {
            let Some(app) = m2.upgrade() else { return };
            let view = app.workspaces.borrow().current();
            if !view.borrow().shows(widget) {
                view.borrow_mut().add(Kind::Panel(widget.clone()));
            }
        }),
    });
}

/// The Tests tile opens failures in the active workspace.
fn init_tests(state: &Rc<App>) {
    let me = Rc::downgrade(state);
    let view = tests_view::TestsView::new(Rc::new(move |path, line| {
        if let Some(app) = me.upgrade() {
            let view = app.workspaces.borrow().current();
            // Deferred: a click in the Tests tile may come while a key handler holds it.
            glib::idle_add_local_once(move || {
                view.borrow_mut().show_line(&path, line);
            });
        }
    }));
    *state.tests.borrow_mut() = Some(view);
}

/// Extensions: found now, started later by their activation events.
fn init_extensions(state: &Rc<App>) -> Vec<String> {
    let root = std::env::var_os("TILEIDE_EXTENSIONS_DIR").map_or_else(extensions::default_root, PathBuf::from);
    let (m1, m2, m3) = (Rc::downgrade(state), Rc::downgrade(state), Rc::downgrade(state));
    let hooks = extensions::Hooks {
        notice: Rc::new(move |message| {
            if let Some(app) = m1.upgrade() {
                app.notice(message);
            }
        }),
        insert: Rc::new(move |text| {
            let Some(app) = m2.upgrade() else { return };
            let editor = app.workspaces.borrow().current().borrow().focused_editor();
            match editor {
                Some(view) => view.buffer().insert_at_cursor(&text),
                None => app.notice("an extension tried to insert text, but no editor is focused".into()),
            }
        }),
        changed: Rc::new(move || {
            if let Some(app) = m3.upgrade()
                && app.settings_card.page() == Some(settings_ui::Page::Extensions)
            {
                app.extensions_panel.refresh();
            }
        }),
    };
    let picks = state.state.borrow().extensions.clone();
    extensions::init(root, picks, hooks)
}

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
    let Some(path) = state.background.borrow().as_ref().filter(|b| b.needs_image()).and_then(background::Background::image_path) else {
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
        if token::rice() == token::Rice::Wikipedia {
            // Not while painting: changing widgets there queues a relayout mid-frame.
            let me = s.me.clone();
            glib::idle_add_local_once(move || {
                if let Some(app) = me.upgrade() {
                    app.refresh_bars();
                }
            });
        }
        // Cheap clone (refcounted); baked only when the size changes.
        // The fractional scale (e.g. 1.5), so the paint is a 1:1 copy, not a resample.
        let scale = area.native().and_then(|n| n.surface()).map_or(area.scale_factor() as f64, |s| s.scale());
        // Baked either way (it makes the backdrops); painted only when it isn't the wallpaper.
        let background = s.background.borrow_mut().as_mut().and_then(|b| b.surface(width, height, scale).cloned().filter(|_| b.paints()));
        let frosted = s.background.borrow().as_ref().and_then(|b| b.frosted().cloned());
        let frosted_frame = s.background.borrow().as_ref().and_then(|b| b.frosted_frame().cloned());
        let tile_rects = if frosted.is_some() { s.workspaces.borrow().current().borrow().tile_rects() } else { Vec::new() };
        let numbers = s.rail_dots();
        let now = area.frame_clock().map_or(0, |c| c.frame_time());
        if s.dots.borrow().animating(now) {
            let area = area.downgrade();
            glib::idle_add_local_once(move || {
                if let Some(area) = area.upgrade() {
                    area.queue_draw();
                }
            });
        }
        let clock = s.clock.borrow();
        let mut scene = s.scene(&numbers, &clock);
        scene.background = background.as_ref();
        scene.frosted = frosted.as_ref();
        scene.frosted_frame = frosted_frame.as_ref();
        scene.tile_rects = &tile_rects;
        let wave = s.visualizer.shown().filter(|_| s.config.borrow().sidebar.show_visualizer);
        scene.visualizer = wave.as_ref().map(|(bars, k)| (bars.as_slice(), *k));
        if let Err(e) = draw::draw(cr, &scene) {
            eprintln!("draw failed: {e}");
        }
        if s.drawer.borrow().animating() {
            s.stats.borrow_mut().draw_times.push(begin.elapsed());
        }
        if s.themes_shown() {
            let report = s.background.borrow().as_ref().and_then(|b| b.report().map(|r| r.describe()));
            s.themes_panel.set_legibility(report.as_ref().map(|(text, _)| text.as_str()), report.as_ref().is_some_and(|(_, warn)| *warn));
        }
        if first_frame.replace(false) {
            println!(
                "first frame: {:.1} ms after start, surface {width}x{height} logical px, RSS {}",
                started.elapsed().as_secs_f64() * 1000.0,
                bench::rss().unwrap_or_else(|| "?".into())
            );
            // The first file opened used to stall 20–30 ms loading the editor's languages
            // and colour scheme; load them now, while nothing animates.
            let theme = s.theme.borrow().clone();
            glib::timeout_add_local_once(Duration::from_millis(300), move || tile_view::prewarm(&theme));
        }
    });

    let s = state.clone();
    state.area.connect_resize(move |_, _, _| s.sync());

}

fn connect_mouse(state: &Rc<App>) {
    // On the window, capture phase: tile and panel widgets sit above the drawing area.
    let click = gtk::GestureClick::new();
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let s = state.clone();
    click.connect_pressed(move |_, _, x, y| {
        // The tray and the overview handle their own clicks.
        if s.tray.is_open() || s.overview.is_open() || s.quick_open.is_open() || s.settings_card.is_open() || s.keybinds_card.is_open() || s.run_card.is_open() || dialog::is_open() {
            return;
        }
        let numbers = s.rail_dots();
        let clock = s.clock.borrow().clone();
        let scene = s.scene(&numbers, &clock);
        let (hit, welcome) = (draw::rail_hit(&scene, x, y), draw::welcome_hit(&scene, x, y));
        drop(scene);
        match welcome {
            Some(draw::Welcome::OpenFile) => s.open_quick(quick_open::Target::NewTile),
            Some(draw::Welcome::Terminal) => s.run(&Action::Terminal),
            Some(draw::Welcome::Folder) => s.open_folder_search(),
            None => {}
        }
        match hit {
            Some(draw::RailHit::Launcher) => s.toggle_drawer(),
            // Hovering a dot previews it; a click goes there.
            Some(draw::RailHit::Workspace(n)) => {
                hide_peek(&s.peek.widget, &s.peek.target);
                s.workspaces.borrow_mut().go(n);
            }
            Some(draw::RailHit::NewWorkspace) => s.workspaces.borrow_mut().create_next(),
            Some(draw::RailHit::Themes) => s.open_tray(),
            Some(draw::RailHit::Config) => s.workspaces.borrow_mut().toggle_config(),
            Some(draw::RailHit::Settings) => s.toggle_settings(settings_ui::Page::General),
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
        let numbers = s.rail_dots();
        let clock = s.clock.borrow().clone();
        let over_card = draw::welcome_hit(&s.scene(&numbers, &clock), x, y).is_some();
        let cursor = if s.near_resize_edge(x) { Some("col-resize") } else { over_card.then_some("pointer") };
        s.window.set_cursor_from_name(cursor);
        s.hover_rail(x, y);
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
    widget.set_margin_start((token::rail_width() + PEEK_GAP) as i32);
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
    go.set_hexpand(true);
    let close = gtk::Button::from_icon_name("window-close-symbolic");
    close.add_css_class("close");
    close.set_tooltip_text(Some("Remove this workspace"));
    close.set_valign(gtk::Align::Center);
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    header.append(&go);
    header.append(&close);
    widget.append(&header);

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
    Peek { widget, go, close, tree, target }
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
        if (s.tray.is_open() || s.overview.is_open() || s.quick_open.is_open() || s.keybinds_card.is_open()) && !matches!(action, Action::Themes | Action::Overview | Action::Editor | Action::Search | Action::Quit | Action::Keybinds) {
            return glib::Propagation::Proceed;
        }
        // The run card: only Alt+R (run again) and quitting act.
        if s.run_card.is_open() && !matches!(action, Action::Run | Action::Quit) {
            return glib::Propagation::Proceed;
        }
        // Settings takes typing; only its own keys act.
        if s.settings_card.is_open() && !matches!(action, Action::Settings | Action::ThemesSection | Action::ExtensionsSection | Action::Toolchains | Action::Quit) {
            return glib::Propagation::Proceed;
        }
        // Editor actions leave their keys to a focused terminal (Ctrl+Space, F12).
        if matches!(action, Action::Hover | Action::Definition | Action::Complete | Action::Format | Action::Rename | Action::References | Action::CodeAction | Action::Outline) && s.workspaces.borrow().current().borrow().focused_code().is_none() {
            return glib::Propagation::Proceed;
        }
        s.run(&action);
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
            lsp::shutdown_all();
            extensions::stop_all();
            s.visualizer.shutdown();
            debug::shutdown();
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

/// One second of `trace_frames`, in µs.
#[derive(Clone, Copy, Default)]
struct FrameTrace {
    second: i64,
    last: i64,
    frames: u32,
    longest_gap: i64,
    frame_began: i64,
    longest_frame: i64,
}

/// `TILEIDE_TRACE_FRAMES`: frames per second, the longest gap between two, and the
/// longest time the main thread spent inside one (layout + paint), printed each second.
fn trace_frames(window: &gtk::Window) {
    if std::env::var_os("TILEIDE_TRACE_FRAMES").is_none() {
        return;
    }
    let stats: Rc<Cell<FrameTrace>> = Rc::default();
    window.connect_realize(move |w| {
        let Some(clock) = w.frame_clock() else { return };
        let s = stats.clone();
        clock.connect_before_paint(move |_| s.set(FrameTrace { frame_began: glib::monotonic_time(), ..s.get() }));
        let s = stats.clone();
        clock.connect_after_paint(move |clock| {
            let (now, v) = (clock.frame_time(), s.get());
            let took = glib::monotonic_time() - v.frame_began;
            if v.second == 0 || now - v.second >= 1_000_000 {
                if v.second != 0 {
                    println!("trace: {} frames/s, longest gap {:.1} ms, longest frame {:.1} ms", v.frames, v.longest_gap as f64 / 1000.0, v.longest_frame as f64 / 1000.0);
                }
                s.set(FrameTrace { second: now, last: now, frames: 1, longest_gap: 0, frame_began: v.frame_began, longest_frame: took });
            } else {
                s.set(FrameTrace { last: now, frames: v.frames + 1, longest_gap: v.longest_gap.max(now - v.last), longest_frame: v.longest_frame.max(took), ..v });
            }
        });
    });
}
