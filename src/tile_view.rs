// SPDX-License-Identifier: GPL-3.0-or-later
//! One workspace's tiles: GtkSourceView editors and VTE terminals, placed by
//! `tiles::TileTree` inside a `gtk::Fixed` that sits over the canvas drawing.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use gtk::{gdk, gio, glib};
use gtk4 as gtk;
use sourceview5::prelude::*;
use vte4::prelude::*;

use crate::draw::token;
use crate::tiles::{Axis, Direction, Rect, TileId, TileTree};

pub enum Kind<'a> {
    /// `None` opens an untitled buffer; saving asks for a path.
    Editor(Option<&'a Path>),
    Terminal,
}

struct Tile {
    frame: gtk::Box,
    /// What receives keyboard focus (the text view or the terminal).
    input: gtk::Widget,
    /// Editors only.
    buffer: Option<sourceview5::Buffer>,
    path: Option<PathBuf>,
}

pub struct TileView {
    tree: TileTree,
    tiles: HashMap<TileId, Tile>,
    fixed: gtk::Fixed,
    area: Rect,
    me: Weak<RefCell<TileView>>,
    /// Word wrap for this view's editors (Alt+Z).
    wrap: bool,
    /// Called when tiles appear or disappear, so the canvas can redraw its empty hint.
    on_change: Rc<dyn Fn()>,
}

/// `tile_opacity`: the tile background's opacity (the full-screen background's control).
pub fn css(tile_opacity: f64) -> String {
    format!(
        ".tile {{ background-color: rgba(27, 31, 38, {tile_opacity}); border-radius: 10px; border: 2px solid rgba(79, 209, 217, 0.12); }}
.tile.focused {{ border-color: rgba(79, 209, 217, 0.85); }}
.tile textview, .tile textview text, .tile scrolledwindow, .tile vte-terminal {{ background-color: transparent; }}"
    )
}

impl TileView {
    pub fn new(on_change: Rc<dyn Fn()>) -> Rc<RefCell<TileView>> {
        let view = Rc::new(RefCell::new(TileView {
            tree: TileTree::default(),
            tiles: HashMap::new(),
            fixed: gtk::Fixed::new(),
            area: Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 },
            me: Weak::new(),
            wrap: false,
            on_change,
        }));
        view.borrow_mut().me = Rc::downgrade(&view);
        view
    }

    pub fn widget(&self) -> &gtk::Fixed {
        &self.fixed
    }

    pub fn is_empty(&self) -> bool {
        self.tree.is_empty()
    }

    /// The code region (window minus `gap.tile.out`), recomputed on resize and drawer animation.
    pub fn set_area(&mut self, area: Rect) {
        if area != self.area {
            self.area = area;
            self.relayout();
        }
    }

    pub fn set_wrap(&mut self, wrap: bool) {
        self.wrap = wrap;
        for tile in self.tiles.values().filter(|t| t.buffer.is_some()) {
            if let Some(view) = tile.input.downcast_ref::<gtk::TextView>() {
                view.set_wrap_mode(wrap_mode(wrap));
            }
        }
    }

    fn rects(&self) -> Vec<(TileId, Rect)> {
        self.tree.layout(self.area, token::GAP_TILE_IN)
    }

    pub fn add(&mut self, kind: Kind, axis: Option<Axis>) {
        let focused_rect = self.rects().into_iter().find(|(id, _)| Some(*id) == self.tree.focused()).map(|(_, r)| r);
        let id = self.tree.split(axis, focused_rect);
        let (content, input, buffer, path) = match kind {
            Kind::Editor(path) => {
                let (scroll, view, buffer) = editor(path, self.wrap);
                (scroll, view, Some(buffer), path.map(Path::to_path_buf))
            }
            Kind::Terminal => {
                let term = terminal();
                // Closing the shell (`exit`, Ctrl+D) closes its tile.
                let me = self.me.clone();
                term.connect_child_exited(move |_, _| {
                    let me = me.clone();
                    // Deferred: the signal can fire while a key handler holds the view.
                    glib::idle_add_local_once(move || {
                        if let Some(view) = me.upgrade() {
                            view.borrow_mut().close(id);
                        }
                    });
                });
                (term.clone().upcast(), term.upcast(), None, None)
            }
        };
        let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
        frame.add_css_class("tile");
        frame.set_overflow(gtk::Overflow::Hidden);
        content.set_hexpand(true);
        content.set_vexpand(true);
        frame.append(&content);

        // Clicking anywhere in a tile focuses it; capture phase so the child still gets the click.
        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let me = self.me.clone();
        click.connect_pressed(move |_, _, _, _| {
            if let Some(view) = me.upgrade()
                && let Ok(mut view) = view.try_borrow_mut()
            {
                view.tree.focus(id);
                view.apply_focus(false);
            }
        });
        frame.add_controller(click);

        self.fixed.put(&frame, 0.0, 0.0);
        self.tiles.insert(id, Tile { frame, input, buffer, path });
        self.relayout();
        self.apply_focus(true);
        (self.on_change)();
    }

    pub fn close_focused(&mut self) {
        if let Some(id) = self.tree.focused() {
            self.close(id);
        }
    }

    fn close(&mut self, id: TileId) {
        if self.tree.close(id)
            && let Some(tile) = self.tiles.remove(&id)
        {
            self.fixed.remove(&tile.frame);
        }
        self.relayout();
        self.apply_focus(true);
        (self.on_change)();
    }

    pub fn focus_direction(&mut self, direction: Direction) {
        self.tree.focus_direction(direction, self.area, token::GAP_TILE_IN);
        self.apply_focus(true);
    }

    pub fn resize(&mut self, direction: Direction) {
        self.tree.resize_focused(direction, 0.05);
        self.relayout();
    }

    /// Ctrl+S: writes the focused editor to its file; an untitled buffer asks for a path.
    pub fn save_focused(&mut self, parent: &gtk::Window) {
        let Some(id) = self.tree.focused() else { return };
        let Some(tile) = self.tiles.get(&id) else { return };
        let Some(buffer) = tile.buffer.clone() else { return };
        if let Some(path) = &tile.path {
            write(path, &buffer);
            return;
        }
        let me = self.me.clone();
        gtk::FileDialog::builder().title("Save file").build().save(Some(parent), None::<&gio::Cancellable>, move |result| {
            let Ok(path) = result.map(|file| file.path()) else { return };
            let Some(path) = path else { return };
            write(&path, &buffer);
            if let Some(view) = me.upgrade()
                && let Some(tile) = view.borrow_mut().tiles.get_mut(&id)
            {
                tile.path = Some(path);
            }
        });
    }

    pub fn set_visible(&self, visible: bool) {
        self.fixed.set_visible(visible);
        if visible {
            self.apply_focus(true);
        }
    }

    fn relayout(&self) {
        for (id, r) in self.rects() {
            if let Some(tile) = self.tiles.get(&id) {
                self.fixed.move_(&tile.frame, r.x, r.y);
                tile.frame.set_size_request(r.w.max(1.0) as i32, r.h.max(1.0) as i32);
            }
        }
    }

    /// Keyboard focus can only be grabbed once the window is shown.
    pub fn grab_focused(&self) {
        self.apply_focus(true);
    }

    fn apply_focus(&self, grab: bool) {
        let focused = self.tree.focused();
        for (id, tile) in &self.tiles {
            if Some(*id) == focused {
                tile.frame.add_css_class("focused");
                if grab {
                    tile.input.grab_focus();
                }
            } else {
                tile.frame.remove_css_class("focused");
            }
        }
    }
}

fn write(path: &Path, buffer: &sourceview5::Buffer) {
    let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true);
    match std::fs::write(path, text.as_str()) {
        Ok(()) => println!("saved {}", path.display()),
        Err(e) => eprintln!("cannot save {}: {e}", path.display()),
    }
}

/// Wraps at word boundaries, or inside a word that is longer than the line.
fn wrap_mode(wrap: bool) -> gtk::WrapMode {
    if wrap { gtk::WrapMode::WordChar } else { gtk::WrapMode::None }
}

fn editor(path: Option<&Path>, wrap: bool) -> (gtk::Widget, gtk::Widget, sourceview5::Buffer) {
    let buffer = sourceview5::Buffer::new(None::<&gtk::TextTagTable>);
    if let Some(scheme) = sourceview5::StyleSchemeManager::default().scheme("Adwaita-dark") {
        buffer.set_style_scheme(Some(&scheme));
    }
    if let Some(path) = path {
        if let Some(language) = sourceview5::LanguageManager::default().guess_language(Some(path), None) {
            buffer.set_language(Some(&language));
        }
        match std::fs::read_to_string(path) {
            Ok(text) => buffer.set_text(&text),
            Err(e) => eprintln!("cannot open {}: {e}", path.display()),
        }
        buffer.place_cursor(&buffer.start_iter());
    }

    let view = sourceview5::View::with_buffer(&buffer);
    view.set_monospace(true);
    view.set_show_line_numbers(true);
    view.set_highlight_current_line(true);
    view.set_auto_indent(true);
    view.set_tab_width(4);
    view.set_left_margin(8);
    view.set_top_margin(8);
    view.set_wrap_mode(wrap_mode(wrap));

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_child(Some(&view));
    (scroll.upcast(), view.upcast(), buffer)
}

fn terminal() -> vte4::Terminal {
    let term = vte4::Terminal::new();
    let fg = gdk::RGBA::new(0.90, 0.91, 0.93, 1.0);
    // Transparent, so the tile's translucent background shows through. The theme's CSS
    // also paints the `vte-terminal` node grey; `css()` above clears that.
    let bg = gdk::RGBA::new(0.0, 0.0, 0.0, 0.0);
    term.set_colors(Some(&fg), Some(&bg), &[]);
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let cwd = std::env::current_dir().ok();
    term.spawn_async(
        vte4::PtyFlags::DEFAULT,
        cwd.as_deref().and_then(Path::to_str),
        &[&shell],
        &[],
        glib::SpawnFlags::DEFAULT,
        || {},
        -1,
        None::<&gio::Cancellable>,
        |result| {
            if let Err(e) = result {
                eprintln!("terminal: cannot start shell: {e}");
            }
        },
    );
    term
}
