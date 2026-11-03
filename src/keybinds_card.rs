// SPDX-License-Identifier: GPL-3.0-or-later
//! `Alt+K`: every active keybind on the quick-open card, filtered by typing (keys or
//! what they do). Esc or a click outside closes.

use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4 as gtk;
use tileide_config::{Action, Bind};
use tileide_core::layout::LayoutKind;
use tileide_core::tiles::{Axis, Direction};

/// What a bound action does, in a few words.
pub fn describe(action: &Action) -> String {
    let dir = |d: &Direction| match d {
        Direction::Left => "left",
        Direction::Right => "right",
        Direction::Up => "up",
        Direction::Down => "down",
    };
    match action {
        Action::Workspace(n) => format!("workspace {}", crate::workspaces::display(*n)),
        Action::ConfigWorkspace => "config workspace".into(),
        Action::Sidebar => "files drawer".into(),
        Action::Layout(None) => "dwindle ⇄ columns (stack in the Wikipedia rice)".into(),
        Action::Layout(Some(LayoutKind::Dwindle)) => "dwindle layout".into(),
        Action::Layout(Some(LayoutKind::Columns)) => "columns layout".into(),
        Action::Layout(Some(LayoutKind::Stack)) => "stack layout".into(),
        Action::Split(Axis::Column) => "next tile opens below".into(),
        Action::Split(Axis::Row) => "next tile opens beside".into(),
        Action::Focus(d) => format!("focus {}", dir(d)),
        Action::Move(d) => format!("move tile {}", dir(d)),
        Action::Resize(d) => format!("resize {}", dir(d)),
        Action::Close => "close tile".into(),
        Action::Terminal => "new terminal".into(),
        Action::Editor => "open a file in a new tile".into(),
        Action::Search => "open a file in this tile".into(),
        Action::Wrap => "word wrap".into(),
        Action::Save => "save".into(),
        Action::Quit => "quit".into(),
        Action::Overview => "workspace overview".into(),
        Action::Themes => "theme tray".into(),
        Action::Folder => "pick the workspace's folder".into(),
        Action::ThemesSection => "themes & background".into(),
        Action::WorkspaceClose => "remove this workspace".into(),
        Action::Hover => "info at the cursor".into(),
        Action::Definition => "go to definition".into(),
        Action::Complete => "complete".into(),
        Action::Format => "format the file".into(),
        Action::Rename => "rename symbol".into(),
        Action::References => "find references".into(),
        Action::CodeAction => "quick fixes".into(),
        Action::ExtensionsSection => "extensions".into(),
        Action::Extension(id) => format!("extension: {id}"),
        Action::Settings => "settings".into(),
        Action::Keybinds => "this list".into(),
        Action::FontSize(n) if *n > 0 => "code font bigger".into(),
        Action::FontSize(_) => "code font smaller".into(),
        Action::Run => "run the focused file".into(),
        Action::Debug => "debug: start or continue".into(),
        Action::DebugStop => "debug: stop".into(),
        Action::Breakpoint => "breakpoint on this line".into(),
        Action::StepOver => "debug: step over".into(),
        Action::StepInto => "debug: step into".into(),
        Action::StepOut => "debug: step out".into(),
        Action::Toolchains => "toolchains (settings)".into(),
        Action::Interpreter => "Python / kernel".into(),
        Action::Test => "run tests (at the cursor, file, all)".into(),
        Action::NextFailure => "next failed test".into(),
        Action::Unavailable(name) => format!("{name} (not yet available)"),
    }
}

/// `Alt+Shift+←` style.
pub fn keys_text(bind: &Bind) -> String {
    let key = match bind.key.as_str() {
        "Left" => "←".to_string(),
        "Right" => "→".to_string(),
        "Up" => "↑".to_string(),
        "Down" => "↓".to_string(),
        "comma" => ",".to_string(),
        "equal" => "=".to_string(),
        "minus" => "-".to_string(),
        "space" => "Space".to_string(),
        k if k.chars().count() == 1 => k.to_uppercase(),
        k => k.to_string(),
    };
    let mods: Vec<String> = ["CTRL", "ALT", "SHIFT", "SUPER"]
        .iter()
        .filter(|m| bind.mods.iter().any(|b| b == *m))
        .map(|m| {
            let mut c = m.chars();
            c.next().map_or_else(String::new, |f| f.to_string() + &c.as_str().to_lowercase())
        })
        .collect();
    mods.into_iter().chain(std::iter::once(key)).collect::<Vec<_>>().join("+")
}

pub struct KeybindsCard {
    root: gtk::Box,
    entry: gtk::SearchEntry,
    list: gtk::ListBox,
    rows: std::cell::RefCell<Vec<(String, gtk::ListBoxRow)>>,
    empty: gtk::Label,
}

impl KeybindsCard {
    pub fn new(overlay: &gtk::Overlay, on_close: Rc<dyn Fn()>) -> Rc<KeybindsCard> {
        // The quick-open look (`quick_open::css`).
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("quick-open");
        root.add_css_class("keybinds");
        root.set_visible(false);
        let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        card.add_css_class("card");
        card.set_halign(gtk::Align::Center);
        card.set_valign(gtk::Align::Start);
        card.set_margin_top(100);
        card.set_size_request(560, -1);
        let entry = gtk::SearchEntry::new();
        entry.set_placeholder_text(Some("Search keybinds"));
        let divider = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        divider.add_css_class("divider");
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&list));
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroll.set_propagate_natural_height(true);
        scroll.set_max_content_height(460);
        let empty = gtk::Label::new(Some("No keybind matches."));
        empty.add_css_class("hint");
        empty.set_xalign(0.0);
        empty.set_visible(false);
        let hint = gtk::Label::new(Some("Rebind in tileide.lua: tileide.bind(mods, key, action, arg) · Esc closes"));
        hint.add_css_class("hint");
        hint.set_xalign(0.0);
        hint.set_wrap(true);
        card.append(&entry);
        card.append(&divider);
        card.append(&scroll);
        card.append(&empty);
        card.append(&hint);
        root.append(&card);
        overlay.add_overlay(&root);

        let view = Rc::new(KeybindsCard { root, entry, list, rows: std::cell::RefCell::new(Vec::new()), empty });
        let v = Rc::downgrade(&view);
        view.entry.connect_search_changed(move |_| {
            if let Some(v) = v.upgrade() {
                v.filter();
            }
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let (v, close) = (Rc::downgrade(&view), on_close.clone());
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(v) = v.upgrade() else { return glib::Propagation::Proceed };
            if key == gdk::Key::Escape {
                v.close();
                close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        view.root.add_controller(keys);
        let click = gtk::GestureClick::new();
        let (v, close) = (Rc::downgrade(&view), on_close);
        click.connect_released(move |gesture, _, x, y| {
            let Some(v) = v.upgrade() else { return };
            if v.root.pick(x, y, gtk::PickFlags::DEFAULT).is_some_and(|w| w == v.root) {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                v.close();
                close();
            }
        });
        view.root.add_controller(click);
        view
    }

    pub fn is_open(&self) -> bool {
        crate::motion::shown(&self.root)
    }

    pub fn open(&self, binds: &[Bind]) {
        self.list.remove_all();
        let mut rows = Vec::new();
        for bind in binds {
            let (keys, what) = (keys_text(bind), describe(&bind.action));
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let k = gtk::Label::new(Some(&keys));
            k.add_css_class("file");
            k.set_width_chars(16);
            k.set_xalign(0.0);
            let d = gtk::Label::new(Some(&what));
            d.add_css_class("dir");
            d.set_xalign(0.0);
            d.set_hexpand(true);
            line.append(&k);
            line.append(&d);
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&line));
            row.set_activatable(false);
            self.list.append(&row);
            rows.push((format!("{keys} {what}").to_lowercase(), row));
        }
        *self.rows.borrow_mut() = rows;
        self.entry.set_text("");
        self.filter();
        crate::motion::fade_in(&self.root);
        self.entry.grab_focus();
    }

    pub fn close(&self) {
        crate::motion::fade_out(&self.root);
    }

    fn filter(&self) {
        let query = self.entry.text().to_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();
        let mut any = false;
        for (text, row) in self.rows.borrow().iter() {
            let shown = words.iter().all(|w| text.contains(w));
            row.set_visible(shown);
            any |= shown;
        }
        self.empty.set_visible(!any);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_read_like_menus() {
        let bind = |mods: &[&str], key: &str| Bind { mods: mods.iter().map(|m| m.to_string()).collect(), key: key.into(), action: Action::Close };
        assert_eq!(keys_text(&bind(&["ALT", "SHIFT"], "Left")), "Alt+Shift+←");
        assert_eq!(keys_text(&bind(&["ALT"], "k")), "Alt+K");
        assert_eq!(keys_text(&bind(&["ALT"], "equal")), "Alt+=");
        assert_eq!(keys_text(&bind(&[], "F12")), "F12");
    }
}
