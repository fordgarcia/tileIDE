// SPDX-License-Identifier: GPL-3.0-or-later
//! Settings (v0.4 phase 1): a large rounded card over a lightly dimmed window, in the
//! quick-open style. A search field on top filters every setting; the sections sit on
//! the left, each showing only its own settings (search looks through all of them);
//! changes apply live and are saved in `state.lua`. A key that `tileide.lua`
//! sets is shown locked (the config wins, Q15). Themes + background and Extensions are
//! pages of their own (the drawer's former sections).

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib, pango};
use gtk4 as gtk;
use tileide_config::Config;
use tileide_config::settings::{self, Kind, Value};
use tileide_core::theme::{self, Theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    General,
    Themes,
    ThemeEditor,
    Extensions,
    Toolchains,
}

pub fn css(t: &Theme) -> String {
    let c = theme::css;
    format!(
        ".settings {{ background-color: rgba(0, 0, 0, 0.35); }}
.settings .card {{ background-color: {surface}; color: {text}; border-radius: 22px; border: 1px solid {edge}; padding: 10px; box-shadow: 0 18px 48px rgba(0, 0, 0, 0.35); }}
.settings entry.search {{ background: transparent; border: none; box-shadow: none; outline: none; color: {text}; font-size: 17px; min-height: 40px; padding: 0 12px; caret-color: {accent}; }}
.settings entry.search image {{ color: {muted}; }}
.settings .divider {{ background-color: {edge}; min-height: 1px; margin: 6px 4px; }}
.settings .vdivider {{ background-color: {edge}; min-width: 1px; margin: 4px 6px; }}
.settings list, .settings scrolledwindow, .settings viewport, .settings stack {{ background: transparent; }}
.settings .nav row {{ border-radius: 14px; padding: 8px 14px; color: {muted}; background: transparent; outline: none; }}
.settings .nav row:selected {{ background-color: {select}; color: {text}; }}
.settings .nav row:hover:not(:selected) {{ background-color: {hover}; }}
.settings .page {{ padding: 2px 14px 14px 10px; }}
.settings .heading {{ font-weight: 600; color: {muted}; font-size: 11px; letter-spacing: 1px; margin: 16px 4px 6px 4px; }}
.settings .item {{ border-radius: 14px; padding: 9px 12px; background-color: {row}; margin-bottom: 4px; }}
.settings .item .title {{ font-size: 14px; }}
.settings .item .subtitle {{ font-size: 12px; color: {muted}; }}
.settings .item .lock {{ font-size: 11px; color: {accent}; }}
.settings button.unlock {{ min-height: 0; padding: 0 8px; border-radius: 9px; border: 1px solid {edge}; background: transparent; box-shadow: none; color: {accent}; font-size: 11px; }}
.settings button.unlock:hover {{ background-color: {hover}; }}
.settings .value {{ color: {muted}; font-size: 12px; font-feature-settings: \"tnum\"; }}
.settings .empty {{ color: {muted}; padding: 24px 6px; }}
.settings .footer {{ color: {muted}; font-size: 12px; padding: 6px 8px 0 8px; }}
.settings .version {{ color: {muted}; font-size: 11px; padding: 6px 8px 0 8px; opacity: 0.8; }}
.settings switch:checked {{ background-color: {accent}; }}
.settings button.pill {{ min-height: 26px; padding: 0 12px; border-radius: 13px; border: 1px solid {edge}; background: transparent; box-shadow: none; color: {text}; font-size: 12px; }}
.settings button.pill:hover {{ background-color: {hover}; }}
.settings scale trough {{ background-color: {edge}; min-height: 4px; border-radius: 2px; }}
.settings scale highlight {{ background-color: {accent}; border-radius: 2px; }}
.settings scale slider {{ background-color: {accent}; min-width: 14px; min-height: 14px; margin: -5px; border-radius: 7px; border: none; box-shadow: none; }}
.settings dropdown button, .settings fontbutton button, .settings button.font {{ min-height: 26px; padding: 0 10px; border-radius: 13px; }}",
        surface = c(t.surface, 0.97),
        text = c(t.text, 1.0),
        muted = c(t.muted, 1.0),
        edge = c(t.text, 0.10),
        accent = c(t.accent, 1.0),
        select = c(t.accent, 0.16),
        hover = c(t.text, 0.05),
        row = c(t.text, 0.035),
    )
}

pub struct Callbacks {
    /// A setting changed (already checked against its kind).
    pub change: Rc<dyn Fn(&'static str, Value)>,
    pub open_config: Rc<dyn Fn()>,
    /// "Unlock" on a row `tileide.lua` sets: comment that line out there.
    pub unlock: Rc<dyn Fn(&'static str)>,
    pub show_keybinds: Rc<dyn Fn()>,
    pub export_setup: Rc<dyn Fn()>,
    /// Where the project's own tool settings are looked for: the focused file's
    /// folder (or the workspace folder), and the workspace folder.
    pub project: Rc<dyn Fn() -> Option<(PathBuf, Option<PathBuf>)>>,
    /// A page became visible (the Themes page refreshes itself from the app).
    pub page_shown: Rc<dyn Fn(Page)>,
    pub on_close: Rc<dyn Fn()>,
}

enum Control {
    Switch(gtk::Switch),
    Scale(gtk::Scale),
    Font(gtk::FontDialogButton),
    Choice(gtk::DropDown, &'static [&'static str]),
}

struct Item {
    key: Option<&'static str>,
    section: usize,
    widget: gtk::Box,
    /// Lowercase title, subtitle, section and keywords.
    text: String,
    control: Option<Control>,
    /// "Set in tileide.lua" and its Unlock button.
    lock: gtk::Box,
}

/// One setting row: (key, title, subtitle, extra search words).
type Spec = (&'static str, &'static str, &'static str, &'static str);

/// The General page, in order.
const SECTIONS: &[(&str, &[Spec])] = &[
    (
        "Appearance",
        &[
            ("appearance.ui_font", "Interface font", "The drawer, cards and rail", "typeface family ui"),
            ("appearance.ui_scale", "Interface size", "Scales TileIDE's own text", "scale zoom gui size bigger smaller"),
            ("appearance.code_font", "Code font", "Editors and terminals", "monospace typeface family editor terminal"),
            ("appearance.code_size", "Code size", "Alt+= and Alt+- change it too", "font size zoom scale points"),
        ],
    ),
    (
        "Editor",
        &[
            ("editor.auto_close", "Close brackets and quotes", "Typing ( [ { \" ' adds the closing one; typing it again steps over", "auto pair autoclose brackets parentheses quotes complete"),
            ("editor.match_brackets", "Highlight matching brackets", "The bracket that pairs with the one at the cursor", "bracket match highlight parentheses pair"),
            ("editor.format_on_save", "Format on save", "Saving sorts the imports and formats the file first (Python: Ruff)", "format save ruff black organize sort imports isort tidy"),
            ("editor.theme_syntax", "Code colours from the theme", "Keywords, types, strings … in the theme's accent and its neighbouring hues (themes from the theme editor too); off: the terminal's colours", "syntax highlighting code colours theme accent palette match"),
            ("editor.inlay_hints", "Inlay hints", "Inferred types and parameter names at the end of the line (basedpyright, clangd, rust-analyzer; pyright has none)", "inlay hints types inferred parameter names annotations"),
        ],
    ),
    (
        "Layout",
        &[
            ("appearance.rice", "Rice", "Default: the rail, Files as a drawer · Wikipedia: a top bar, Files on the left, workspaces on the right, tiles stacked top to bottom", "layout style arrangement wikipedia rail top bar sidebar vertical stack"),
            ("appearance.window_radius", "Rounded corners", "Tile corner radius", "radius round corners window"),
            ("appearance.tile_border", "Tile border", "0 hides it; the focused tile's is always the accent", "outline edge stroke line width tile"),
            ("appearance.tile_border_color", "Tile border colour", "From the theme", "outline edge colour color tile"),
            ("appearance.frame_border", "Outer border", "A line around the area the tiles sit in; 0 hides it", "outline edge stroke line frame window outer"),
            ("appearance.frame_border_color", "Outer border colour", "From the theme", "outline edge colour color frame outer"),
            ("tiling.gaps_in", "Gaps between tiles", "", "gap spacing inner"),
            ("tiling.gaps_out", "Gaps at the edge", "Between the frame and the tiles", "gap spacing outer margin"),
            ("tiling.max_tiles", "Most tiles per workspace", "New tiles beyond this are refused", "tile limit maximum count many crash"),
            ("tiling.peek", "Next-tile peek", "Columns layout (Alt+V): how much of the tiles beside the focused one stays in sight; 0 hides it", "peek preview neighbour next previous column edge scroll niri hint"),
            ("link:themes", "Transparency, blur and wallpaper", "In Themes & background", "opacity transparent blur background wallpaper image gradient"),
        ],
    ),
    (
        "Motion",
        &[
            ("appearance.animations", "Animations", "Tiles glide; cards and workspaces fade", "animation motion smooth off reduce"),
            ("appearance.motion", "Style", "Fluid: soft springs and fades, like iOS · Snappy: quick and direct", "apple ios spring curve easing"),
            ("appearance.motion_speed", "Speed", "Faster or slower than designed", "duration animation speed"),
        ],
    ),
    (
        "Sidebar",
        &[
            ("sidebar.scale", "Sidebar size", "The rail's width and everything on it", "width scale rail bigger smaller wide"),
            ("sidebar.icon_scale", "Icon size", "Icons and workspace dots on the rail", "icons scale bigger smaller dots"),
            ("sidebar.icon_backgrounds", "Icon backgrounds", "The circles and squares behind the rail's icons", "icon background circle square button fill plain"),
            ("sidebar.pills", "Pills", "The two rounded shapes the rail's buttons sit on", "pill capsule background group plain minimal"),
            ("sidebar.show_visualizer", "Audio visualizer", "A waveform while something plays (in the rail, or under the files)", "music sound audio waveform spectrum"),
            ("sidebar.start_expanded", "Start with the files open", "", "drawer expanded startup"),
            ("sidebar.show_launcher", "File-tree button", "At the top of the rail", "launcher opener tree files show hide"),
            ("sidebar.show_workspaces", "Workspace dots", "", "workspaces show hide"),
            ("sidebar.show_label", "Logo", "In the middle of the rail", "label logo name show hide"),
            ("sidebar.show_themes", "Theme tray button", "The palette", "palette tray show hide"),
            ("sidebar.show_config", "Config folder", "Under the workspace dots", "config gear folder show hide"),
            ("sidebar.show_clock", "Clock", "", "time show hide"),
        ],
    ),
    (
        "Alerts",
        &[(
            "alerts.missing_tools",
            "Missing tools",
            "A popup when a language server, debugger, Jupyter or a Python module is missing. Off, the message still shows in the run card, debug tile or notebook cell",
            "alert notification popup banner missing dependency dependencies install module library package warning",
        ), (
            "alerts.tile_limit",
            "Tile limit reached",
            "A popup when a new tile is refused because the workspace holds Most tiles per workspace already",
            "alert notification popup banner tile limit maximum max count full",
        )],
    ),
    (
        "Keyboard",
        &[
            ("link:keybinds", "Keybinds", "Every key, searchable · Alt+K", "keys shortcuts bindings hotkeys"),
            ("link:config", "Change keys in tileide.lua", "tileide.bind(mods, key, action, arg)", "rebind config lua"),
        ],
    ),
    (
        "Share",
        &[
            ("link:theme_editor", "Theme editor", "Make a theme from the one in use: change its colours live, save or export it", "create new theme colours colors custom editor make export"),
            ("link:export_setup", "Export setup", "tileide.lua, your Settings choices, themes, rices and background images in one .tar.gz", "export backup share rice bundle archive dotfiles setup copy machine"),
        ],
    ),
    (
        "More",
        &[
            ("link:themes", "Themes & background", "Colours, wallpaper, VS Code themes", "theme colours colors background wallpaper import vscode"),
            ("link:extensions", "Extensions", "Installed extensions and what they cost", "plugins addons weight meter"),
            ("link:toolchains", "Toolchains", "Compilers, interpreters, debuggers and Jupyter found here", "python gcc clang rust node java go gdb debugpy jupyter compiler install"),
            ("link:config", "Config file", "Open the Config workspace · Alt+C", "lua tileide.lua config edit"),
        ],
    ),
];

/// Left column: the General sections, then the two pages.
fn nav_entries() -> Vec<(&'static str, Option<usize>, Page)> {
    let mut entries: Vec<(&str, Option<usize>, Page)> = SECTIONS.iter().enumerate().filter(|(_, s)| s.0 != "More").map(|(i, s)| (s.0, Some(i), Page::General)).collect();
    entries.push(("Themes & background", None, Page::Themes));
    entries.push(("Theme editor", None, Page::ThemeEditor));
    entries.push(("Extensions", None, Page::Extensions));
    entries.push(("Toolchains", None, Page::Toolchains));
    entries
}

pub struct SettingsCard {
    root: gtk::Box,
    card: gtk::Box,
    search: gtk::SearchEntry,
    nav: gtk::ListBox,
    stack: gtk::Stack,
    scroll: gtk::ScrolledWindow,
    toolchains: gtk::Box,
    headings: Vec<gtk::Label>,
    empty: gtk::Label,
    items: Vec<Item>,
    callbacks: Callbacks,
    updating: Cell<bool>,
    /// Set while the app selects a nav row itself.
    navigating: Cell<bool>,
    page: Cell<Page>,
    /// The General section shown when nothing is searched.
    section: Cell<usize>,
    me: RefCell<std::rc::Weak<SettingsCard>>,
}

fn label(text: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class(class);
    l.set_xalign(0.0);
    l
}

fn format_value(key: &str, v: f64) -> String {
    match key {
        "appearance.ui_scale" | "sidebar.scale" | "sidebar.icon_scale" => format!("{:.0}%", v * 100.0),
        "appearance.motion_speed" => format!("{v:.2}×"),
        "appearance.code_size" => format!("{v:.0} pt"),
        "tiling.max_tiles" => format!("{v:.0}"),
        _ => format!("{v:.0} px"),
    }
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

fn step(key: &str) -> f64 {
    match key {
        "appearance.ui_scale" | "sidebar.scale" | "sidebar.icon_scale" => 0.05,
        "appearance.motion_speed" => 0.25,
        _ => 1.0,
    }
}

impl SettingsCard {
    pub fn new(overlay: &gtk::Overlay, themes: &impl IsA<gtk::Widget>, editor: &impl IsA<gtk::Widget>, extensions: &impl IsA<gtk::Widget>, callbacks: Callbacks) -> Rc<SettingsCard> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("settings");
        root.set_visible(false);

        let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        card.add_css_class("card");
        card.set_halign(gtk::Align::Center);
        card.set_valign(gtk::Align::Center);
        card.set_vexpand(true);
        card.set_overflow(gtk::Overflow::Hidden);
        let search = gtk::SearchEntry::new();
        search.add_css_class("search");
        search.set_placeholder_text(Some("Search settings"));
        card.append(&search);
        let divider = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        divider.add_css_class("divider");
        card.append(&divider);

        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        body.set_vexpand(true);
        let nav = gtk::ListBox::new();
        nav.add_css_class("nav");
        nav.set_selection_mode(gtk::SelectionMode::Single);
        nav.set_size_request(190, -1);
        for (title, _, _) in nav_entries() {
            let l = label(title, "nav-title");
            nav.append(&l);
        }
        body.append(&nav);
        let vdivider = gtk::Box::new(gtk::Orientation::Vertical, 0);
        vdivider.add_css_class("vdivider");
        body.append(&vdivider);

        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        page.add_css_class("page");
        let empty = label("Nothing matches.", "empty");
        empty.set_visible(false);
        page.append(&empty);
        let mut headings = Vec::new();
        let mut items = Vec::new();
        for (i, (name, specs)) in SECTIONS.iter().enumerate() {
            let heading = label(&name.to_uppercase(), "heading");
            page.append(&heading);
            headings.push(heading);
            for &(key, title, subtitle, words) in *specs {
                let item = build_item(key, title, subtitle, i);
                let mut text = format!("{title} {subtitle} {name} {words}").to_lowercase();
                text.push(' ');
                page.append(&item.widget);
                items.push(Item { text, ..item });
            }
        }
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroll.set_hexpand(true);
        scroll.set_child(Some(&page));

        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.set_hexpand(true);
        stack.add_named(&scroll, Some("general"));
        stack.add_named(themes, Some("themes"));
        stack.add_named(editor, Some("editor"));
        stack.add_named(extensions, Some("extensions"));
        let toolchains = gtk::Box::new(gtk::Orientation::Vertical, 0);
        toolchains.add_css_class("page");
        let toolchains_scroll = gtk::ScrolledWindow::new();
        toolchains_scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        toolchains_scroll.set_child(Some(&toolchains));
        stack.add_named(&toolchains_scroll, Some("toolchains"));
        body.append(&stack);
        card.append(&body);

        let footer = label("Changes apply as you make them and are kept in state.lua; anything tileide.lua sets wins and is locked here. Esc closes.", "footer");
        footer.set_wrap(true);
        footer.set_hexpand(true);
        // The running version, bottom right.
        let version = label(concat!("TileIDE ", env!("CARGO_PKG_VERSION")), "version");
        version.set_valign(gtk::Align::End);
        let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        bottom.append(&footer);
        bottom.append(&version);
        card.append(&bottom);
        root.append(&card);
        overlay.add_overlay(&root);

        let view = Rc::new(SettingsCard {
            root,
            card,
            search,
            nav,
            stack,
            scroll,
            toolchains,
            headings,
            empty,
            items,
            callbacks,
            updating: Cell::new(false),
            navigating: Cell::new(false),
            page: Cell::new(Page::General),
            section: Cell::new(0),
            me: RefCell::new(std::rc::Weak::new()),
        });
        *view.me.borrow_mut() = Rc::downgrade(&view);
        view.connect();
        view
    }

    fn connect(self: &Rc<Self>) {
        for item in &self.items {
            let Some(key) = item.key else {
                // Link rows: their button.
                continue;
            };
            let weak = Rc::downgrade(self);
            let send = move |value: Value| {
                if let Some(v) = weak.upgrade()
                    && !v.updating.get()
                    && let Some(value) = settings::checked(key, value)
                {
                    (v.callbacks.change)(key, value);
                }
            };
            match &item.control {
                Some(Control::Switch(s)) => {
                    s.connect_state_set(move |_, on| {
                        send(Value::Bool(on));
                        glib::Propagation::Proceed
                    });
                }
                Some(Control::Scale(s)) => {
                    s.connect_value_changed(move |s| send(Value::Number(s.value())));
                }
                Some(Control::Font(f)) => {
                    f.connect_font_desc_notify(move |f| {
                        if let Some(family) = f.font_desc().and_then(|d| d.family()) {
                            send(Value::Text(family.to_string()));
                        }
                    });
                }
                Some(Control::Choice(d, keys)) => {
                    let keys = *keys;
                    d.connect_selected_notify(move |d| {
                        if let Some(k) = keys.get(d.selected() as usize) {
                            send(Value::Text((*k).to_string()));
                        }
                    });
                }
                None => {}
            }
            if let Some(button) = item.lock.last_child().and_downcast::<gtk::Button>() {
                let weak = Rc::downgrade(self);
                button.connect_clicked(move |_| {
                    if let Some(v) = weak.upgrade() {
                        (v.callbacks.unlock)(key);
                    }
                });
            }
        }
        for item in self.items.iter().filter(|i| i.key.is_none()) {
            let Some(button) = item.widget.last_child().and_downcast::<gtk::Button>() else { continue };
            let weak = Rc::downgrade(self);
            let link = button.widget_name().to_string();
            button.connect_clicked(move |_| {
                let Some(v) = weak.upgrade() else { return };
                match link.as_str() {
                    "link:themes" => v.show_page(Page::Themes),
                    "link:theme_editor" => v.show_page(Page::ThemeEditor),
                    "link:export_setup" => (v.callbacks.export_setup)(),
                    "link:extensions" => v.show_page(Page::Extensions),
                    "link:toolchains" => v.show_page(Page::Toolchains),
                    "link:keybinds" => {
                        v.close();
                        (v.callbacks.show_keybinds)();
                    }
                    _ => {
                        v.close();
                        (v.callbacks.open_config)();
                    }
                }
            });
        }

        let weak = Rc::downgrade(self);
        self.search.connect_search_changed(move |_| {
            if let Some(v) = weak.upgrade() {
                v.filter();
            }
        });
        let weak = Rc::downgrade(self);
        self.nav.connect_row_selected(move |_, row| {
            let (Some(v), Some(row)) = (weak.upgrade(), row) else { return };
            if v.navigating.get() {
                return;
            }
            let Some(&(_, section, page)) = nav_entries().get(row.index() as usize) else { return };
            if let Some(section) = section {
                v.section.set(section);
            }
            if !v.search.text().is_empty() {
                v.navigating.set(true);
                v.search.set_text("");
                v.navigating.set(false);
            }
            v.set_page(page);
            if page == Page::General {
                v.filter();
                v.scroll.vadjustment().set_value(0.0);
            }
        });

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(v) = weak.upgrade() else { return glib::Propagation::Proceed };
            if key == gdk::Key::Escape {
                v.close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.root.add_controller(keys);

        // Only clicks outside the card close it: pressed and released there. A slider
        // dragged out of the card is released outside but was pressed inside.
        let click = gtk::GestureClick::new();
        let pressed_outside = Rc::new(Cell::new(false));
        let (weak, outside) = (Rc::downgrade(self), pressed_outside.clone());
        click.connect_pressed(move |_, _, x, y| {
            outside.set(weak.upgrade().is_some_and(|v| v.root.pick(x, y, gtk::PickFlags::DEFAULT).is_some_and(|w| w == v.root)));
        });
        let weak = Rc::downgrade(self);
        click.connect_released(move |gesture, _, x, y| {
            let Some(v) = weak.upgrade() else { return };
            if pressed_outside.replace(false) && v.root.pick(x, y, gtk::PickFlags::DEFAULT).is_some_and(|w| w == v.root) {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                v.close();
            }
        });
        self.root.add_controller(click);
    }

    pub fn is_open(&self) -> bool {
        crate::motion::shown(&self.root)
    }

    pub fn page(&self) -> Option<Page> {
        self.is_open().then(|| self.page.get())
    }

    /// Opens on `page` (or closes when already showing it), sized to the window.
    pub fn open(&self, page: Page, config: &Config) {
        self.refresh(config);
        let (w, h) = (self.root.parent().map_or(1100, |p| p.width()), self.root.parent().map_or(700, |p| p.height()));
        self.card.set_size_request((w - 80).clamp(320, 960), (h - 80).clamp(300, 680));
        self.navigating.set(true);
        self.search.set_text("");
        self.navigating.set(false);
        self.filter();
        self.show_page(page);
        crate::motion::fade_in(&self.root);
        match page {
            Page::General => self.search.grab_focus(),
            _ => self.stack.grab_focus(),
        };
    }

    pub fn close(&self) {
        if !self.is_open() {
            return;
        }
        crate::motion::fade_out(&self.root);
        (self.callbacks.on_close)();
    }

    fn show_page(&self, page: Page) {
        if page == Page::General {
            self.section.set(0);
        }
        self.select_nav(page);
        self.set_page(page);
        if page == Page::General {
            self.filter();
            self.scroll.vadjustment().set_value(0.0);
        }
    }

    /// Selects `page`'s nav row (the current section's for General) without acting on it.
    fn select_nav(&self, page: Page) {
        let section = self.section.get();
        let index = nav_entries().iter().position(|e| e.2 == page && (page != Page::General || e.1 == Some(section)));
        self.navigating.set(true);
        self.nav.select_row(index.and_then(|i| self.nav.row_at_index(i as i32)).as_ref());
        self.navigating.set(false);
    }

    fn set_page(&self, page: Page) {
        self.page.set(page);
        self.stack.set_visible_child_name(match page {
            Page::General => "general",
            Page::Themes => "themes",
            Page::ThemeEditor => "editor",
            Page::Extensions => "extensions",
            Page::Toolchains => "toolchains",
        });
        if page == Page::Toolchains {
            self.detect_toolchains();
        }
        (self.callbacks.page_shown)(page);
    }

    /// Fills the Toolchains page (detected off the main thread; each opening re-checks).
    fn detect_toolchains(&self) {
        while let Some(child) = self.toolchains.first_child() {
            self.toolchains.remove(&child);
        }
        // The project's own settings first (small files, read now), then the tools.
        if let Some((start, stop)) = (self.callbacks.project)() {
            fill_project(&self.toolchains, &start, stop.as_deref());
        }
        let heading = label("TOOLCHAINS", "heading");
        heading.set_margin_top(14);
        self.toolchains.append(&heading);
        let note = label("Found on this machine. TileIDE never installs anything; missing ones show the command that does.", "subtitle");
        note.set_wrap(true);
        note.set_margin_start(4);
        note.set_margin_bottom(6);
        self.toolchains.append(&note);
        let checking = label("Checking…", "subtitle");
        checking.set_margin_start(4);
        self.toolchains.append(&checking);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::toolchains::detect());
        });
        let page = self.toolchains.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
            let found = match rx.try_recv() {
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(_) => return glib::ControlFlow::Break,
                Ok(found) => found,
            };
            page.remove(&checking);
            for f in found {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
                row.add_css_class("item");
                let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
                text.set_hexpand(true);
                text.append(&label(&format!("{} · {}", f.name, f.used_for), "title"));
                let detail = match &f.detail {
                    Some(d) => label(d, "subtitle"),
                    None => {
                        let l = label(&format!("Missing · {}", f.install), "lock");
                        l.set_selectable(true);
                        l
                    }
                };
                detail.set_wrap(true);
                detail.set_wrap_mode(pango::WrapMode::WordChar);
                text.append(&detail);
                row.append(&text);
                page.append(&row);
            }
            glib::ControlFlow::Break
        });
    }

    /// Without a search, shows only the current section; with one, the rows of every
    /// section that match every typed word (on the General page).
    fn filter(&self) {
        let query = self.search.text().to_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();
        let mut shown_per_section = vec![0; SECTIONS.len()];
        for item in &self.items {
            let shown = if words.is_empty() { item.section == self.section.get() } else { words.iter().all(|w| item.text.contains(w)) };
            item.widget.set_visible(shown);
            if shown {
                shown_per_section[item.section] += 1;
            }
        }
        for (heading, n) in self.headings.iter().zip(&shown_per_section) {
            heading.set_visible(*n > 0);
        }
        self.empty.set_visible(shown_per_section.iter().all(|n| *n == 0));
        if !words.is_empty() && !self.navigating.get() {
            self.navigating.set(true);
            self.nav.unselect_all();
            self.navigating.set(false);
            self.set_page(Page::General);
            self.scroll.vadjustment().set_value(0.0);
        } else if words.is_empty() && self.page.get() == Page::General && self.nav.selected_row().is_none() {
            // The search was cleared: back on the section it left.
            self.select_nav(Page::General);
        }
    }

    /// Development (`--test-actions settings-nav:<row>`, `settings-nav:-1 <words>`, `settings-nav:-2`): selects nav row `row` and
    /// returns the page, the visible rows and how many sliders still take the wheel.
    /// Row -1 types `search` into the search field instead; -2 only reports.
    pub fn test_nav(&self, row: i32, search: &str) -> String {
        if row == -1 {
            self.search.set_text(search);
        } else if row >= 0 {
            self.nav.select_row(self.nav.row_at_index(row).as_ref());
        }
        let shown: Vec<&str> = self.items.iter().filter(|i| i.widget.is_visible()).map(|i| i.key.unwrap_or("link")).collect();
        let wheel = |s: &gtk::Scale| {
            let c = s.observe_controllers();
            (0..c.n_items()).filter(|i| c.item(*i).and_downcast::<gtk::EventControllerScroll>().is_some()).count()
        };
        let sliders = self.items.iter().filter_map(|i| match &i.control {
            Some(Control::Scale(s)) => Some(wheel(s)),
            _ => None,
        });
        let fresh = wheel(&gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.1));
        let row = self.nav.selected_row().map(|r| r.index());
        format!("page {:?} section {} nav {row:?} shown {shown:?} sliders-with-wheel {} (a plain GtkScale has {fresh})", self.page.get(), self.section.get(), sliders.sum::<usize>())
    }

    /// Shows `config`'s values; keys `tileide.lua` sets are locked.
    pub fn refresh(&self, config: &Config) {
        self.updating.set(true);
        for item in &self.items {
            let Some(key) = item.key else { continue };
            let locked = config.explicit.contains(key);
            item.lock.set_visible(locked);
            let Some(value) = settings::get(config, key) else { continue };
            match (&item.control, value) {
                (Some(Control::Switch(s)), Value::Bool(b)) => {
                    s.set_active(b);
                    s.set_sensitive(!locked);
                }
                (Some(Control::Scale(s)), Value::Number(n)) => {
                    s.set_value(n);
                    s.set_sensitive(!locked);
                }
                (Some(Control::Font(f)), Value::Text(family)) => {
                    let current = f.font_desc().and_then(|d| d.family()).map(|f| f.to_string());
                    if current.as_deref() != Some(family.as_str()) {
                        f.set_font_desc(&pango::FontDescription::from_string(&family));
                    }
                    f.set_sensitive(!locked);
                }
                (Some(Control::Choice(d, keys)), Value::Text(t)) => {
                    d.set_selected(keys.iter().position(|k| *k == t).unwrap_or(0) as u32);
                    d.set_sensitive(!locked);
                }
                _ => {}
            }
        }
        self.updating.set(false);
    }
}

/// While a slider in Settings is dragged, the whole card fades out so the change can be
/// seen on the tiles behind it; it comes back on release. A plain click doesn't hide it.
pub fn see_through_while_dragged(scale: &gtk::Scale) {
    const MOVED: f64 = 4.0;
    let drag = gtk::GestureDrag::new();
    drag.set_propagation_phase(gtk::PropagationPhase::Capture);
    let settings = |w: &gtk::Widget| std::iter::successors(w.parent(), gtk::Widget::parent).find(|p| p.has_css_class("settings"));
    drag.connect_drag_update(move |g, dx, dy| {
        if dx.hypot(dy) > MOVED
            && let Some(root) = g.widget().as_ref().and_then(settings)
        {
            root.set_opacity(0.0);
        }
    });
    let show = move |g: &gtk::GestureDrag| {
        if let Some(root) = g.widget().as_ref().and_then(settings).filter(|r| r.opacity() == 0.0) {
            root.set_opacity(1.0);
        }
    };
    drag.connect_drag_end(move |g, _, _| show(g));
    drag.connect_cancel(move |g, _| show(g));
    scale.add_controller(drag);
}

/// Removes GtkRange's wheel handler, so the wheel over a slider scrolls the page
/// instead of changing the value; dragging, clicks and keys still work.
pub fn ignore_wheel(scale: &gtk::Scale) {
    let controllers = scale.observe_controllers();
    let wheel: Vec<gtk::EventController> = (0..controllers.n_items()).filter_map(|i| controllers.item(i).and_downcast::<gtk::EventControllerScroll>()).map(|c| c.upcast()).collect();
    for c in wheel {
        scale.remove_controller(&c);
    }
}

fn build_item(key: &'static str, title: &str, subtitle: &str, section: usize) -> Item {
    let widget = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    widget.add_css_class("item");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let t = label(title, "title");
    t.set_wrap(true);
    text.append(&t);
    if !subtitle.is_empty() {
        let s = label(subtitle, "subtitle");
        s.set_wrap(true);
        text.append(&s);
    }
    let lock = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    lock.append(&label("Set in tileide.lua", "lock"));
    let unlock = gtk::Button::with_label("Unlock");
    unlock.add_css_class("unlock");
    unlock.set_tooltip_text(Some("Comment this line out in tileide.lua so Settings can change it"));
    lock.append(&unlock);
    lock.set_visible(false);
    text.append(&lock);
    widget.append(&text);

    if key.starts_with("link:") {
        let open = gtk::Button::with_label(match key {
            "link:keybinds" => "Show",
            "link:export_setup" => "Export…",
            _ => "Open",
        });
        open.add_css_class("pill");
        open.set_valign(gtk::Align::Center);
        open.set_widget_name(key);
        widget.append(&open);
        return Item { key: None, section, widget, text: String::new(), control: None, lock };
    }

    let control = match settings::kind(key) {
        Some(Kind::Bool) => {
            let s = gtk::Switch::new();
            s.set_valign(gtk::Align::Center);
            widget.append(&s);
            Control::Switch(s)
        }
        Some(Kind::Number(min, max)) => {
            let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, step(key));
            ignore_wheel(&scale);
            see_through_while_dragged(&scale);
            scale.set_draw_value(false);
            scale.set_size_request(180, -1);
            scale.set_valign(gtk::Align::Center);
            let value = label("", "value");
            value.set_width_chars(6);
            value.set_xalign(1.0);
            let v = value.clone();
            scale.connect_value_changed(move |s| v.set_text(&format_value(key, s.value())));
            widget.append(&scale);
            widget.append(&value);
            Control::Scale(scale)
        }
        Some(Kind::Choice(keys)) => {
            let labels: Vec<String> = keys.iter().map(|k| tileide_core::motion::Curve::from_key(k).map_or_else(|| capitalized(k), |c| c.label().to_string())).collect();
            let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            let d = gtk::DropDown::from_strings(&refs);
            d.set_valign(gtk::Align::Center);
            widget.append(&d);
            Control::Choice(d, keys)
        }
        Some(Kind::Text) | None => {
            let dialog = gtk::FontDialog::builder().title(title).build();
            if key == "appearance.code_font" {
                dialog.set_filter(Some(&gtk::CustomFilter::new(|o| o.downcast_ref::<pango::FontFamily>().is_none_or(|f| f.is_monospace()))));
            }
            let f = gtk::FontDialogButton::new(Some(dialog));
            f.set_level(gtk::FontLevel::Family);
            f.set_valign(gtk::Align::Center);
            widget.append(&f);
            Control::Font(f)
        }
    };
    Item { key: Some(key), section, widget, text: String::new(), control: Some(control), lock }
}

/// Settings → Toolchains, "This project": the Ruff, pyright and pytest settings the
/// project's files give (read-only; the tools read the files themselves).
fn fill_project(page: &gtk::Box, start: &Path, stop: Option<&Path>) {
    let dir = crate::py_settings::project_dir(start, stop);
    let sections = crate::py_settings::read(&dir);
    page.append(&label("THIS PROJECT", "heading"));
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let shown = home.as_ref().and_then(|h| dir.strip_prefix(h).ok()).map_or_else(|| dir.display().to_string(), |r| format!("~/{}", r.display()));
    let note = label(
        &if sections.is_empty() {
            format!("{shown} has no Ruff, pyright or pytest settings (pyproject.toml [tool.…], ruff.toml, pyrightconfig.json).")
        } else {
            format!("{shown}: what Ruff, pyright and pytest read from the project. Read-only here: edit the file to change them.")
        },
        "subtitle",
    );
    // Paths have no spaces to wrap at.
    note.set_wrap(true);
    note.set_wrap_mode(pango::WrapMode::WordChar);
    note.set_margin_start(4);
    note.set_margin_bottom(6);
    page.append(&note);
    // Its Python: no subprocess here, just which one would run.
    if let Some(python) = tileide_core::python::candidates(start, stop).into_iter().next() {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
        row.add_css_class("item");
        row.append(&label("Python · Run, tests, notebooks, pyright", "title"));
        let path = label(&format!("{} · Alt+P picks another", python.display()), "subtitle");
        path.set_wrap(true);
        path.set_wrap_mode(pango::WrapMode::WordChar);
        row.append(&path);
        page.append(&row);
    }
    for s in sections {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
        row.add_css_class("item");
        row.append(&label(&s.title, "title"));
        let lines: Vec<String> = s.entries.iter().map(|(k, v)| format!("{k} = {v}")).collect();
        let body = label(&lines.join("\n"), "subtitle");
        body.add_css_class("monospace");
        body.set_selectable(true);
        body.set_wrap(true);
        body.set_wrap_mode(pango::WrapMode::WordChar);
        row.append(&body);
        page.append(&row);
    }
}
