// SPDX-License-Identifier: GPL-3.0-or-later
//! The Themes & background page of Settings (design.md S1d + S1f, v0.3 phase 7; in the drawer until v0.4): the local themes
//! (hover or select one to preview it on the real window, click or Enter to apply),
//! applied to every workspace or only this one, the VS Code theme import, and the
//! background picker (modes, sliders, legibility, live). The app owns what applies;
//! this only shows choices and reports them.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4 as gtk;
use tileide_config::model::Background;
use tileide_core::theme::{self, Rgb, Surfaces, Theme};

/// Hovering a row this long previews it, so sweeping the pointer across doesn't flash.
const PREVIEW_DWELL: Duration = Duration::from_millis(250);
/// Sliders settle this long before the background is rebuilt.
const BACKGROUND_DELAY: Duration = Duration::from_millis(120);
const MODES: [(&str, &str); 4] = [("see-through", "None"), ("image", "Image"), ("gradient", "Gradient"), ("solid", "Solid")];
const FITS: [&str; 3] = ["cover", "contain", "tile"];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scope {
    All,
    Workspace(u8),
}

type OnPreview = Rc<dyn Fn(Option<&str>)>;
type OnApply = Rc<dyn Fn(&str, Scope)>;

pub struct Callbacks {
    /// `None` ends the preview.
    pub preview: OnPreview,
    pub apply: OnApply,
    pub background: Rc<dyn Fn(Background, Scope)>,
    pub import: Rc<dyn Fn()>,
    /// A Sidebar / Tiles & terminals slider moved: (settings key, value).
    pub surface: Rc<dyn Fn(&'static str, f64)>,
}

/// What the section shows; the app builds it on every refresh.
pub struct Snapshot {
    /// (file stem, theme), sorted.
    pub themes: Vec<(String, Theme)>,
    /// The theme in effect (not a preview).
    pub current: String,
    /// The active numbered workspace; `None` in the Config workspace.
    pub workspace: Option<u8>,
    /// Whether the active workspace has its own picks (the scope then starts there).
    pub scope: Scope,
    pub background: Background,
    /// Backgrounds are drawn only in full-screen mode.
    pub fullscreen: bool,
    /// Why a pick may not show (the config pins something), if anything.
    pub note: Option<String>,
    /// Sidebar and tiles as they are now (the theme's look until the user moves a slider).
    pub surfaces: Surfaces,
    /// Full-screen over a background TileIDE draws or a wallpaper it found, so it can
    /// blur under the sidebar and tiles.
    pub can_blur: bool,
    /// Surface keys `tileide.lua` sets (their sliders are locked).
    pub locked: Vec<&'static str>,
}

/// The four surface sliders' settings keys, in `Controls::surface` order.
pub const SURFACE_KEYS: [&str; 4] = ["sidebar.transparency", "sidebar.blur", "appearance.tile_transparency", "appearance.tile_blur"];

pub fn css(t: &Theme) -> String {
    let c = theme::css;
    format!(
        ".themes {{ padding: 4px 10px 14px 12px; }}
.themes .section {{ font-weight: 600; color: {muted}; font-size: 11px; letter-spacing: 1px; margin: 14px 2px 6px 2px; }}
.themes .note {{ color: {muted}; font-size: 12px; margin: 2px 2px 6px 2px; }}
.themes .warn {{ color: {error}; }}
.themes list {{ background: transparent; }}
.themes row {{ border-radius: 8px; padding: 5px 6px; color: {text}; }}
.themes row:hover {{ background-color: {hover}; }}
.themes row:selected {{ background-color: {select}; color: {text}; }}
.themes row image {{ color: {accent}; }}
.themes button.pill {{ min-height: 26px; padding: 0 10px; border-radius: 13px; border: 1px solid {edge}; background: transparent; box-shadow: none; color: {text}; font-size: 12px; }}
.themes button.pill:checked {{ background-color: {accent}; border-color: {accent}; color: {on_accent}; }}
.themes button.pill:hover:not(:checked) {{ background-color: {hover}; }}
.themes button.pill:focus-visible, .themes row:focus-visible, .themes scale:focus-visible slider {{ outline: 2px solid {accent}; outline-offset: 1px; }}
.themes scale trough {{ background-color: {edge}; min-height: 4px; border-radius: 2px; }}
.themes scale highlight {{ background-color: {accent}; border-radius: 2px; }}
.themes scale slider {{ background-color: {accent}; min-width: 14px; min-height: 14px; margin: -5px; border-radius: 7px; border: none; box-shadow: none; }}
.themes .value {{ color: {muted}; font-size: 12px; }}
.drawer .switcher {{ padding: 8px 8px 0 10px; }}
.drawer flowbox.flow, .drawer flowbox.flow flowboxchild {{ background: transparent; padding: 0; min-width: 0; }}",
        text = c(t.text, 1.0),
        muted = c(t.muted, 1.0),
        error = c(t.error, 1.0),
        accent = c(t.accent, 1.0),
        on_accent = c(t.on_accent, 1.0),
        edge = c(t.text, 0.14),
        hover = c(t.text, 0.06),
        select = c(t.accent, 0.18),
    )
}

struct Controls {
    scope_all: gtk::ToggleButton,
    scope_workspace: gtk::ToggleButton,
    note: gtk::Label,
    list: gtk::ListBox,
    modes: Vec<gtk::ToggleButton>,
    image_row: gtk::FlowBox,
    image_button: gtk::Button,
    fit: gtk::DropDown,
    gradient_row: gtk::FlowBox,
    gradient_top: gtk::ColorDialogButton,
    gradient_bottom: gtk::ColorDialogButton,
    solid_row: gtk::Box,
    solid: gtk::ColorDialogButton,
    /// Sidebar transparency, sidebar blur, tile transparency, tile blur (`SURFACE_KEYS`).
    surface: [gtk::Scale; 4],
    blur_note: gtk::Label,
    picker: gtk::Box,
    windowed_note: gtk::Label,
    legibility: gtk::Label,
}

pub struct ThemesPanel {
    root: gtk::ScrolledWindow,
    c: Controls,
    callbacks: Callbacks,
    /// Row index → theme file stem.
    keys: RefCell<Vec<String>>,
    workspace: Cell<Option<u8>>,
    /// The background being edited (kept across mode switches, e.g. the image path).
    background: RefCell<Background>,
    /// Set while the app refreshes the controls, so they don't report back.
    updating: Cell<bool>,
    dwell: RefCell<Option<glib::SourceId>>,
    pending: RefCell<Option<glib::SourceId>>,
}

impl ThemesPanel {
    pub fn new(callbacks: Callbacks) -> Rc<ThemesPanel> {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.add_css_class("themes");

        let scope_all = pill("All workspaces");
        let scope_workspace = pill("This workspace");
        scope_workspace.set_group(Some(&scope_all));
        scope_all.set_active(true);
        let scope = flow_row(&[scope_all.upcast_ref(), scope_workspace.upcast_ref()]);
        scope.set_margin_top(6);
        content.append(&scope);
        let note = gtk::Label::new(None);
        note.add_css_class("note");
        note.set_wrap(true);
        note.set_xalign(0.0);
        note.set_visible(false);
        content.append(&note);

        content.append(&section("THEMES"));
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Browse);
        content.append(&list);
        let import = gtk::Button::with_label("Import VS Code theme…");
        import.add_css_class("pill");
        import.set_halign(gtk::Align::Start);
        import.set_margin_top(8);
        content.append(&import);

        content.append(&section("BACKGROUND"));
        let windowed_note = gtk::Label::new(Some("Backgrounds are drawn in full-screen mode (tileide -f); windowed, the desktop shows through."));
        windowed_note.add_css_class("note");
        windowed_note.set_wrap(true);
        windowed_note.set_xalign(0.0);
        content.append(&windowed_note);
        let picker = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let modes: Vec<gtk::ToggleButton> = MODES.iter().map(|(_, label)| pill(label)).collect();
        for m in &modes[1..] {
            m.set_group(Some(&modes[0]));
        }
        let mode_widgets: Vec<&gtk::Widget> = modes.iter().map(|m| m.upcast_ref()).collect();
        picker.append(&flow_row(&mode_widgets));

        let image_button = gtk::Button::with_label("Choose image…");
        image_button.add_css_class("pill");
        let fit = gtk::DropDown::from_strings(&FITS);
        let image_row = flow_row(&[image_button.upcast_ref(), fit.upcast_ref()]);
        picker.append(&image_row);
        let colour = || {
            let b = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::builder().with_alpha(false).build()));
            b.set_valign(gtk::Align::Center);
            b
        };
        let (gradient_top, gradient_bottom) = (colour(), colour());
        let top = row_of(&[label("Top").upcast_ref(), gradient_top.upcast_ref()]);
        let bottom = row_of(&[label("Bottom").upcast_ref(), gradient_bottom.upcast_ref()]);
        let gradient_row = flow_row(&[top.upcast_ref(), bottom.upcast_ref()]);
        picker.append(&gradient_row);
        let solid = colour();
        let solid_row = row_of(&[label("Colour").upcast_ref(), solid.upcast_ref()]);
        picker.append(&solid_row);

        content.append(&picker);

        // See-through and blur, in every mode (blur needs a background TileIDE draws).
        let max = (tileide_config::settings::TRANSPARENCY_MAX, tileide_config::settings::BLUR_MAX);
        content.append(&section("SIDEBAR"));
        let (sidebar_t, row) = slider("Transparency", 0.0, max.0, 1.0, "%");
        content.append(&row);
        let (sidebar_b, row) = slider("Blur", 0.0, max.1, 1.0, " px");
        content.append(&row);
        content.append(&section("TILES & TERMINALS"));
        let (tile_t, row) = slider("Transparency", 0.0, max.0, 1.0, "%");
        content.append(&row);
        let (tile_b, row) = slider("Blur", 0.0, max.1, 1.0, " px");
        content.append(&row);
        let blur_note = gtk::Label::new(Some("Blur works full-screen: over Image, Gradient or Solid, and over See-through too, where TileIDE blurs your desktop's wallpaper (omarchy, swww, hyprpaper or swaybg) under the tiles. Windowed, blur comes from your compositor, e.g. Hyprland's decoration:blur."));
        blur_note.add_css_class("note");
        blur_note.set_wrap(true);
        blur_note.set_xalign(0.0);
        content.append(&blur_note);
        let legibility = gtk::Label::new(None);
        legibility.add_css_class("note");
        legibility.set_wrap(true);
        legibility.set_xalign(0.0);
        content.append(&legibility);

        let root = gtk::ScrolledWindow::new();
        root.set_hscrollbar_policy(gtk::PolicyType::Never);
        root.set_vexpand(true);
        root.set_child(Some(&content));

        let panel = Rc::new(ThemesPanel {
            root,
            c: Controls { scope_all, scope_workspace, note, list, modes, image_row, image_button, fit, gradient_row, gradient_top, gradient_bottom, solid_row, solid, surface: [sidebar_t, sidebar_b, tile_t, tile_b], blur_note, picker, windowed_note, legibility },
            callbacks,
            keys: RefCell::new(Vec::new()),
            workspace: Cell::new(None),
            background: RefCell::new(tileide_config::Config::default().background),
            updating: Cell::new(false),
            dwell: RefCell::new(None),
            pending: RefCell::new(None),
        });
        panel.connect(&import);
        panel
    }

    pub fn widget(&self) -> &gtk::ScrolledWindow {
        &self.root
    }

    /// Test action `themes-select:<n>`: focuses row `n` and selects it, as the arrow keys do.
    pub fn test_select(&self, n: i32) {
        if let Some(row) = self.c.list.row_at_index(n) {
            row.grab_focus();
            self.c.list.select_row(Some(&row));
        }
    }

    /// Keyboard focus on the theme in use: ↑/↓ preview, Enter applies.
    pub fn focus(&self) {
        match self.c.list.selected_row() {
            Some(row) => row.grab_focus(),
            None => self.c.list.grab_focus(),
        };
    }

    fn connect(self: &Rc<Self>, import: &gtk::Button) {
        let c = &self.c;
        let weak = Rc::downgrade(self);
        // Keys are copied out first: applying refreshes this panel, which rewrites them.
        c.list.connect_row_activated(move |_, row| {
            let Some(p) = weak.upgrade() else { return };
            let key = p.keys.borrow().get(row.index() as usize).cloned();
            if let Some(key) = key {
                (p.callbacks.apply)(&key, p.scope());
            }
        });
        // Keyboard selection previews at once; the pointer previews after a short dwell.
        let weak = Rc::downgrade(self);
        c.list.connect_row_selected(move |list, row| {
            let (Some(p), Some(row)) = (weak.upgrade(), row) else { return };
            if p.updating.get() || !list.has_focus() && !row.has_focus() {
                return;
            }
            let key = p.keys.borrow().get(row.index() as usize).cloned();
            if let Some(key) = key {
                (p.callbacks.preview)(Some(&key));
            }
        });
        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(self);
        motion.connect_motion(move |m, _, y| {
            let Some(p) = weak.upgrade() else { return };
            let Some(list) = m.widget().and_downcast::<gtk::ListBox>() else { return };
            let Some(row) = list.row_at_y(y as i32) else { return };
            let Some(key) = p.keys.borrow().get(row.index() as usize).cloned() else { return };
            if let Some(old) = p.dwell.take() {
                old.remove();
            }
            let w = Rc::downgrade(&p);
            *p.dwell.borrow_mut() = Some(glib::timeout_add_local_once(PREVIEW_DWELL, move || {
                if let Some(p) = w.upgrade() {
                    p.dwell.take();
                    (p.callbacks.preview)(Some(&key));
                }
            }));
        });
        let weak = Rc::downgrade(self);
        motion.connect_leave(move |_| {
            if let Some(p) = weak.upgrade() {
                p.end_preview_later();
            }
        });
        c.list.add_controller(motion);
        let focus = gtk::EventControllerFocus::new();
        let weak = Rc::downgrade(self);
        focus.connect_leave(move |_| {
            if let Some(p) = weak.upgrade() {
                p.end_preview_later();
            }
        });
        c.list.add_controller(focus);

        let weak = Rc::downgrade(self);
        import.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                (p.callbacks.import)();
            }
        });

        for (i, m) in c.modes.iter().enumerate() {
            let weak = Rc::downgrade(self);
            m.connect_toggled(move |b| {
                let Some(p) = weak.upgrade().filter(|p| !p.updating.get() && b.is_active()) else { return };
                p.set_mode(MODES[i].0);
            });
        }
        let weak = Rc::downgrade(self);
        c.image_button.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.choose_image();
            }
        });
        let weak = Rc::downgrade(self);
        c.fit.connect_selected_notify(move |d| {
            if let Some(p) = weak.upgrade().filter(|p| !p.updating.get()) {
                p.background.borrow_mut().fit = FITS[d.selected() as usize % FITS.len()].into();
                p.changed();
            }
        });
        for button in [&c.gradient_top, &c.gradient_bottom, &c.solid] {
            let weak = Rc::downgrade(self);
            button.connect_rgba_notify(move |_| {
                if let Some(p) = weak.upgrade().filter(|p| !p.updating.get()) {
                    p.read_colours();
                    p.changed();
                }
            });
        }
        for (scale, key) in c.surface.iter().zip(SURFACE_KEYS) {
            let weak = Rc::downgrade(self);
            scale.connect_value_changed(move |s| {
                if let Some(p) = weak.upgrade().filter(|p| !p.updating.get()) {
                    (p.callbacks.surface)(key, s.value().round());
                }
            });
        }
        let weak = Rc::downgrade(self);
        c.scope_workspace.connect_toggled(move |_| {
            if let Some(p) = weak.upgrade().filter(|p| !p.updating.get()) {
                p.changed();
            }
        });
    }

    fn scope(&self) -> Scope {
        match (self.c.scope_workspace.is_active(), self.workspace.get()) {
            (true, Some(n)) => Scope::Workspace(n),
            _ => Scope::All,
        }
    }

    /// Called from leave handlers, which GTK runs inside whatever moved the focus or
    /// pointer away (often a tile grabbing focus while the app's state is borrowed), so
    /// the preview ends from the main loop instead (0.5.0-alpha.5 crash, `RefCell
    /// already borrowed` via `App::refresh`).
    fn end_preview_later(self: &Rc<Self>) {
        if let Some(old) = self.dwell.take() {
            old.remove();
        }
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(p) = weak.upgrade() {
                (p.callbacks.preview)(None);
            }
        });
    }

    /// Shows `s` without reporting anything back.
    pub fn refresh(&self, s: Snapshot) {
        self.updating.set(true);
        let c = &self.c;
        self.workspace.set(s.workspace);
        c.scope_workspace.set_label(&s.workspace.map_or_else(|| "This workspace".into(), |n| format!("Workspace {}", crate::workspaces::display(n))));
        c.scope_workspace.set_sensitive(s.workspace.is_some());
        match s.scope {
            Scope::Workspace(_) if s.workspace.is_some() => c.scope_workspace.set_active(true),
            _ => c.scope_all.set_active(true),
        }
        c.note.set_text(s.note.as_deref().unwrap_or(""));
        c.note.set_visible(s.note.is_some());

        c.list.remove_all();
        for (key, t) in &s.themes {
            c.list.append(&theme_row(t, *key == s.current));
        }
        if let Some(i) = s.themes.iter().position(|(k, _)| *k == s.current) {
            c.list.select_row(c.list.row_at_index(i as i32).as_ref());
        }
        *self.keys.borrow_mut() = s.themes.into_iter().map(|(k, _)| k).collect();

        c.windowed_note.set_visible(!s.fullscreen);
        c.picker.set_sensitive(s.fullscreen);
        let values = [
            (1.0 - s.surfaces.sidebar.opacity) * 100.0,
            s.surfaces.sidebar.blur,
            (1.0 - s.surfaces.tiles.opacity) * 100.0,
            s.surfaces.tiles.blur,
        ];
        for (i, (scale, value)) in c.surface.iter().zip(values).enumerate() {
            scale.set_value(value.round());
            let blur = i % 2 == 1;
            let locked = s.locked.contains(&SURFACE_KEYS[i]);
            scale.set_sensitive(!locked && (!blur || s.can_blur));
            scale.set_tooltip_text(locked.then_some("Set in tileide.lua"));
        }
        c.blur_note.set_visible(!s.can_blur);
        self.show_background(&s.background);
        *self.background.borrow_mut() = s.background;
        self.updating.set(false);
    }

    fn show_background(&self, bg: &Background) {
        let c = &self.c;
        let mode = MODES.iter().position(|(m, _)| *m == bg.mode).unwrap_or(0);
        c.modes[mode].set_active(true);
        c.image_row.set_visible(bg.mode == "image");
        c.gradient_row.set_visible(bg.mode == "gradient");
        c.solid_row.set_visible(bg.mode == "solid");
        if bg.mode == "image" {
            let name = bg.source.as_deref().and_then(|s| std::path::Path::new(s).file_name().map(|n| n.to_string_lossy().into_owned()));
            c.image_button.set_label(name.as_deref().unwrap_or("Choose image…"));
        }
        c.fit.set_selected(FITS.iter().position(|f| *f == bg.fit).unwrap_or(0) as u32);
        let colours: Vec<Rgb> = bg.source.as_deref().unwrap_or("").split(',').filter_map(|h| parse_hex(h.trim())).collect();
        if bg.mode == "gradient" && colours.len() == 2 {
            c.gradient_top.set_rgba(&rgba(colours[0]));
            c.gradient_bottom.set_rgba(&rgba(colours[1]));
        }
        if bg.mode == "solid"
            && let Some(&colour) = colours.first()
        {
            c.solid.set_rgba(&rgba(colour));
        }
    }

    /// The background's contrast report (from the last bake), or `None` to hide it.
    pub fn set_legibility(&self, text: Option<&str>, warn: bool) {
        let l = &self.c.legibility;
        if l.text() != text.unwrap_or("") {
            l.set_text(text.unwrap_or(""));
        }
        l.set_visible(text.is_some());
        if warn { l.add_css_class("warn") } else { l.remove_css_class("warn") }
    }

    fn set_mode(self: &Rc<Self>, mode: &str) {
        let needs_image = {
            let mut bg = self.background.borrow_mut();
            let keeps_source = bg.mode == mode || (mode == "image" && bg.source.as_deref().is_some_and(|s| parse_hex(s.split(',').next().unwrap_or("")).is_none()));
            bg.mode = mode.into();
            if !keeps_source {
                bg.source = match mode {
                    "gradient" => Some(format!("{},{}", hex(self.c.gradient_top.rgba()), hex(self.c.gradient_bottom.rgba()))),
                    "solid" => Some(hex(self.c.solid.rgba())),
                    _ => None,
                };
            }
            mode == "image" && bg.source.is_none()
        };
        self.updating.set(true);
        self.show_background(&self.background.borrow());
        self.updating.set(false);
        if needs_image {
            self.choose_image();
        } else {
            self.changed();
        }
    }

    fn read_colours(&self) {
        let mut bg = self.background.borrow_mut();
        bg.source = match bg.mode.as_str() {
            "gradient" => Some(format!("{},{}", hex(self.c.gradient_top.rgba()), hex(self.c.gradient_bottom.rgba()))),
            "solid" => Some(hex(self.c.solid.rgba())),
            _ => bg.source.clone(),
        };
    }

    fn choose_image(self: &Rc<Self>) {
        let Some(window) = self.root.root().and_downcast::<gtk::Window>() else { return };
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Images"));
        for t in ["image/png", "image/jpeg", "image/webp", "image/gif", "image/bmp"] {
            filter.add_mime_type(t);
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let weak = Rc::downgrade(self);
        gtk::FileDialog::builder().title("Background image").filters(&filters).build().open(Some(&window), None::<&gio::Cancellable>, move |result| {
            let Some(p) = weak.upgrade() else { return };
            match result.ok().and_then(|f| f.path()) {
                Some(path) => {
                    p.background.borrow_mut().source = Some(path.to_string_lossy().into_owned());
                    p.updating.set(true);
                    p.show_background(&p.background.borrow());
                    p.updating.set(false);
                    p.changed();
                }
                // Cancelled with no image yet: back to no background.
                None if p.background.borrow().source.is_none() => p.set_mode("see-through"),
                None => {}
            }
        });
    }

    /// Reports the edited background once the controls settle.
    fn changed(self: &Rc<Self>) {
        if let Some(old) = self.pending.take() {
            old.remove();
        }
        let weak = Rc::downgrade(self);
        *self.pending.borrow_mut() = Some(glib::timeout_add_local_once(BACKGROUND_DELAY, move || {
            if let Some(p) = weak.upgrade() {
                p.pending.take();
                let bg = p.background.borrow().clone();
                (p.callbacks.background)(bg, p.scope());
            }
        }));
    }
}

fn pill(text: &str) -> gtk::ToggleButton {
    let b = gtk::ToggleButton::with_label(text);
    b.add_css_class("pill");
    b
}

fn label(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("value");
    l
}

fn section(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("section");
    l.set_xalign(0.0);
    l
}

fn row_of(children: &[&gtk::Widget]) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    for c in children {
        row.append(*c);
    }
    row
}

/// A row that wraps onto more lines instead of widening the drawer (its minimum width
/// is its widest child's).
pub fn flow_row(children: &[&gtk::Widget]) -> gtk::FlowBox {
    let row = gtk::FlowBox::new();
    row.add_css_class("flow");
    // Packed to the left; a FlowBox spreads extra width between its children otherwise.
    row.set_halign(gtk::Align::Start);
    row.set_selection_mode(gtk::SelectionMode::None);
    row.set_max_children_per_line(children.len().max(1) as u32);
    row.set_column_spacing(4);
    row.set_row_spacing(4);
    for c in children {
        row.append(*c);
        if let Some(child) = c.parent() {
            child.set_focusable(false);
            child.set_halign(gtk::Align::Start);
        }
    }
    row
}

/// A labelled slider with its value; returns the scale and the row.
fn slider(name: &str, min: f64, max: f64, step: f64, unit: &'static str) -> (gtk::Scale, gtk::Box) {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, step);
    crate::settings_ui::ignore_wheel(&scale);
    crate::settings_ui::see_through_while_dragged(&scale);
    scale.set_hexpand(true);
    scale.set_draw_value(false);
    let value = label("");
    value.set_width_chars(6);
    value.set_xalign(1.0);
    value.set_text(&format!("{}{unit}", min.round()));
    let v = value.clone();
    scale.connect_value_changed(move |s| v.set_text(&format!("{}{unit}", s.value().round())));
    let name = label(name);
    name.set_width_chars(12);
    name.set_xalign(0.0);
    let row = row_of(&[name.upcast_ref(), scale.upcast_ref(), value.upcast_ref()]);
    (scale, row)
}

/// The theme's strip (surface, mid tone, accent) and its name; a check on the one in use.
fn theme_row(t: &Theme, current: bool) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let strip = gtk::DrawingArea::new();
    strip.set_content_width(42);
    strip.set_content_height(16);
    strip.set_valign(gtk::Align::Center);
    let colours = t.strip;
    strip.set_draw_func(move |_, cr, w, h| {
        let (w, h) = (w as f64, h as f64);
        let r = h / 2.0;
        cr.new_sub_path();
        cr.arc(w - r, r, r, -std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);
        cr.arc(r, r, r, std::f64::consts::FRAC_PI_2, 3.0 * std::f64::consts::FRAC_PI_2);
        cr.close_path();
        cr.clip();
        for (i, c) in colours.iter().enumerate() {
            cr.set_source_rgb(c.0, c.1, c.2);
            cr.rectangle(w * i as f64 / 3.0, 0.0, w / 3.0 + 1.0, h);
            let _ = cr.fill();
        }
    });
    row.append(&strip);
    let name = gtk::Label::new(Some(&t.name));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&name);
    if current {
        let check = gtk::Image::from_icon_name("object-select-symbolic");
        check.set_tooltip_text(Some("In use"));
        row.append(&check);
    }
    row
}

fn rgba(c: Rgb) -> gdk::RGBA {
    gdk::RGBA::new(c.0 as f32, c.1 as f32, c.2 as f32, 1.0)
}

/// `RRGGBB`, as `tileide.background` writes colours.
fn hex(c: gdk::RGBA) -> String {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("{:02X}{:02X}{:02X}", byte(c.red()), byte(c.green()), byte(c.blue()))
}

fn parse_hex(text: &str) -> Option<Rgb> {
    let text = text.trim_start_matches('#');
    (text.len() == 6).then(|| u32::from_str_radix(text, 16).ok()).flatten().map(theme::rgb)
}
