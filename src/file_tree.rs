// SPDX-License-Identifier: GPL-3.0-or-later
//! The drawer's Files section: a folder tree with a VS Code-style toolbar
//! (new file, new folder, refresh, collapse all). Clicking a file opens it in a tile.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4 as gtk;

pub const CSS: &str = "
.drawer { font-family: \"Space Grotesk\"; font-size: 13px; color: #E6E8EC; }
.drawer .header { padding: 10px 8px 6px 14px; }
.drawer .title { font-weight: 500; }
.drawer button { min-width: 26px; min-height: 26px; padding: 0; border: none; box-shadow: none; background: transparent; color: #9AA1AD; }
.drawer button:hover { background-color: rgba(255, 255, 255, 0.08); color: #E6E8EC; }
.drawer list, .drawer row, .drawer scrolledwindow { background: transparent; }
.drawer row { padding: 3px 8px; border-radius: 6px; }
.drawer row:hover { background-color: rgba(255, 255, 255, 0.05); }
.drawer row:selected { background-color: rgba(79, 209, 217, 0.18); color: #E6E8EC; }
.drawer entry { min-height: 24px; font-family: \"Space Grotesk\"; }
.drawer entry.error { border-color: #E06C75; }
.peek { background-color: rgba(14, 16, 19, 0.97); border-radius: 12px; border: 1px solid rgba(79, 209, 217, 0.35); }
.peek .go { padding: 8px 14px; font-weight: 500; color: #4FD1D9; border-radius: 12px 12px 0 0; }
";

#[derive(Clone, Copy)]
enum NewKind {
    File,
    Folder,
}

pub struct FileTree {
    root: PathBuf,
    expanded: HashSet<PathBuf>,
    selected: Option<PathBuf>,
    /// One path per list row, in order (the inline name entry has no path).
    rows: Vec<PathBuf>,
    list: gtk::ListBox,
    container: gtk::Box,
    title: gtk::Label,
    on_open: Rc<dyn Fn(&Path)>,
    me: Weak<RefCell<FileTree>>,
}

impl FileTree {
    pub fn new(root: PathBuf, on_open: Rc<dyn Fn(&Path)>) -> Rc<RefCell<FileTree>> {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.add_css_class("drawer");

        let list = gtk::ListBox::new();
        list.set_activate_on_single_click(true);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_vexpand(true);
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroll.set_child(Some(&list));

        let title = gtk::Label::new(Some(&folder_name(&root)));
        let tree = Rc::new(RefCell::new(FileTree {
            root,
            expanded: HashSet::new(),
            selected: None,
            rows: Vec::new(),
            list: list.clone(),
            container: container.clone(),
            title,
            on_open,
            me: Weak::new(),
        }));
        tree.borrow_mut().me = Rc::downgrade(&tree);

        container.append(&header(&tree));
        container.append(&scroll);

        let weak = Rc::downgrade(&tree);
        list.connect_row_activated(move |_, row| {
            if let Some(tree) = weak.upgrade() {
                FileTree::activate(&tree, row.index());
            }
        });
        tree.borrow_mut().rebuild();
        tree
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }

    /// Shows another folder (workspace switch); folds everything and clears the selection.
    pub fn set_root(&mut self, root: &Path) {
        if root == self.root {
            return;
        }
        self.root = root.to_path_buf();
        self.expanded.clear();
        self.selected = None;
        self.title.set_text(&folder_name(root));
        self.rebuild();
    }

    fn activate(tree: &Rc<RefCell<FileTree>>, index: i32) {
        let Some(path) = usize::try_from(index).ok().and_then(|i| tree.borrow().rows.get(i).cloned()) else { return };
        // The inline name entry's row has an empty path.
        if path.as_os_str().is_empty() {
            return;
        }
        tree.borrow_mut().selected = Some(path.clone());
        if path.is_dir() {
            {
                let mut t = tree.borrow_mut();
                if !t.expanded.remove(&path) {
                    t.expanded.insert(path);
                }
            }
            // Rebuilding the list inside its own signal handler is not safe; defer it.
            let weak = Rc::downgrade(tree);
            glib::idle_add_local_once(move || {
                if let Some(tree) = weak.upgrade() {
                    tree.borrow_mut().rebuild();
                }
            });
        } else {
            let on_open = tree.borrow().on_open.clone();
            on_open(&path);
        }
    }

    fn rebuild(&mut self) {
        self.list.remove_all();
        self.rows.clear();
        let root = self.root.clone();
        self.add_children(&root, 0);
        if let Some(i) = self.selected.as_ref().and_then(|s| self.rows.iter().position(|p| p == s)) {
            self.list.select_row(self.list.row_at_index(i as i32).as_ref());
        }
    }

    fn add_children(&mut self, dir: &Path, depth: usize) {
        for path in sorted_entries(dir) {
            let is_dir = path.is_dir();
            let open = is_dir && self.expanded.contains(&path);
            self.list.append(&row(&path, depth, is_dir, open));
            self.rows.push(path.clone());
            if open {
                self.add_children(&path, depth + 1);
            }
        }
    }

    /// Where "new file/folder" goes: the selected folder, the selected file's folder, or the root.
    fn target_dir(&self) -> PathBuf {
        match &self.selected {
            Some(p) if p.is_dir() => p.clone(),
            Some(p) => p.parent().map(Path::to_path_buf).unwrap_or_else(|| self.root.clone()),
            None => self.root.clone(),
        }
    }

    /// Shows an inline name entry under the target folder, like VS Code.
    fn start_new(&mut self, kind: NewKind) {
        let dir = self.target_dir();
        if dir != self.root {
            self.expanded.insert(dir.clone());
        }
        self.rebuild();
        let (index, depth) = match self.rows.iter().position(|p| *p == dir) {
            Some(i) => (i as i32 + 1, dir.strip_prefix(&self.root).map_or(0, |r| r.components().count())),
            None => (0, 0),
        };

        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some(match kind {
            NewKind::File => "File name",
            NewKind::Folder => "Folder name",
        }));
        entry.set_margin_start(8 + depth as i32 * 14);
        // A small minimum width, so the drawer can be narrow without GTK size warnings.
        entry.set_width_chars(1);
        self.list.insert(&entry, index);
        // Keep `rows` aligned with list indices.
        self.rows.insert(index as usize, PathBuf::new());
        entry.grab_focus();

        let weak = self.me.clone();
        entry.connect_activate(move |entry| {
            let name = entry.text().trim().to_string();
            if name.is_empty() || name.contains('/') || name == "." || name == ".." {
                entry.add_css_class("error");
                return;
            }
            let path = dir.join(&name);
            let created = match kind {
                NewKind::File => std::fs::File::create_new(&path).map(|_| ()),
                NewKind::Folder => std::fs::create_dir(&path),
            };
            if let Err(e) = created {
                entry.add_css_class("error");
                entry.set_tooltip_text(Some(&e.to_string()));
                return;
            }
            let Some(tree) = weak.upgrade() else { return };
            let weak = Rc::downgrade(&tree);
            glib::idle_add_local_once(move || {
                let Some(tree) = weak.upgrade() else { return };
                tree.borrow_mut().selected = Some(path.clone());
                tree.borrow_mut().rebuild();
                if matches!(kind, NewKind::File) {
                    let on_open = tree.borrow().on_open.clone();
                    on_open(&path);
                }
            });
        });

        let keys = gtk::EventControllerKey::new();
        let weak = self.me.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key != gdk::Key::Escape {
                return glib::Propagation::Proceed;
            }
            let weak = weak.clone();
            glib::idle_add_local_once(move || {
                if let Some(tree) = weak.upgrade() {
                    tree.borrow_mut().rebuild();
                }
            });
            glib::Propagation::Stop
        });
        entry.add_controller(keys);
    }
}

fn header(tree: &Rc<RefCell<FileTree>>) -> gtk::Box {
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    header.add_css_class("header");
    let title = tree.borrow().title.clone();
    title.add_css_class("title");
    title.set_hexpand(true);
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    header.append(&title);

    type Action = fn(&mut FileTree);
    let buttons: [(&str, &str, Action); 4] = [
        ("document-new-symbolic", "New File", |t| t.start_new(NewKind::File)),
        ("folder-new-symbolic", "New Folder", |t| t.start_new(NewKind::Folder)),
        ("view-refresh-symbolic", "Refresh", |t| t.rebuild()),
        ("view-restore-symbolic", "Collapse Folders", |t| {
            t.expanded.clear();
            t.rebuild();
        }),
    ];
    for (icon, tooltip, action) in buttons {
        let button = gtk::Button::from_icon_name(icon);
        button.set_tooltip_text(Some(tooltip));
        let weak = Rc::downgrade(tree);
        button.connect_clicked(move |_| {
            if let Some(tree) = weak.upgrade() {
                action(&mut tree.borrow_mut());
            }
        });
        header.append(&button);
    }
    header
}

fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(|| "/".into(), |n| n.to_string_lossy().into_owned())
}

fn row(path: &Path, depth: usize, is_dir: bool, open: bool) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.set_margin_start(depth as i32 * 14);
    let chevron = gtk::Image::from_icon_name(match (is_dir, open) {
        (true, true) => "pan-down-symbolic",
        (true, false) => "pan-end-symbolic",
        _ => "",
    });
    chevron.set_pixel_size(12);
    row.append(&chevron);
    let icon = gtk::Image::from_icon_name(if is_dir { "folder-symbolic" } else { "text-x-generic-symbolic" });
    icon.set_pixel_size(14);
    row.append(&icon);
    let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let label = gtk::Label::new(Some(&name));
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&label);
    row
}

/// Folders first, then files, each alphabetical (case-insensitive); `.git` hidden.
fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut entries: Vec<PathBuf> = read.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n != ".git")).collect();
    entries.sort_by_key(|p| (!p.is_dir(), p.file_name().map(|n| n.to_string_lossy().to_lowercase())));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_first_then_alphabetical_and_git_hidden() {
        let dir = std::env::temp_dir().join(format!("tileide-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for d in ["b_dir", "A_dir", ".git"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        for f in ["z.rs", "B.md", "a.lua"] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        let names: Vec<String> = sorted_entries(&dir).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["A_dir", "b_dir", "a.lua", "B.md", "z.rs"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
