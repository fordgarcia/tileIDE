// SPDX-License-Identifier: GPL-3.0-or-later
//! One workspace's tiles: GtkSourceView editors and VTE terminals, placed by a
//! `tileide_core::layout::Layout` (dwindle or scrolling columns) inside a container
//! that sits over the canvas drawing, clipped to the code region. Its layout manager
//! gives every tile exactly its rect: `gtk::Fixed` would use a child's natural size,
//! and a terminal that was once large reports a large natural width.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use gtk::{gdk, gio, glib};
use gtk4 as gtk;
use sourceview5::prelude::*;
use crate::dialog::{self, Role};
use crate::motion;
use tileide_core::layout::{Layout, LayoutKind};
use tileide_core::theme::{self, Theme};
use tileide_core::tiles::{Axis, Direction, Rect, TileId};
use tileide_core::token::Geometry;
use vte4::prelude::*;

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

/// Tiles glide from where they are shown to where the layout puts them: opening,
/// closing, resizing, moving, `Alt+V` and the columns scroll (`tileide_core::motion`).
#[derive(Default)]
struct Glide {
    /// Where each tile was drawn last.
    shown: HashMap<TileId, Rect>,
    /// Where the running glide started; empty when nothing moves.
    from: HashMap<TileId, Rect>,
    start_us: Option<i64>,
    /// Eased 0..1 progress of the running glide.
    k: f64,
    /// The frame callback, only while something glides (idle costs no wakeups).
    ticking: Option<gtk::TickCallbackId>,
}

/// A layout manager that gives each child exactly the rect stored for its widget name
/// (`tile-<id>`), whatever the child measures.
mod exact {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use gtk::subclass::prelude::*;
    use gtk::{glib, prelude::*};
    use gtk4 as gtk;
    use tileide_core::tiles::Rect;

    #[derive(Default)]
    pub struct Imp {
        pub rects: RefCell<HashMap<String, Rect>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Imp {
        const NAME: &'static str = "TileideExactLayout";
        type Type = ExactLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for Imp {}

    impl LayoutManagerImpl for Imp {
        fn request_mode(&self, _: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::ConstantSize
        }

        /// The container's own size comes from `set_size_request`.
        fn measure(&self, _: &gtk::Widget, _: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            (0, 0, -1, -1)
        }

        fn allocate(&self, widget: &gtk::Widget, _: i32, _: i32, _: i32) {
            let rects = self.rects.borrow();
            let mut child = widget.first_child();
            while let Some(c) = child {
                if let Some(r) = rects.get(c.widget_name().as_str())
                    && c.should_layout()
                {
                    // GTK requires a measure before every allocation.
                    let _ = c.measure(gtk::Orientation::Horizontal, -1);
                    c.size_allocate(&gtk::Allocation::new(r.x.round() as i32, r.y.round() as i32, r.w.max(1.0) as i32, r.h.max(1.0) as i32), -1);
                }
                child = c.next_sibling();
            }
        }
    }

    glib::wrapper! {
        pub struct ExactLayout(ObjectSubclass<Imp>) @extends gtk::LayoutManager;
    }

    impl ExactLayout {
        pub fn new() -> ExactLayout {
            glib::Object::new()
        }

        pub fn set(&self, name: &str, rect: Rect) {
            self.imp().rects.borrow_mut().insert(name.into(), rect);
        }

        pub fn remove(&self, name: &str) {
            self.imp().rects.borrow_mut().remove(name);
        }
    }
}

pub struct TileView {
    layout: Layout,
    tiles: HashMap<TileId, Tile>,
    container: gtk::Box,
    placement: exact::ExactLayout,
    area: Rect,
    stage: Rect,
    gap: f64,
    glide: Glide,
    me: Weak<RefCell<TileView>>,
    /// Word wrap for this view's editors (Alt+Z).
    wrap: bool,
    theme: Rc<Theme>,
    /// Where new terminals start (the workspace's folder).
    folder: PathBuf,
    /// Called when tiles appear or disappear, so the canvas can redraw its empty hint.
    on_change: Rc<dyn Fn()>,
}

/// `tile_opacity`: the tile background's opacity (the full-screen background's control).
/// Focused tile: a plain 2px accent border (no glow). The tile layer is clipped to the
/// frame's rounded hole, so columns scrolled past an edge slide under the frame's curve.
/// Glass themes ignore `tile_opacity`: a diagonal fill, a 1px top highlight, and the
/// blurred backdrop the canvas paints under each tile (`draw::Scene::frosted`).
/// `see_behind`: TileIDE paints the background (else the glass fill keeps `min_tint`).
pub fn css(t: &Theme, g: Geometry, tile_opacity: f64, see_behind: bool) -> String {
    let (fill, edge, focus) = match t.glass {
        Some(glass) => {
            let floor = if see_behind { 0.0 } else { glass.min_tint };
            (
                format!(
                    "background-image: linear-gradient(160deg, {}, {}); box-shadow: inset 0 1px {};",
                    theme::css(t.tile, glass.opacity.max(floor)),
                    theme::css(glass.bottom, glass.bottom_opacity.max(floor)),
                    theme::css(t.text, 0.12),
                ),
                theme::css(t.text, glass.edge_opacity),
                theme::css(t.accent, glass.focus_opacity),
            )
        }
        None => (format!("background-color: {};", theme::css(t.tile, tile_opacity)), theme::css(t.text, 0.08), theme::css(t.accent, 1.0)),
    };
    format!(
        ".tiles {{ border-radius: {stage_radius}px; }}
.tile {{ {fill} color: {text}; border-radius: {radius}px; border: 2px solid {edge}; }}
.tile.focused {{ border-color: {focus}; }}
.tile textview, .tile textview text, .tile scrolledwindow, .tile vte-terminal, .tile textview gutter {{ background-color: transparent; }}
.tile textview text {{ color: {text}; }}
.tile vte-terminal {{ padding: 18px 24px; }}",
        text = theme::css(t.text, 1.0),
        radius = g.tile_radius,
        stage_radius = g.frame_radius(),
    )
}

impl TileView {
    pub fn new(kind: LayoutKind, column_width: f64, gap: f64, theme: Rc<Theme>, folder: PathBuf, on_change: Rc<dyn Fn()>) -> Rc<RefCell<TileView>> {
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        // Placed over the frame's hole (the stage) by `set_area` and clipped to its rounded
        // corners, so a column scrolled past an edge disappears under the frame.
        container.set_halign(gtk::Align::Start);
        container.set_valign(gtk::Align::Start);
        container.set_overflow(gtk::Overflow::Hidden);
        container.add_css_class("tiles");
        let placement = exact::ExactLayout::new();
        container.set_layout_manager(Some(placement.clone()));
        let view = Rc::new(RefCell::new(TileView {
            layout: Layout::new(kind, column_width),
            tiles: HashMap::new(),
            container: container.clone(),
            placement,
            area: Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 },
            stage: Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 },
            gap,
            glide: Glide::default(),
            me: Weak::new(),
            wrap: false,
            theme,
            folder,
            on_change,
        }));
        view.borrow_mut().me = Rc::downgrade(&view);
        view
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }

    pub fn is_empty(&self) -> bool {
        self.layout.is_empty()
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    pub fn layout_kind(&self) -> LayoutKind {
        self.layout.kind()
    }

    /// The code region (stage minus `gaps_out`) and the stage itself (the frame's hole,
    /// the clip), recomputed on resize and drawer animation.
    pub fn set_area(&mut self, area: Rect, stage: Rect) {
        if area != self.area || stage != self.stage {
            self.area = area;
            self.stage = stage;
            self.container.set_margin_start(stage.x.max(0.0) as i32);
            self.container.set_margin_top(stage.y.max(0.0) as i32);
            self.container.set_size_request(stage.w.max(0.0) as i32, stage.h.max(0.0) as i32);
            // Follows the drawer frame by frame, so no glide here.
            self.layout.fit(area, self.gap);
            self.glide.from.clear();
            self.place();
        }
    }

    pub fn set_gap(&mut self, gap: f64) {
        if gap != self.gap {
            self.gap = gap;
            self.relayout();
        }
    }

    pub fn set_column_width(&mut self, width: f64) {
        self.layout.set_column_width(width);
    }

    pub fn set_folder(&mut self, folder: PathBuf) {
        self.folder = folder;
    }

    pub fn set_wrap(&mut self, wrap: bool) {
        self.wrap = wrap;
        for tile in self.tiles.values().filter(|t| t.buffer.is_some()) {
            if let Some(view) = tile.input.downcast_ref::<gtk::TextView>() {
                view.set_wrap_mode(wrap_mode(wrap));
            }
        }
    }

    /// Recolours the open editors and terminals.
    pub fn set_theme(&mut self, theme: Rc<Theme>) {
        for tile in self.tiles.values() {
            if let Some(buffer) = &tile.buffer {
                apply_scheme(buffer, &theme);
            } else if let Some(term) = tile.input.downcast_ref::<vte4::Terminal>() {
                apply_terminal_colours(term, &theme);
            }
        }
        self.theme = theme;
    }

    /// Opens a tile next to the focused one: a new column in the columns layout, a
    /// split in dwindle (see `split_next` for the direction).
    pub fn add(&mut self, kind: Kind) {
        // A file that is already open gets focus instead of a second, diverging buffer.
        if let Kind::Editor(Some(path)) = &kind
            && let Some(id) = self.find(path)
        {
            self.layout.focus(id, self.area, self.gap);
            self.relayout();
            self.apply_focus(true);
            return;
        }
        // The new tile grows out of the one it splits from.
        let parent = self.layout.focused().and_then(|f| self.glide.shown.get(&f).copied());
        let id = self.layout.add(self.area, self.gap);
        let (content, input, buffer, path) = match kind {
            Kind::Editor(path) => {
                let (scroll, view, buffer) = editor(path, self.wrap, &self.theme);
                (scroll, view, Some(buffer), path.map(Path::to_path_buf))
            }
            Kind::Terminal => {
                let term = terminal(&self.theme, &self.folder);
                // Closing the shell (`exit`, Ctrl+D) closes its tile.
                let me = self.me.clone();
                term.connect_child_exited(move |_, _| {
                    let me = me.clone();
                    // Deferred: the signal can fire while a key handler holds the view.
                    glib::idle_add_local_once(move || {
                        // Not if Alt+F already turned the tile into an editor.
                        if let Some(view) = me.upgrade()
                            && view.borrow().is_terminal(id)
                        {
                            view.borrow_mut().close(id);
                        }
                    });
                });
                (term.clone().upcast(), term.upcast(), None, None)
            }
        };
        let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
        frame.set_widget_name(&format!("tile-{id}"));
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
                let (area, gap) = (view.area, view.gap);
                view.layout.focus(id, area, gap);
                view.relayout();
                view.apply_focus(false);
            }
        });
        frame.add_controller(click);

        self.container.append(&frame);
        motion::fade_in(&frame);
        self.tiles.insert(id, Tile { frame, input, buffer, path });
        self.relayout();
        if let Some(parent) = parent.filter(|_| !self.glide.from.is_empty()) {
            self.glide.from.insert(id, parent);
            self.place();
        }
        self.apply_focus(true);
        (self.on_change)();
    }

    /// The axis the next new tile uses (`split vertical` / `horizontal`).
    pub fn split_next(&mut self, axis: Axis) {
        self.layout.pending_axis = Some(axis);
    }

    /// `None` toggles between dwindle and columns.
    pub fn set_layout(&mut self, kind: Option<LayoutKind>) {
        match kind {
            Some(kind) => self.layout.set_kind(kind, self.area, self.gap),
            None => self.layout.toggle_kind(self.area, self.gap),
        }
        self.relayout();
        self.apply_focus(true);
        (self.on_change)();
    }

    fn find(&self, path: &Path) -> Option<TileId> {
        let wanted = path.canonicalize().ok()?;
        self.tiles.iter().find(|(_, t)| t.path.as_ref().and_then(|p| p.canonicalize().ok()).as_ref() == Some(&wanted)).map(|(id, _)| *id)
    }

    fn is_terminal(&self, id: TileId) -> bool {
        self.tiles.get(&id).is_some_and(|t| t.buffer.is_none())
    }

    /// Alt+F: shows `path` in the focused tile. An editor loads it in place; a terminal
    /// becomes an editor. Both ask first when something would be lost (unsaved
    /// changes, a running shell). A file open in another tile is focused instead, and
    /// with no tile it opens in a new one.
    pub fn open_in_focused(&mut self, path: &Path, parent: &gtk::Window) {
        let focused = self.layout.focused();
        if self.find(path).is_some() || focused.is_none() {
            self.add(Kind::Editor(Some(path)));
            return;
        }
        let id = focused.unwrap_or_default();
        let (message, detail, buttons): (String, &str, &[(&str, Role)]) = if self.is_terminal(id) {
            ("Replace this terminal?".into(), "Its shell and anything running in it will end.", &[("Cancel", Role::Cancel), ("Replace", Role::Destructive)])
        } else if let Some(name) = self.dirty_name(id) {
            (format!("Save changes to {name}?"), "Your changes are lost if you don't save them.", &[("Cancel", Role::Cancel), ("Don't Save", Role::Destructive), ("Save", Role::Primary)])
        } else {
            self.replace_with_file(id, path);
            return;
        };
        let (me, owner, path) = (self.me.clone(), parent.clone(), path.to_path_buf());
        // Default: Save for an editor; Cancel for a terminal (ending a shell is never the default).
        let default = if buttons.len() == 3 { 2 } else { 0 };
        dialog::ask(&message, detail, buttons, default, move |choice| {
            let Some(view) = me.upgrade() else { return };
            match choice {
                // "Replace" (terminal) or "Don't Save" (editor).
                1 => view.borrow_mut().replace_with_file(id, &path),
                2 => {
                    let me = me.clone();
                    let then: Box<dyn FnOnce()> = Box::new(move || {
                        if let Some(view) = me.upgrade() {
                            view.borrow_mut().replace_with_file(id, &path);
                        }
                    });
                    view.borrow().save(id, &owner, Some(then));
                }
                _ => {}
            }
        });
    }

    /// Swaps the tile's content for an editor on `path`; the tile keeps its place. A
    /// replaced terminal's shell ends when its widget is dropped.
    fn replace_with_file(&mut self, id: TileId, path: &Path) {
        let Some(tile) = self.tiles.get_mut(&id) else { return };
        let (content, input, buffer) = editor(Some(path), self.wrap, &self.theme);
        content.set_hexpand(true);
        content.set_vexpand(true);
        while let Some(child) = tile.frame.first_child() {
            tile.frame.remove(&child);
        }
        tile.frame.append(&content);
        tile.input = input;
        tile.buffer = Some(buffer);
        tile.path = Some(path.to_path_buf());
        self.apply_focus(true);
        (self.on_change)();
    }

    /// Editors with unsaved changes in this view.
    pub fn dirty_count(&self) -> usize {
        self.tiles.keys().filter(|id| self.dirty_name(**id).is_some()).count()
    }

    /// The tile's display name if it is an editor with unsaved changes.
    fn dirty_name(&self, id: TileId) -> Option<String> {
        let tile = self.tiles.get(&id)?;
        tile.buffer.as_ref().filter(|b| b.is_modified())?;
        Some(tile.path.as_deref().and_then(Path::file_name).map_or_else(|| "Untitled".into(), |n| n.to_string_lossy().into_owned()))
    }

    /// Alt+W. Unsaved changes are never dropped silently: a dirty editor asks first.
    pub fn close_focused(&mut self, parent: &gtk::Window) {
        let Some(id) = self.layout.focused() else { return };
        let Some(name) = self.dirty_name(id) else {
            self.close(id);
            return;
        };
        let (me, owner) = (self.me.clone(), parent.clone());
        dialog::ask(
            &format!("Save changes to {name}?"),
            "Your changes are lost if you don't save them.",
            &[("Cancel", Role::Cancel), ("Don't Save", Role::Destructive), ("Save", Role::Primary)],
            2,
            move |choice| {
                let Some(view) = me.upgrade() else { return };
                match choice {
                    1 => view.borrow_mut().close(id),
                    2 => {
                        let me = me.clone();
                        let then: Box<dyn FnOnce()> = Box::new(move || {
                            if let Some(view) = me.upgrade() {
                                view.borrow_mut().close(id);
                            }
                        });
                        view.borrow().save(id, &owner, Some(then));
                    }
                    _ => {}
                }
            },
        );
    }

    fn close(&mut self, id: TileId) {
        if self.layout.close(id, self.area, self.gap)
            && let Some(tile) = self.tiles.remove(&id)
        {
            // It fades where it stands while the others glide into its space.
            let (container, placement, frame) = (self.container.clone(), self.placement.clone(), tile.frame.clone());
            motion::fade_out_then(&tile.frame, move || {
                container.remove(&frame);
                placement.remove(frame.widget_name().as_str());
            });
        }
        self.relayout();
        self.apply_focus(true);
        (self.on_change)();
    }

    pub fn focus_direction(&mut self, direction: Direction) {
        self.layout.focus_direction(direction, self.area, self.gap);
        self.relayout();
        self.apply_focus(true);
    }

    pub fn resize(&mut self, direction: Direction) {
        self.layout.resize_focused(direction, self.area, self.gap);
        self.relayout();
    }

    pub fn move_focused(&mut self, direction: Direction) {
        self.layout.move_focused(direction, self.area, self.gap);
        self.relayout();
        self.apply_focus(true);
    }

    /// Ctrl+S: writes the focused editor to its file; an untitled buffer asks for a path.
    pub fn save_focused(&mut self, parent: &gtk::Window) {
        if let Some(id) = self.layout.focused() {
            self.save(id, parent, None);
        }
    }

    /// Writes tile `id`'s buffer, asking for a path if it has none. `then` runs (from the
    /// main loop, never inside this borrow) only after a successful write.
    fn save(&self, id: TileId, parent: &gtk::Window, then: Option<Box<dyn FnOnce()>>) {
        let Some(tile) = self.tiles.get(&id) else { return };
        let Some(buffer) = tile.buffer.clone() else { return };
        if let Some(path) = &tile.path {
            if write(path, &buffer)
                && let Some(then) = then
            {
                glib::idle_add_local_once(then);
            }
            return;
        }
        let me = self.me.clone();
        gtk::FileDialog::builder().title("Save file").build().save(Some(parent), None::<&gio::Cancellable>, move |result| {
            let Ok(path) = result.map(|file| file.path()) else { return };
            let Some(path) = path else { return };
            if !write(&path, &buffer) {
                return;
            }
            if let Some(view) = me.upgrade()
                && let Some(tile) = view.borrow_mut().tiles.get_mut(&id)
            {
                tile.path = Some(path);
            }
            if let Some(then) = then {
                then();
            }
        });
    }

    /// A switch crossfades: the workspace left fades out quickly while this one fades in.
    pub fn set_visible(&self, visible: bool) {
        if visible {
            if !motion::shown(&self.container) {
                motion::fade_in(&self.container);
            }
            self.apply_focus(true);
        } else {
            motion::fade_out(&self.container);
        }
    }

    /// Places tiles after a layout change, gliding from where they are shown now.
    fn relayout(&mut self) {
        if motion::enabled() && !self.glide.shown.is_empty() {
            self.glide.from = self.glide.shown.clone();
            self.glide.start_us = None;
            self.glide.k = 0.0;
            self.start_ticking();
        }
        self.place();
    }

    fn start_ticking(&mut self) {
        if self.glide.ticking.is_some() {
            return;
        }
        let me = self.me.clone();
        self.glide.ticking = Some(self.container.add_tick_callback(move |_, clock| {
            let Some(view) = me.upgrade() else { return glib::ControlFlow::Break };
            let Ok(mut view) = view.try_borrow_mut() else { return glib::ControlFlow::Continue };
            view.tick(clock.frame_time());
            if view.glide.from.is_empty() {
                view.glide.ticking = None;
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        }));
    }

    fn tick(&mut self, now_us: i64) {
        if self.glide.from.is_empty() {
            return;
        }
        let start = *self.glide.start_us.get_or_insert(now_us);
        let (k, done) = tileide_core::motion::progress(start, now_us);
        self.glide.k = k;
        if done {
            self.glide.from.clear();
        }
        self.place();
    }

    /// Where the shown tiles are, in window coordinates (for the glass backdrop).
    pub fn tile_rects(&self) -> Vec<Rect> {
        let stage = self.stage;
        self.glide.shown.values().copied().filter(|r| r.x < stage.x + stage.w && r.x + r.w > stage.x).collect()
    }

    /// Positions are relative to the container, which covers the stage. Tiles entirely
    /// outside the stage are hidden, so they cost nothing to draw. A tile new to the
    /// glide appears at its place (it fades in, see `add`).
    fn place(&mut self) {
        let (ox, oy) = (self.stage.x, self.stage.y);
        let mut shown = HashMap::new();
        for (id, target) in self.layout.layout(self.area, self.gap, self.layout.scroll()) {
            let Some(tile) = self.tiles.get(&id) else { continue };
            let r = match self.glide.from.get(&id) {
                Some(&from) => tileide_core::motion::lerp_rect(from, target, self.glide.k),
                None => target,
            };
            shown.insert(id, r);
            let visible = r.x < self.stage.x + self.stage.w && r.x + r.w > self.stage.x;
            tile.frame.set_visible(visible);
            self.placement.set(tile.frame.widget_name().as_str(), Rect { x: r.x - ox, y: r.y - oy, ..r });
        }
        self.glide.shown = shown;
        self.container.queue_allocate();
        // The canvas paints the glass backdrop under each tile, so it must follow them.
        if self.theme.glass.is_some() {
            (self.on_change)();
        }
    }

    /// Keyboard focus can only be grabbed once the window is shown.
    pub fn grab_focused(&self) {
        self.apply_focus(true);
    }

    fn apply_focus(&self, grab: bool) {
        let focused = self.layout.focused();
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

/// Returns whether the file was written; the buffer is then clean.
fn write(path: &Path, buffer: &sourceview5::Buffer) -> bool {
    let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true);
    match std::fs::write(path, text.as_str()) {
        Ok(()) => {
            buffer.set_modified(false);
            println!("saved {}", path.display());
            true
        }
        Err(e) => {
            eprintln!("cannot save {}: {e}", path.display());
            false
        }
    }
}

/// Wraps at word boundaries, or inside a word that is longer than the line.
fn wrap_mode(wrap: bool) -> gtk::WrapMode {
    if wrap { gtk::WrapMode::WordChar } else { gtk::WrapMode::None }
}

/// Syntax colours follow the theme's mode until themes carry their own syntax palette.
/// The editor's syntax colours: Adwaita (light or dark) with TileIDE's own gutter and
/// current line (a theme's colours, so it is generated per theme): line numbers on no
/// background, and the current line as a faint white band.
fn apply_scheme(buffer: &sourceview5::Buffer, theme: &Theme) {
    let manager = sourceview5::StyleSchemeManager::default();
    let scheme = tileide_scheme(&manager, theme).or_else(|| manager.scheme(if theme.dark { "Adwaita-dark" } else { "Adwaita" }));
    if let Some(scheme) = scheme {
        buffer.set_style_scheme(Some(&scheme));
    }
}

fn tileide_scheme(manager: &sourceview5::StyleSchemeManager, theme: &Theme) -> Option<sourceview5::StyleScheme> {
    let hex = |c: tileide_core::theme::Rgb| format!("#{:02X}{:02X}{:02X}", (c.0 * 255.0).round() as u8, (c.1 * 255.0).round() as u8, (c.2 * 255.0).round() as u8);
    let (text, muted) = (hex(theme.text), hex(theme.muted));
    // White reads as a lift on dark tiles; light tiles need more of it to show.
    let band = if theme.dark { "#FFFFFF12" } else { "#FFFFFF99" };
    let id = format!("tileide-{}{}{}", if theme.dark { "dark" } else { "light" }, &text[1..], &muted[1..]);
    if manager.scheme(&id).is_none() {
        let dir = glib::user_runtime_dir().join("tileide-schemes");
        let parent = if theme.dark { "Adwaita-dark" } else { "Adwaita" };
        let xml = format!(
            r##"<?xml version="1.0" encoding="UTF-8"?>
<style-scheme id="{id}" name="TileIDE" version="1.0" parent-scheme="{parent}">
  <style name="current-line" background="{band}"/>
  <style name="current-line-number" foreground="{text}" background="{band}" bold="true"/>
  <style name="line-numbers" foreground="{muted}" background="#00000000"/>
</style-scheme>
"##
        );
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(dir.join(format!("{id}.xml")), xml).ok()?;
        if !manager.search_path().iter().any(|p| Path::new(p.as_str()) == dir) {
            manager.append_search_path(dir.to_str()?);
        }
        manager.force_rescan();
    }
    manager.scheme(&id)
}

fn editor(path: Option<&Path>, wrap: bool, theme: &Theme) -> (gtk::Widget, gtk::Widget, sourceview5::Buffer) {
    let buffer = sourceview5::Buffer::new(None::<&gtk::TextTagTable>);
    apply_scheme(&buffer, theme);
    if let Some(path) = path {
        if let Some(language) = sourceview5::LanguageManager::default().guess_language(Some(path), None) {
            buffer.set_language(Some(&language));
        }
        match std::fs::read_to_string(path) {
            Ok(text) => buffer.set_text(&text),
            Err(e) => eprintln!("cannot open {}: {e}", path.display()),
        }
        buffer.place_cursor(&buffer.start_iter());
        // Loading the file is not an edit.
        buffer.set_modified(false);
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

/// Transparent background, so the tile's translucent fill shows through. The GTK
/// theme's CSS also paints the `vte-terminal` node grey; `css()` above clears that.
/// Text, the 16 ANSI colours, cursor and selection all come from the theme.
fn apply_terminal_colours(term: &vte4::Terminal, theme: &Theme) {
    let rgba = |(r, g, b): theme::Rgb| gdk::RGBA::new(r as f32, g as f32, b as f32, 1.0);
    let palette = theme.terminal_palette().map(rgba);
    let palette: Vec<&gdk::RGBA> = palette.iter().collect();
    term.set_colors(Some(&rgba(theme.text)), Some(&gdk::RGBA::new(0.0, 0.0, 0.0, 0.0)), &palette);
    term.set_color_cursor(Some(&rgba(theme.accent)));
    term.set_color_cursor_foreground(Some(&rgba(theme.on_accent)));
    term.set_color_highlight(Some(&rgba(theme::over(theme.accent, 0.3, theme.tile))));
    term.set_color_highlight_foreground(Some(&rgba(theme.text)));
}

fn terminal(theme: &Theme, folder: &Path) -> vte4::Terminal {
    let term = vte4::Terminal::new();
    apply_terminal_colours(&term, theme);
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    term.spawn_async(
        vte4::PtyFlags::DEFAULT,
        folder.to_str(),
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
