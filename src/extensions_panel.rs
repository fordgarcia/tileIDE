// SPDX-License-Identifier: GPL-3.0-or-later
//! The Extensions page of Settings (v0.3 phase 8; in the drawer until v0.4): installed extensions with their
//! switch, per-workspace mode, commands and weight meter (startup, memory, CPU,
//! suspend). Installs from a folder or a git URL; there is no registry yet (v0.5).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk::{gio, glib, prelude::*};
use gtk4 as gtk;
use tileide_core::theme::{self, Theme};
use tileide_extensions::Mode;

use crate::dialog::{self, Role};
use crate::extensions::{self, Row, Source, Status};
use crate::themes_panel::flow_row;

/// How often the weight meter refreshes while the section is on screen.
const METER_EVERY: Duration = Duration::from_secs(2);

pub fn css(t: &Theme) -> String {
    let c = theme::css;
    format!(
        ".extensions {{ padding: 4px 10px 14px 12px; }}
.extensions .ext {{ border-radius: 10px; padding: 8px 8px 10px 8px; margin-top: 6px; background-color: {card}; }}
.extensions .name {{ font-weight: 600; color: {text}; }}
.extensions .meter {{ color: {muted}; font-size: 12px; font-feature-settings: \"tnum\"; }}
.extensions .meter.busy {{ color: {accent}; }}
.extensions entry, .extensions searchbar entry {{ min-height: 26px; border-radius: 13px; }}
.extensions dropdown button {{ min-height: 24px; padding: 0 8px; border-radius: 12px; }}
.extensions switch {{ margin: 0; }}
.extensions button.icon {{ min-width: 26px; min-height: 26px; padding: 0; border: none; box-shadow: none; background: transparent; color: {muted}; border-radius: 13px; }}
.extensions button.icon:hover {{ background-color: {card}; color: {text}; }}
.extensions switch:checked {{ background-color: {accent}; }}",
        text = c(t.text, 1.0),
        muted = c(t.muted, 1.0),
        accent = c(t.accent, 1.0),
        card = c(t.text, 0.04),
    )
}

pub struct Callbacks {
    /// Runs an extension command against the focused editor.
    pub run: Rc<dyn Fn(String)>,
    /// A choice changed: write `state.lua`.
    pub save: Rc<dyn Fn()>,
}

pub struct ExtensionsPanel {
    root: gtk::ScrolledWindow,
    search: gtk::SearchEntry,
    list: gtk::Box,
    empty: gtk::Label,
    status: gtk::Label,
    git: gtk::Entry,
    window: gtk::Window,
    callbacks: Callbacks,
    /// Meter label per extension id, updated in place (a rebuild would close an open
    /// mode menu every two seconds).
    meters: RefCell<Vec<(String, gtk::Label)>>,
    timer: RefCell<Option<glib::SourceId>>,
}

fn label(text: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class(class);
    l.set_xalign(0.0);
    l.set_wrap(true);
    l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    l
}

fn button(text: &str) -> gtk::Button {
    let b = gtk::Button::with_label(text);
    b.add_css_class("pill");
    b
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.add_css_class("icon");
    b.set_tooltip_text(Some(tooltip));
    b.set_valign(gtk::Align::Start);
    b
}

fn mb(kb: u64) -> String {
    format!("{:.1}\u{a0}MB", kb as f64 / 1024.0)
}

/// The weight meter line.
pub fn meter_text(row: &Row) -> String {
    let startup = row.startup.map(|d| format!("started in {}\u{a0}ms", d.as_millis()));
    let memory = row.rss_kb.map(mb);
    let cpu = row.cpu.map(|c| format!("{c:.1}%\u{a0}CPU"));
    let parts = |head: &str, rest: Vec<Option<String>>| std::iter::once(head.to_string()).chain(rest.into_iter().flatten()).collect::<Vec<_>>().join(" · ");
    match &row.status {
        Status::NotStarted if !row.pick.enabled => "Disabled".into(),
        Status::NotStarted => parts("Not running", vec![startup.map(|s| format!("last {s}"))]),
        Status::Starting => "Starting…".into(),
        Status::Running => parts("Running", vec![startup, memory, cpu]),
        Status::Suspended => parts("Suspended", vec![memory]),
        Status::Exited(reason) => format!("Stopped: {reason}"),
    }
}

impl ExtensionsPanel {
    pub fn new(window: &gtk::Window, callbacks: Callbacks) -> Rc<ExtensionsPanel> {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.add_css_class("extensions");
        content.add_css_class("themes");

        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search installed"));
        search.set_margin_top(6);
        content.append(&search);

        let section = |text: &str| {
            let l = gtk::Label::new(Some(text));
            l.add_css_class("section");
            l.set_xalign(0.0);
            l
        };
        content.append(&section("INSTALLED"));
        let empty = label("", "note");
        content.append(&empty);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&list);

        content.append(&section("INSTALL"));
        content.append(&label("No registry yet: install from a folder or a git URL. Extensions run as their own processes and start only when needed.", "note"));
        let from_folder = button("From folder…");
        let open = button("Open folder");
        content.append(&flow_row(&[from_folder.upcast_ref(), open.upcast_ref()]));
        let git = gtk::Entry::new();
        git.set_placeholder_text(Some("https://… .git"));
        git.set_hexpand(true);
        git.set_margin_top(6);
        let clone = button("Clone");
        clone.set_margin_top(6);
        let git_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        git_row.append(&git);
        git_row.append(&clone);
        content.append(&git_row);
        let status = label("", "note");
        status.set_visible(false);
        content.append(&status);

        let root = gtk::ScrolledWindow::new();
        root.set_hscrollbar_policy(gtk::PolicyType::Never);
        root.set_vexpand(true);
        root.set_child(Some(&content));

        let panel = Rc::new(ExtensionsPanel { root, search, list, empty, status, git, window: window.clone(), callbacks, meters: RefCell::new(Vec::new()), timer: RefCell::new(None) });
        let weak = Rc::downgrade(&panel);
        panel.search.connect_search_changed(move |_| {
            if let Some(p) = weak.upgrade() {
                p.refresh();
            }
        });
        let weak = Rc::downgrade(&panel);
        from_folder.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.pick_folder();
            }
        });
        open.connect_clicked(|_| {
            let uri = gio::File::for_path(extensions::ensure_root()).uri();
            let _ = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>);
        });
        let weak = Rc::downgrade(&panel);
        let clone_now = move || {
            if let Some(p) = weak.upgrade() {
                let url = p.git.text().to_string();
                p.install(Source::Git(url), false);
            }
        };
        let c = clone_now.clone();
        clone.connect_clicked(move |_| c());
        panel.git.connect_activate(move |_| clone_now());

        // The meter runs only while the section is on screen.
        let weak = Rc::downgrade(&panel);
        panel.root.connect_map(move |_| {
            let Some(p) = weak.upgrade() else { return };
            p.refresh();
            let weak = Rc::downgrade(&p);
            let id = glib::timeout_add_local(METER_EVERY, move || match weak.upgrade() {
                Some(p) => {
                    p.update_meters();
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            });
            if let Some(old) = p.timer.replace(Some(id)) {
                old.remove();
            }
        });
        let weak = Rc::downgrade(&panel);
        panel.root.connect_unmap(move |_| {
            if let Some(id) = weak.upgrade().and_then(|p| p.timer.take()) {
                id.remove();
            }
        });
        panel
    }

    pub fn widget(&self) -> &gtk::ScrolledWindow {
        &self.root
    }

    pub fn focus(&self) {
        self.search.grab_focus();
    }

    fn update_meters(&self) {
        extensions::sample();
        let rows = extensions::rows();
        for (id, l) in self.meters.borrow().iter() {
            if let Some(row) = rows.iter().find(|r| &r.installed.manifest.id == id) {
                l.set_text(&meter_text(row));
            }
        }
    }

    /// Rebuilds the list (filtered by the search text).
    pub fn refresh(self: &Rc<Self>) {
        extensions::sample();
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        self.meters.borrow_mut().clear();
        let query = self.search.text().to_lowercase();
        let rows = extensions::rows();
        let shown: Vec<&Row> = rows
            .iter()
            .filter(|r| {
                let m = &r.installed.manifest;
                query.is_empty() || [&m.name, &m.id, &m.description].iter().any(|s| s.to_lowercase().contains(&query))
            })
            .collect();
        let none = match (rows.is_empty(), shown.is_empty()) {
            (true, _) => Some(format!("Nothing installed. Extensions live in {}.", extensions::root().display())),
            (false, true) => Some("No extension matches.".into()),
            _ => None,
        };
        self.empty.set_text(none.as_deref().unwrap_or(""));
        self.empty.set_visible(none.is_some());
        for row in shown {
            self.list.append(&self.row(row));
        }
    }

    fn row(self: &Rc<Self>, row: &Row) -> gtk::Box {
        let m = &row.installed.manifest;
        let id = m.id.clone();
        let card = gtk::Box::new(gtk::Orientation::Vertical, 4);
        card.add_css_class("ext");

        let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let name = gtk::Label::new(Some(&m.name));
        name.add_css_class("name");
        name.set_xalign(0.0);
        name.set_hexpand(true);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        name.set_tooltip_text(Some(&format!("{} {}", m.id, m.version)));
        head.append(&name);
        let switch = gtk::Switch::new();
        switch.set_active(row.pick.enabled);
        switch.set_valign(gtk::Align::Center);
        switch.set_tooltip_text(Some("Enabled"));
        let weak = Rc::downgrade(self);
        let key = id.clone();
        switch.connect_state_set(move |_, on| {
            extensions::set_enabled(&key, on);
            if let Some(p) = weak.upgrade() {
                (p.callbacks.save)();
            }
            glib::Propagation::Proceed
        });
        head.append(&switch);
        card.append(&head);

        if !m.description.is_empty() {
            card.append(&label(&m.description, "note"));
        }
        if m.starts_always() {
            let warn = label("Activates always: starts with TileIDE in every workspace.", "note");
            warn.add_css_class("warn");
            card.append(&warn);
        }
        let meter = label(&meter_text(row), "meter");
        meter.set_hexpand(true);
        if row.status == Status::Running && row.cpu.is_some_and(|c| c >= 5.0) {
            meter.add_css_class("busy");
        }
        self.meters.borrow_mut().push((id.clone(), meter.clone()));
        // The meter line carries the process controls as icons, so they fit at the
        // narrowest drawer.
        let meter_row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        meter_row.append(&meter);
        if matches!(row.status, Status::Running | Status::Suspended) {
            let suspended = row.status == Status::Suspended;
            let toggle = icon_button(if suspended { "media-playback-start-symbolic" } else { "media-playback-pause-symbolic" }, if suspended { "Resume" } else { "Suspend (no CPU until resumed)" });
            let key = id.clone();
            toggle.connect_clicked(move |_| extensions::set_suspended(&key, !suspended));
            meter_row.append(&toggle);
            let stop = icon_button("media-playback-stop-symbolic", "Stop (starts again when needed)");
            let key = id.clone();
            stop.connect_clicked(move |_| extensions::stop(&key));
            meter_row.append(&stop);
        }
        let remove = icon_button("user-trash-symbolic", "Remove");
        let weak = Rc::downgrade(self);
        let (key, title) = (id.clone(), m.name.clone());
        remove.connect_clicked(move |_| {
            let weak = weak.clone();
            let key = key.clone();
            dialog::ask(&format!("Remove {title}?"), "Its folder is deleted; reinstall it to get it back.", &[("Cancel", Role::Cancel), ("Remove", Role::Destructive)], 0, move |choice| {
                if choice != 1 {
                    return;
                }
                if let Err(e) = extensions::remove(&key)
                    && let Some(p) = weak.upgrade()
                {
                    p.show_status(&e);
                }
                if let Some(p) = weak.upgrade() {
                    (p.callbacks.save)();
                    p.refresh();
                }
            });
        });
        meter_row.append(&remove);
        card.append(&meter_row);

        if let Some(n) = extensions::workspace() {
            let labels: Vec<&str> = Mode::ALL.iter().map(|m| m.label()).collect();
            let mode = gtk::DropDown::from_strings(&labels);
            mode.set_halign(gtk::Align::Start);
            mode.set_tooltip_text(Some(&format!("In workspace {}", crate::workspaces::display(n))));
            let current = row.pick.mode(Some(n));
            mode.set_selected(Mode::ALL.iter().position(|m| *m == current).unwrap_or(0) as u32);
            mode.set_sensitive(row.pick.enabled);
            let weak = Rc::downgrade(self);
            let key = id.clone();
            mode.connect_selected_notify(move |d| {
                let Some(chosen) = Mode::ALL.get(d.selected() as usize) else { return };
                extensions::set_mode(&key, *chosen);
                if let Some(p) = weak.upgrade() {
                    (p.callbacks.save)();
                }
            });
            card.append(&mode);
        }
        let hidden_here = !row.pick.enabled || row.pick.mode(extensions::workspace()) == Mode::Off;
        if !hidden_here && !m.commands.is_empty() {
            let mut commands: Vec<gtk::Widget> = Vec::new();
            for c in &m.commands {
                let run = button(&c.title);
                run.set_tooltip_text(Some(&format!("Runs \"{}\" on the focused file. Bind it: tileide.bind(\"ALT\", \"…\", \"extension\", \"{}\")", c.id, c.id)));
                let weak = Rc::downgrade(self);
                let command = c.id.clone();
                run.connect_clicked(move |_| {
                    if let Some(p) = weak.upgrade() {
                        (p.callbacks.run)(command.clone());
                    }
                });
                commands.push(run.upcast());
            }
            let refs: Vec<&gtk::Widget> = commands.iter().collect();
            card.append(&flow_row(&refs));
        }
        card
    }

    fn show_status(&self, text: &str) {
        self.status.set_text(text);
        self.status.set_visible(!text.is_empty());
    }

    fn pick_folder(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        gtk::FileDialog::builder().title("Install an extension folder").build().select_folder(Some(&self.window), None::<&gio::Cancellable>, move |result| {
            let (Some(p), Some(path)) = (weak.upgrade(), result.ok().and_then(|f| f.path())) else { return };
            p.install(Source::Folder(path), false);
        });
    }

    fn install(self: &Rc<Self>, source: Source, replace: bool) {
        let what = match &source {
            Source::Folder(p) => p.display().to_string(),
            Source::Git(url) if url.trim().is_empty() => return self.show_status("Paste a git URL first."),
            Source::Git(url) => url.clone(),
        };
        self.show_status(&format!("Installing {what}…"));
        let retry = match &source {
            Source::Folder(p) => Source::Folder(p.clone()),
            Source::Git(u) => Source::Git(u.clone()),
        };
        let weak = Rc::downgrade(self);
        extensions::install(source, replace, move |result| {
            let Some(p) = weak.upgrade() else { return };
            match result {
                Ok(name) => {
                    p.show_status(&format!("Installed {name}."));
                    p.git.set_text("");
                    p.refresh();
                }
                Err(e) if e.ends_with("is already installed") && !replace => {
                    p.show_status("");
                    let weak = Rc::downgrade(&p);
                    dialog::ask("Replace the installed copy?", &format!("{e}. Replacing stops it and overwrites its folder."), &[("Cancel", Role::Cancel), ("Replace", Role::Destructive)], 0, move |choice| {
                        if let (1, Some(p)) = (choice, weak.upgrade()) {
                            if let Some(id) = e.strip_prefix('"').and_then(|e| e.strip_suffix("\" is already installed")) {
                                extensions::stop(id);
                            }
                            p.install(retry, true);
                        }
                    });
                }
                Err(e) => p.show_status(&e),
            }
        });
    }
}
