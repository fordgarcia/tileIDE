// SPDX-License-Identifier: GPL-3.0-or-later
//! Settings → Theme editor (v0.5 phase 4b): a draft that starts from the current theme;
//! every change previews live while the page is shown. Save writes it to the user's
//! `themes/` and applies it; Export writes the file anywhere. Glass, syntax and terminal
//! colours come along from the theme it started from.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::gdk;
use gtk::prelude::*;
use gtk4 as gtk;
use tileide_core::theme::{Rgb, Theme};

/// The colour roles shown, in order: (title, subtitle).
const ROLES: [(&str, &str); 8] = [
    ("Frame", "The rail, the drawer and the window around the tiles"),
    ("Tiles", "Editors, terminals and notebooks"),
    ("Text", ""),
    ("Muted text", "Hints, line numbers, secondary labels"),
    ("Accent", "Focus, selection, switches and sliders"),
    ("Text on accent", "Labels on accent-filled buttons"),
    ("Errors", "Diagnostics and failed runs"),
    ("Tray middle tone", "The middle of the theme tray's colour strip"),
];

fn role(theme: &Theme, i: usize) -> Rgb {
    [theme.surface, theme.tile, theme.text, theme.muted, theme.accent, theme.on_accent, theme.error, theme.strip[1]][i]
}

fn set_role(theme: &mut Theme, i: usize, c: Rgb) {
    match i {
        0 => {
            theme.surface = c;
            theme.strip[0] = c;
        }
        1 => theme.tile = c,
        2 => theme.text = c,
        3 => theme.muted = c,
        4 => {
            theme.accent = c;
            theme.strip[2] = c;
        }
        5 => theme.on_accent = c,
        6 => theme.error = c,
        _ => theme.strip[1] = c,
    }
}

pub struct Callbacks {
    /// The theme in use, without the draft (a new draft starts from it).
    pub current: Rc<dyn Fn() -> Theme>,
    /// Show this draft live; `None` ends the preview.
    pub preview: Rc<dyn Fn(Option<Theme>)>,
    pub save: Rc<dyn Fn(Theme)>,
    pub export: Rc<dyn Fn(Theme)>,
}

pub struct ThemeEditor {
    root: gtk::ScrolledWindow,
    name: gtk::Entry,
    mode: gtk::DropDown,
    colours: Vec<gtk::ColorDialogButton>,
    status: gtk::Label,
    /// Kept when Settings closes, so reopening the page carries on.
    draft: RefCell<Option<Theme>>,
    /// Set while the page fills its controls, so they don't report back.
    updating: Cell<bool>,
    callbacks: Callbacks,
}

fn label(text: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class(class);
    l.set_xalign(0.0);
    l.set_wrap(true);
    l
}

/// A settings row: title and subtitle on the left, `control` on the right.
fn item(title: &str, subtitle: &str, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("item");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    text.append(&label(title, "title"));
    if !subtitle.is_empty() {
        text.append(&label(subtitle, "subtitle"));
    }
    row.append(&text);
    control.set_valign(gtk::Align::Center);
    row.append(control);
    row
}

fn rgba((r, g, b): Rgb) -> gdk::RGBA {
    gdk::RGBA::new(r as f32, g as f32, b as f32, 1.0)
}

impl ThemeEditor {
    pub fn new(callbacks: Callbacks) -> Rc<ThemeEditor> {
        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        page.add_css_class("page");
        page.append(&label("THEME EDITOR", "heading"));
        let intro = label("Starts from the theme in use. Changes show live while this page is open; Save keeps them as a theme of your own.", "subtitle");
        intro.set_margin_start(4);
        intro.set_margin_bottom(6);
        page.append(&intro);

        let name = gtk::Entry::new();
        name.set_width_chars(22);
        page.append(&item("Name", "Saved as themes/<name>.lua in your config folder", &name));
        let mode = gtk::DropDown::from_strings(&["Dark", "Light"]);
        page.append(&item("Mode", "Picks the editor's dark or light syntax colours", &mode));

        page.append(&label("COLOURS", "heading"));
        let mut colours = Vec::new();
        for (title, subtitle) in ROLES {
            let dialog = gtk::ColorDialog::builder().title(title).with_alpha(false).build();
            let button = gtk::ColorDialogButton::new(Some(dialog));
            page.append(&item(title, subtitle, &button));
            colours.push(button);
        }

        let save = gtk::Button::with_label("Save and use");
        let export = gtk::Button::with_label("Export…");
        let reset = gtk::Button::with_label("Start over");
        reset.set_tooltip_text(Some("Discard the draft and start again from the theme in use"));
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.set_margin_top(10);
        for b in [&save, &export, &reset] {
            b.add_css_class("pill");
            buttons.append(b);
        }
        page.append(&buttons);
        let status = label("", "subtitle");
        status.set_margin_start(4);
        status.set_margin_top(6);
        status.set_selectable(true);
        page.append(&status);

        let root = gtk::ScrolledWindow::new();
        root.set_hscrollbar_policy(gtk::PolicyType::Never);
        root.set_child(Some(&page));
        let editor = Rc::new(ThemeEditor { root, name, mode, colours, status, draft: RefCell::new(None), updating: Cell::new(false), callbacks });
        editor.connect(&save, &export, &reset);
        editor
    }

    fn connect(self: &Rc<Self>, save: &gtk::Button, export: &gtk::Button, reset: &gtk::Button) {
        let weak = Rc::downgrade(self);
        self.name.connect_changed(move |e| {
            if let Some(ed) = weak.upgrade() {
                let name = e.text().trim().to_string();
                ed.edit(|t| t.name = name);
            }
        });
        let weak = Rc::downgrade(self);
        self.mode.connect_selected_notify(move |d| {
            if let Some(ed) = weak.upgrade() {
                let dark = d.selected() == 0;
                ed.edit(|t| t.dark = dark);
            }
        });
        for (i, button) in self.colours.iter().enumerate() {
            let weak = Rc::downgrade(self);
            button.connect_rgba_notify(move |b| {
                if let Some(ed) = weak.upgrade() {
                    let c = b.rgba();
                    ed.edit(|t| set_role(t, i, (c.red() as f64, c.green() as f64, c.blue() as f64)));
                }
            });
        }
        let weak = Rc::downgrade(self);
        save.connect_clicked(move |_| {
            if let Some(ed) = weak.upgrade()
                && let Some(theme) = ed.valid_draft()
            {
                (ed.callbacks.save)(theme);
            }
        });
        let weak = Rc::downgrade(self);
        export.connect_clicked(move |_| {
            if let Some(ed) = weak.upgrade()
                && let Some(theme) = ed.valid_draft()
            {
                (ed.callbacks.export)(theme);
            }
        });
        let weak = Rc::downgrade(self);
        reset.connect_clicked(move |_| {
            if let Some(ed) = weak.upgrade() {
                ed.draft.borrow_mut().take();
                ed.set_status("");
                ed.shown();
            }
        });
    }

    pub fn widget(&self) -> &gtk::ScrolledWindow {
        &self.root
    }

    /// The page became visible: carry on with the draft, or start one from the theme in use.
    pub fn shown(&self) {
        let draft = self.draft.borrow().clone().unwrap_or_else(|| {
            let current = (self.callbacks.current)();
            let mut t = current.clone();
            t.name = format!("{} (edited)", current.name);
            t
        });
        self.fill(&draft);
        *self.draft.borrow_mut() = Some(draft.clone());
        (self.callbacks.preview)(Some(draft));
    }

    /// The page was left or Settings closed: the theme in use shows again.
    pub fn hidden(&self) {
        (self.callbacks.preview)(None);
    }

    /// After Save: editing carries on from the saved theme (saving again replaces it).
    pub fn saved(&self, theme: Theme, message: &str) {
        self.fill(&theme);
        *self.draft.borrow_mut() = Some(theme);
        self.set_status(message);
    }

    pub fn set_status(&self, text: &str) {
        self.status.set_text(text);
        self.status.set_visible(!text.is_empty());
    }

    fn fill(&self, theme: &Theme) {
        self.updating.set(true);
        self.name.set_text(&theme.name);
        self.mode.set_selected(if theme.dark { 0 } else { 1 });
        for (i, button) in self.colours.iter().enumerate() {
            button.set_rgba(&rgba(role(theme, i)));
        }
        self.updating.set(false);
    }

    fn edit(&self, change: impl FnOnce(&mut Theme)) {
        if self.updating.get() {
            return;
        }
        let draft = {
            let mut draft = self.draft.borrow_mut();
            let Some(theme) = draft.as_mut() else { return };
            change(theme);
            theme.clone()
        };
        (self.callbacks.preview)(Some(draft));
    }

    fn valid_draft(&self) -> Option<Theme> {
        let draft = self.draft.borrow().clone()?;
        if draft.name.trim().is_empty() {
            self.set_status("Give the theme a name first.");
            return None;
        }
        Some(draft)
    }

    /// Development (`--test-actions editor:<cmd>`): `name=<text>`, `<role index>=RRGGBB`,
    /// `light`, `save`; every command reports the draft.
    pub fn test(&self, command: &str) -> String {
        match command.split_once('=') {
            Some(("name", text)) => self.name.set_text(text),
            Some((index, hex)) => {
                if let (Ok(i), Ok(v)) = (index.parse::<usize>(), u32::from_str_radix(hex, 16))
                    && let Some(b) = self.colours.get(i)
                {
                    b.set_rgba(&rgba((f64::from(v >> 16 & 255) / 255.0, f64::from(v >> 8 & 255) / 255.0, f64::from(v & 255) / 255.0)));
                }
            }
            None if command == "light" => self.mode.set_selected(1),
            None if command == "save" => {
                if let Some(theme) = self.valid_draft() {
                    (self.callbacks.save)(theme);
                }
            }
            None => {}
        }
        let draft = self.draft.borrow();
        format!("draft {:?} status {:?}", draft.as_ref().map(|t| (&t.name, t.dark, t.accent)), self.status.text())
    }
}
