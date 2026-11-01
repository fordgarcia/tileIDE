// SPDX-License-Identifier: GPL-3.0-or-later
//! Quick open: a rounded search card over a lightly dimmed window, like a browser's
//! new-tab bar. `Alt+E` opens the pick in a new tile (an empty query offers a blank
//! file); `Alt+F` opens it in the focused tile. Type to filter the workspace's files,
//! ↑/↓ (or Ctrl+N/P) to select, Enter to open, Esc or a click on the dim to close.
//! `Alt+D` uses the same card to pick the workspace's folder: recent folders first,
//! then folders under `~`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4 as gtk;
use tileide_core::search;
use tileide_core::theme::{self, Theme};

/// Where the pick opens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    NewTile,
    Focused,
    /// Picks a folder for the active workspace.
    Folder,
}

/// Files listed per search; enough for any project, and `~` can't stall it.
const INDEX_LIMIT: usize = 50_000;
const SHOWN: usize = 50;
/// How deep the folder search looks under `~`.
const FOLDER_DEPTH: usize = 6;

pub fn css(t: &Theme) -> String {
    let c = theme::css;
    format!(
        ".quick-open {{ background-color: rgba(0, 0, 0, 0.35); }}
.quick-open .card {{ background-color: {surface}; color: {text}; border-radius: 22px; border: 1px solid {edge}; padding: 10px; box-shadow: 0 18px 48px rgba(0, 0, 0, 0.35); }}
.quick-open entry {{ background: transparent; border: none; box-shadow: none; outline: none; color: {text}; font-size: 17px; min-height: 40px; padding: 0 12px; caret-color: {accent}; }}
.quick-open entry image {{ color: {muted}; }}
.quick-open list, .quick-open scrolledwindow, .quick-open viewport {{ background: transparent; }}
.quick-open .divider {{ background-color: {edge}; min-height: 1px; margin: 6px 4px; }}
.quick-open row {{ border-radius: 14px; padding: 7px 12px; color: {text}; background: transparent; outline: none; }}
.quick-open row:selected, .quick-open row:hover {{ background-color: {select}; }}
.quick-open .file {{ font-size: 14px; }}
.quick-open .dir, .quick-open .hint {{ font-size: 12px; color: {muted}; }}
.quick-open .hint {{ padding: 6px 12px 2px 12px; }}",
        surface = c(t.surface, 0.97),
        text = c(t.text, 1.0),
        muted = c(t.muted, 1.0),
        edge = c(t.text, 0.10),
        accent = c(t.accent, 1.0),
        select = c(t.accent, 0.16),
    )
}

struct State {
    target: Target,
    root: PathBuf,
    /// Files (or folders, for `Target::Folder`) under `root`, relative to it.
    files: Vec<PathBuf>,
    /// Folder search only: recent folders (absolute), listed first.
    recent: Vec<PathBuf>,
    /// What each list row opens; `None` = a blank file.
    rows: Vec<Option<PathBuf>>,
    /// Bumped on every open, so a slow index from an earlier open is dropped.
    generation: u64,
}

pub struct QuickOpen {
    root: gtk::Box,
    entry: gtk::SearchEntry,
    list: gtk::ListBox,
    scroll: gtk::ScrolledWindow,
    hint: gtk::Label,
    state: RefCell<State>,
}

impl QuickOpen {
    /// `on_pick(target, path)`: `None` is a blank file. `on_close` runs on every close.
    pub fn new(overlay: &gtk::Overlay, on_pick: Rc<dyn Fn(Target, Option<PathBuf>)>, on_close: Rc<dyn Fn()>) -> Rc<QuickOpen> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("quick-open");
        root.set_visible(false);

        let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        card.add_css_class("card");
        card.set_halign(gtk::Align::Center);
        card.set_valign(gtk::Align::Start);
        card.set_margin_top(140);
        card.set_size_request(640, -1);
        let entry = gtk::SearchEntry::new();
        let divider = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        divider.add_css_class("divider");
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Browse);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&list));
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroll.set_propagate_natural_height(true);
        scroll.set_max_content_height(380);
        let hint = gtk::Label::new(None);
        hint.add_css_class("hint");
        hint.set_xalign(0.0);
        card.append(&entry);
        card.append(&divider);
        card.append(&scroll);
        card.append(&hint);
        root.append(&card);
        overlay.add_overlay(&root);

        let view = Rc::new(QuickOpen {
            root,
            entry,
            list,
            scroll,
            hint,
            state: RefCell::new(State { target: Target::NewTile, root: PathBuf::new(), files: Vec::new(), recent: Vec::new(), rows: Vec::new(), generation: 0 }),
        });

        let v = Rc::downgrade(&view);
        view.entry.connect_search_changed(move |_| {
            if let Some(v) = v.upgrade() {
                v.refilter();
            }
        });

        let (v, pick) = (Rc::downgrade(&view), on_pick.clone());
        view.list.connect_row_activated(move |_, row| {
            if let Some(v) = v.upgrade() {
                v.pick(row.index(), &pick);
            }
        });

        // Capture phase: the entry would otherwise take Up/Down/Enter itself.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let (v, pick, close) = (Rc::downgrade(&view), on_pick, on_close.clone());
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(v) = v.upgrade() else { return glib::Propagation::Proceed };
            let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
            match key {
                gdk::Key::Escape => {
                    v.close();
                    close();
                }
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    let index = v.list.selected_row().map_or(0, |r| r.index());
                    v.pick(index, &pick);
                }
                gdk::Key::Down => v.step(1),
                gdk::Key::Up => v.step(-1),
                gdk::Key::n if ctrl => v.step(1),
                gdk::Key::p if ctrl => v.step(-1),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        view.root.add_controller(keys);

        // Only clicks outside the card reach this (rows and the entry claim theirs).
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

    /// Opens over the window and lists `folder`'s files (indexed off the main thread).
    pub fn open(self: &Rc<Self>, target: Target, folder: &Path) {
        self.show(target, folder, Vec::new());
    }

    /// The folder search: `recent` first, then folders under `home`.
    pub fn open_folders(self: &Rc<Self>, home: &Path, recent: Vec<PathBuf>) {
        self.show(Target::Folder, home, recent);
    }

    fn show(self: &Rc<Self>, target: Target, folder: &Path, recent: Vec<PathBuf>) {
        let generation = {
            let mut st = self.state.borrow_mut();
            // Same folder and kind: show the last index at once while the new one runs.
            if st.root != folder || (st.target == Target::Folder) != (target == Target::Folder) {
                st.root = folder.to_path_buf();
                st.files.clear();
            }
            st.target = target;
            st.recent = recent;
            st.generation += 1;
            st.generation
        };
        let placeholder = match target {
            Target::NewTile => "Open a file in a new tile…",
            Target::Focused => "Open a file in this tile…",
            Target::Folder => "Open a folder in this workspace…",
        };
        self.entry.set_placeholder_text(Some(placeholder));
        self.entry.set_text("");
        self.refilter();
        crate::motion::fade_in(&self.root);
        self.entry.grab_focus();

        let (me, root) = (Rc::downgrade(self), folder.to_path_buf());
        let job = gio::spawn_blocking(move || match target {
            Target::Folder => search::list_dirs(&root, FOLDER_DEPTH, INDEX_LIMIT),
            _ => search::list_files(&root, INDEX_LIMIT),
        });
        glib::spawn_future_local(async move {
            let Ok(files) = job.await else { return };
            let Some(me) = me.upgrade() else { return };
            if me.state.borrow().generation != generation {
                return;
            }
            me.state.borrow_mut().files = files;
            me.refilter();
        });
    }

    pub fn close(&self) {
        crate::motion::fade_out(&self.root);
    }

    fn refilter(&self) {
        let query = self.entry.text();
        let mut st = self.state.borrow_mut();
        let mut rows: Vec<Option<PathBuf>> = Vec::new();
        if st.target == Target::NewTile && query.trim().is_empty() {
            rows.push(None);
        }
        let folders = st.target == Target::Folder;
        if folders {
            // A typed path that exists comes first (`~/notes`, `/etc`).
            let typed = crate::workspaces::expand_home(query.trim());
            if typed.is_absolute() && typed.is_dir() {
                rows.push(Some(typed));
            }
            // Recent ones next; a folder listed both ways appears once.
            rows.extend(search::rank(&query, &st.recent, SHOWN).into_iter().map(|p| Some(p.to_path_buf())));
        }
        for p in search::rank(&query, &st.files, SHOWN) {
            let full = st.root.join(p);
            if rows.len() >= SHOWN {
                break;
            }
            if !(folders && st.recent.contains(&full)) {
                rows.push(Some(p.to_path_buf()));
            }
        }
        if folders {
            let mut seen = std::collections::HashSet::new();
            rows.retain(|r| r.as_ref().is_none_or(|p| seen.insert(st.root.join(p))));
        }
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        for row in &rows {
            let shown = match row {
                Some(p) if folders => Some(tilde(&st.root.join(p))),
                other => other.clone(),
            };
            self.list.append(&row_widget(shown.as_deref()));
        }
        self.list.select_row(self.list.row_at_index(0).as_ref());
        let found = st.files.len();
        let (kind, verb) = if folders { ("folders", "open here") } else { ("files", "open") };
        self.hint.set_text(&match (found, rows.iter().flatten().count()) {
            (0, 0) => "Indexing… · Esc closes".to_string(),
            (_, 0) => format!("No matching {kind} · Esc closes"),
            (0, _) => format!("↑↓ select · Enter {verb} · Esc close · indexing…"),
            _ => format!("↑↓ select · Enter {verb} · Esc close · {found} {kind} in {}", folder_name(&st.root)),
        });
        st.rows = rows;
    }

    fn step(&self, by: i32) {
        let count = self.state.borrow().rows.len() as i32;
        if count == 0 {
            return;
        }
        let current = self.list.selected_row().map_or(0, |r| r.index());
        let next = (current + by).rem_euclid(count);
        if let Some(row) = self.list.row_at_index(next) {
            self.list.select_row(Some(&row));
            // Typing stays in the entry; only the list scrolls to the selection.
            if let Some(bounds) = row.compute_bounds(&self.list) {
                let (top, bottom) = (bounds.y() as f64, (bounds.y() + bounds.height()) as f64);
                self.scroll.vadjustment().clamp_page(top, bottom);
            }
        }
    }

    fn pick(&self, index: i32, on_pick: &Rc<dyn Fn(Target, Option<PathBuf>)>) {
        let (target, choice) = {
            let st = self.state.borrow();
            let Some(choice) = st.rows.get(index.max(0) as usize).cloned() else { return };
            (st.target, choice.map(|relative| st.root.join(relative)))
        };
        self.close();
        on_pick(target, choice);
    }
}

/// File name, then its folder in the muted tone.
fn row_widget(path: Option<&Path>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let (name, dir) = match path {
        None => ("New empty file".to_string(), String::new()),
        Some(p) => (
            p.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            p.parent().map_or_else(String::new, |d| d.to_string_lossy().into_owned()),
        ),
    };
    let name = gtk::Label::new(Some(&name));
    name.add_css_class("file");
    let dir = gtk::Label::new(Some(&dir));
    dir.add_css_class("dir");
    dir.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    dir.set_hexpand(true);
    dir.set_xalign(1.0);
    row.append(&name);
    row.append(&dir);
    row
}

fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// `$HOME/x` → `~/x`, for display.
fn tilde(path: &Path) -> PathBuf {
    match std::env::var_os("HOME").and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf)) {
        Some(rest) => Path::new("~").join(rest),
        None => path.to_path_buf(),
    }
}
