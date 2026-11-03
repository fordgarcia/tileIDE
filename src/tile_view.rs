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
use crate::{lsp, motion};
use tileide_core::layout::{Layout, LayoutKind};
use tileide_core::theme::{self, Theme};
use tileide_core::tiles::{Axis, Direction, Rect, TileId};
use tileide_core::token::{self, Geometry};
use tileide_lsp::protocol::Position;
use tileide_lsp::text::Encoding;
use vte4::prelude::*;

pub enum Kind<'a> {
    /// `None` opens an untitled buffer; saving asks for a path.
    Editor(Option<&'a Path>),
    Terminal,
    /// A terminal in this folder whose shell is handed this command line (it stays open
    /// afterwards, with the output).
    Command(&'a Path, &'a str),
    /// Any other view in a tile (the debug panel).
    Panel(gtk::Widget),
}

struct Tile {
    frame: gtk::Box,
    /// The Wikipedia rice's header: a dot, the name, "2 of 3" and ×.
    head: Head,
    /// What receives keyboard focus (the text view or the terminal).
    input: gtk::Widget,
    /// Editors only.
    buffer: Option<sourceview5::Buffer>,
    path: Option<PathBuf>,
    /// `.ipynb` files open as notebooks.
    notebook: Option<Rc<crate::notebook_view::NotebookView>>,
}

struct Head {
    root: gtk::Box,
    name: gtk::Label,
    position: gtk::Label,
}

impl Head {
    fn new(on_close: impl Fn() + 'static) -> Head {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        root.add_css_class("tile-head");
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("dot");
        dot.set_valign(gtk::Align::Center);
        let name = gtk::Label::new(None);
        name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        name.set_hexpand(true);
        name.set_xalign(0.0);
        let position = gtk::Label::new(None);
        position.add_css_class("position");
        let close = gtk::Button::with_label("×");
        close.add_css_class("flat");
        close.set_tooltip_text(Some("Close (Alt+W)"));
        close.connect_clicked(move |_| on_close());
        root.append(&dot);
        root.append(&name);
        root.append(&position);
        root.append(&close);
        root.set_visible(false);
        Head { root, name, position }
    }
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
    /// `TILEIDE_TRACE_GLIDE`: when the glide was asked for, and each frame's time.
    trace: Option<(std::time::Instant, Vec<i64>)>,
}

fn tracing() -> bool {
    std::env::var_os("TILEIDE_TRACE_GLIDE").is_some()
}

/// A tile frame's layout manager: children stack from the top at their natural height
/// (the header), and the last one (the content) gets the rest, but never less than
/// `token::MIN_TILE`. A tile may shrink to a sliver (no limit on splits); the frame
/// clips what doesn't fit. Without the floor a terminal in a tiny tile gets a negative
/// size and GTK aborts.
mod floor {
    use gtk::subclass::prelude::*;
    use gtk::{glib, prelude::*};
    use gtk4 as gtk;
    use tileide_core::token::MIN_TILE;

    #[derive(Default)]
    pub struct Imp;

    #[glib::object_subclass]
    impl ObjectSubclass for Imp {
        const NAME: &'static str = "TileideFloorLayout";
        type Type = FloorLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for Imp {}

    impl LayoutManagerImpl for Imp {
        fn request_mode(&self, _: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::ConstantSize
        }

        fn measure(&self, _: &gtk::Widget, _: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            (0, 0, -1, -1)
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, _: i32) {
            let width = width.max(MIN_TILE.0 as i32);
            let mut y = 0;
            let mut child = widget.first_child();
            while let Some(c) = child {
                child = c.next_sibling();
                if !c.should_layout() {
                    continue;
                }
                let _ = c.measure(gtk::Orientation::Horizontal, -1);
                if child.is_some() {
                    let (_, natural, _, _) = c.measure(gtk::Orientation::Vertical, width);
                    c.size_allocate(&gtk::Allocation::new(0, y, width, natural), -1);
                    y += natural;
                } else {
                    c.size_allocate(&gtk::Allocation::new(0, y, width, (height - y).max(MIN_TILE.1 as i32)), -1);
                }
            }
        }
    }

    glib::wrapper! {
        pub struct FloorLayout(ObjectSubclass<Imp>) @extends gtk::LayoutManager;
    }

    impl FloorLayout {
        pub fn new() -> FloorLayout {
            glib::Object::new()
        }
    }
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
    /// The workspace's folder: language servers start for it, terminals in it (home
    /// when `None`).
    folder: Option<PathBuf>,
    /// Called when tiles appear or disappear, so the canvas can redraw its empty hint.
    on_change: Rc<dyn Fn()>,
    /// `tiling.max_tiles`.
    max_tiles: usize,
    /// What `Alt+V` toggles to from dwindle: columns, or the Wikipedia rice's stack.
    scrolling: LayoutKind,
    /// Tiles show a header (Wikipedia rice).
    heads: bool,
}

/// Shows a message on the app's banner.
type Notice = Rc<dyn Fn(String)>;

thread_local! {
    static NOTICE: RefCell<Option<Notice>> = const { RefCell::new(None) };
    /// `alerts.tile_limit`: the refusal pops up on the banner.
    static LIMIT_ALERT: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
    /// `editor.format_on_save`.
    static FORMAT_ON_SAVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn set_limit_alert(on: bool) {
    LIMIT_ALERT.set(on);
}

pub fn set_format_on_save(on: bool) {
    FORMAT_ON_SAVE.set(on);
}

pub fn set_notice(notice: Notice) {
    NOTICE.set(Some(notice));
}

/// `tile_opacity`: the tile fill's opacity (Settings → Themes & background → Tiles &
/// terminals; terminals are transparent on it). Border: `appearance.tile_border` (width,
/// colour); the focused tile's is the accent, no glow. The tile layer is clipped to the frame's rounded hole, so columns
/// scrolled past an edge slide under the frame's curve. Glass themes: a diagonal fill
/// (its darker end follows the opacity), a 1px top highlight, and the blurred backdrop
/// the canvas paints under each tile (`draw::Scene::frosted`).
pub fn css(t: &Theme, g: Geometry, tile_opacity: f64) -> String {
    let (edge_rgb, edge_alpha) = g.tile_edge.paint(t);
    let (fill, focus) = match t.glass {
        Some(glass) => {
            let bottom = (tile_opacity + glass.bottom_opacity - glass.opacity).clamp(0.0, 1.0);
            (
                format!(
                    "background-image: linear-gradient(160deg, {}, {}); box-shadow: inset 0 1px {};",
                    theme::css(t.tile, tile_opacity),
                    theme::css(glass.bottom, bottom),
                    theme::css(t.text, 0.12),
                ),
                theme::css(t.accent, glass.focus_opacity),
            )
        }
        None => (format!("background-color: {};", theme::css(t.tile, tile_opacity)), theme::css(t.accent, 1.0)),
    };
    // Without a border the focused tile still shows: an outline inside its edge, which
    // (unlike a border) doesn't move the content.
    let focus_rule = if g.tile_border > 0.0 { format!("border-color: {focus};") } else { format!("outline: 2px solid {focus}; outline-offset: -2px;") };
    format!(
        ".tiles {{ border-radius: {stage_radius}px; }}
.tile {{ {fill} color: {text}; border-radius: {radius}px; border: {border}px solid {edge}; }}
.tile.focused {{ {focus_rule} }}
.tile textview, .tile textview text, .tile scrolledwindow, .tile vte-terminal, .tile textview border, .tile textview gutter, .tile textview gutterrenderer {{ background-color: transparent; background-image: none; }}
.tile textview text {{ color: {text}; }}
.tile vte-terminal {{ padding: 18px 24px; }}
.tile-head {{ padding: 8px 10px 8px 16px; border-bottom: 1px solid {head_edge}; font-size: 13px; }}
.tile-head .dot {{ min-width: 8px; min-height: 8px; border-radius: 4px; background-color: {muted}; }}
.tile.focused .tile-head .dot {{ background-color: {accent}; }}
.tile-head .position {{ color: {muted}; font-size: 11px; }}
.tile-head button {{ min-height: 20px; min-width: 20px; padding: 0 6px; color: {muted}; }}",
        text = theme::css(t.text, 1.0),
        radius = g.tile_radius,
        border = g.tile_border,
        head_edge = theme::css(t.text, 0.08),
        muted = theme::css(t.muted, 1.0),
        accent = theme::css(t.accent, 1.0),
        edge = theme::css(edge_rgb, edge_alpha),
        stage_radius = g.frame_radius(),
    )
}

impl TileView {
    pub fn new(kind: LayoutKind, column_width: f64, gap: f64, theme: Rc<Theme>, folder: Option<PathBuf>, on_change: Rc<dyn Fn()>) -> Rc<RefCell<TileView>> {
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
            max_tiles: token::MAX_TILES as usize,
            scrolling: LayoutKind::Columns,
            heads: false,
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

    pub fn set_max_tiles(&mut self, max: u32) {
        self.max_tiles = max.max(1) as usize;
    }

    pub fn set_peek(&mut self, peek: f64) {
        if self.layout.set_peek(peek, self.area, self.gap) {
            self.relayout();
        }
    }

    pub fn set_column_width(&mut self, width: f64) {
        self.layout.set_column_width(width);
    }

    /// A new folder also moves open editors to its language servers.
    pub fn set_folder(&mut self, folder: Option<PathBuf>) {
        if self.folder == folder {
            return;
        }
        self.folder = folder;
        let ids: Vec<TileId> = self.tiles.keys().copied().collect();
        for id in ids {
            self.attach_lsp(id);
        }
    }

    /// Language support for editor `id`, if it has a file.
    fn attach_lsp(&self, id: TileId) {
        let Some(tile) = self.tiles.get(&id) else { return };
        let (Some(buffer), Some(path), Some(view)) = (&tile.buffer, &tile.path, tile.input.downcast_ref::<sourceview5::View>()) else { return };
        let me = self.me.clone();
        let goto: lsp::Goto = Rc::new(move |path, at, encoding| {
            if let Some(view) = me.upgrade() {
                view.borrow_mut().goto(&path, at, encoding);
            }
        });
        lsp::attach(buffer, view, path, self.folder.as_deref(), goto);
        crate::debug::attach(buffer, view, path);
        if let Some(language) = buffer.language() {
            crate::extensions::event(tileide_extensions::Event::Language(&language.id()));
        }
    }

    /// Go to definition: the file's tile (opened if needed), cursor on `at`.
    fn goto(&mut self, path: &Path, at: Position, encoding: Encoding) {
        let id = match self.find(path) {
            Some(id) => {
                self.layout.focus(id, self.area, self.gap);
                self.relayout();
                self.apply_focus(true);
                id
            }
            None => {
                self.add(Kind::Editor(Some(path)));
                let Some(id) = self.find(path) else { return };
                id
            }
        };
        let Some(tile) = self.tiles.get(&id) else { return };
        let (Some(buffer), Some(view)) = (tile.buffer.clone(), tile.input.downcast_ref::<sourceview5::View>().cloned()) else { return };
        buffer.place_cursor(&lsp::iter_at(buffer.upcast_ref(), at, encoding));
        // After a new tile's first layout, or the scroll has nothing to measure.
        glib::idle_add_local_once(move || view.scroll_to_mark(&buffer.get_insert(), 0.1, false, 0.0, 0.0));
    }

    /// Ends language support for every editor (the workspace is being removed).
    pub fn release(&self) {
        for tile in self.tiles.values() {
            if let Some(buffer) = &tile.buffer {
                lsp::detach(buffer);
            }
            lsp::release_popovers(&tile.input);
        }
    }

    /// The focused tile's editor, if it is one (hover, definition and completion act on it).
    pub fn focused_editor(&self) -> Option<sourceview5::View> {
        self.tiles.get(&self.layout.focused()?)?.input.downcast_ref::<sourceview5::View>().cloned()
    }

    /// Where language actions apply: the focused editor, or a notebook's current cell.
    pub fn focused_code(&self) -> Option<sourceview5::View> {
        self.focused_editor().or_else(|| self.focused_notebook()?.current_view())
    }

    /// The focused tile's notebook, if it is one.
    pub fn focused_notebook(&self) -> Option<Rc<crate::notebook_view::NotebookView>> {
        self.tiles.get(&self.layout.focused()?)?.notebook.clone()
    }

    /// The focused notebook, else any notebook here (test actions, while a panel it
    /// opened has the focus).
    pub fn some_notebook(&self) -> Option<Rc<crate::notebook_view::NotebookView>> {
        self.focused_notebook().or_else(|| self.tiles.values().find_map(|t| t.notebook.clone()))
    }

    /// The focused editor has unsaved changes.
    pub fn focused_dirty(&self) -> bool {
        self.layout.focused().and_then(|id| self.tiles.get(&id)).and_then(|t| t.buffer.as_ref()).is_some_and(|b| b.is_modified())
    }

    /// The focused editor's file, if it has one.
    pub fn focused_file(&self) -> Option<PathBuf> {
        self.tiles.get(&self.layout.focused()?)?.path.clone()
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
            } else if let Some(nb) = &tile.notebook {
                nb.set_theme(theme.clone());
            } else if let Some(term) = tile.input.downcast_ref::<vte4::Terminal>() {
                apply_terminal_colours(term, &theme);
            }
        }
        self.theme = theme;
    }

    /// Why a new tile can't open: the workspace holds `tiling.max_tiles` already. Size
    /// is no limit: tiles may get tiny (`floor` keeps their content safe).
    fn refusal(&self) -> Option<String> {
        (self.tiles.len() >= self.max_tiles)
            .then(|| format!("This workspace has {} tiles, the most allowed (Settings → Most tiles per workspace). Close one first.", self.max_tiles))
    }

    /// Opens a tile next to the focused one: a new column in the columns layout, a
    /// split in dwindle (see `split_next` for the direction).
    pub fn add(&mut self, kind: Kind) {
        let began = std::time::Instant::now();
        // A file that is already open gets focus instead of a second, diverging buffer.
        if let Kind::Editor(Some(path)) = &kind
            && let Some(id) = self.find(path)
        {
            self.layout.focus(id, self.area, self.gap);
            self.relayout();
            self.apply_focus(true);
            return;
        }
        if let Some(refusal) = self.refusal() {
            if LIMIT_ALERT.get() {
                NOTICE.with_borrow(|notice| notice.as_ref().map(|n| n(refusal)));
            } else {
                println!("tiles: {refusal}");
            }
            return;
        }
        // The new tile grows out of the one it splits from.
        let parent = self.layout.focused().and_then(|f| self.glide.shown.get(&f).copied());
        let id = self.layout.add(self.area, self.gap);
        let mut notebook = None;
        let (content, input, buffer, path) = match kind {
            Kind::Editor(Some(path)) if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("ipynb")) => {
                let me = self.me.clone();
                let goto: lsp::Goto = Rc::new(move |path, at, encoding| {
                    if let Some(view) = me.upgrade() {
                        view.borrow_mut().goto(&path, at, encoding);
                    }
                });
                let me = self.me.clone();
                // Deferred: it can be asked while this view is borrowed (a key handler).
                let open_panel: crate::notebook_view::OpenPanel = Rc::new(move |widget| {
                    let me = me.clone();
                    glib::idle_add_local_once(move || {
                        if let Some(view) = me.upgrade()
                            && !view.borrow().shows(&widget)
                        {
                            view.borrow_mut().add(Kind::Panel(widget));
                        }
                    });
                });
                let nb = crate::notebook_view::NotebookView::open(path, self.theme.clone(), self.folder.clone(), goto, open_panel);
                let root: gtk::Widget = nb.widget().clone().upcast();
                notebook = Some(nb);
                (root.clone(), root, None, Some(path.to_path_buf()))
            }
            Kind::Editor(path) => {
                let (scroll, view, buffer) = editor(path, self.wrap, &self.theme);
                (scroll, view, Some(buffer), path.map(Path::to_path_buf))
            }
            Kind::Panel(widget) => {
                // Moved here from another workspace's tile, if it was shown there.
                if let Some(frame) = widget.parent().and_downcast::<gtk::Box>() {
                    frame.remove(&widget);
                }
                (widget.clone(), widget, None, None)
            }
            Kind::Terminal | Kind::Command(..) => {
                let home = crate::workspaces::expand_home("~");
                let (dir, command) = match kind {
                    Kind::Command(dir, command) => (dir, Some(command)),
                    _ => (self.folder.as_deref().unwrap_or(&home), None),
                };
                let term = terminal(&self.theme, dir, command);
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
        frame.set_layout_manager(Some(floor::FloorLayout::new()));
        let me = self.me.clone();
        let head = Head::new(move || {
            let Some(view) = me.upgrade() else { return };
            let Ok(mut view) = view.try_borrow_mut() else { return };
            let (area, gap) = (view.area, view.gap);
            view.layout.focus(id, area, gap);
            if let Some(window) = view.container.root().and_downcast::<gtk::Window>() {
                view.close_focused(&window);
            }
        });
        head.root.set_visible(self.heads);
        frame.append(&head.root);
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
        self.tiles.insert(id, Tile { frame, head, input, buffer, path, notebook });
        // The language server (a process start and the whole file sent) waits until the
        // tile has settled, so it never costs a frame of the opening glide.
        let me = self.me.clone();
        glib::timeout_add_local_once(std::time::Duration::from_micros(tileide_core::motion::move_us() as u64), move || {
            if let Some(view) = me.upgrade()
                && let Ok(view) = view.try_borrow()
            {
                view.attach_lsp(id);
            }
        });
        self.relayout();
        if let Some(parent) = parent.filter(|_| !self.glide.from.is_empty()) {
            self.glide.from.insert(id, parent);
            self.place();
        }
        self.apply_focus(true);
        (self.on_change)();
        if tracing() {
            eprintln!("trace: add() took {:.1} ms", began.elapsed().as_secs_f64() * 1000.0);
        }
    }

    /// The axis the next new tile uses (`split vertical` / `horizontal`).
    pub fn split_next(&mut self, axis: Axis) {
        self.layout.pending_axis = Some(axis);
    }

    /// The rice: what `Alt+V` toggles to, the layout to use now, and tile headers.
    pub fn set_rice(&mut self, scrolling: LayoutKind, layout: LayoutKind, heads: bool) {
        self.scrolling = scrolling;
        self.heads = heads;
        for tile in self.tiles.values() {
            tile.head.root.set_visible(heads);
        }
        if layout != self.layout.kind() {
            self.layout.set_kind(layout, self.area, self.gap);
            self.relayout();
        }
        self.apply_focus(false);
    }

    /// Each header's name and "n of m", in reading order.
    fn refresh_heads(&self) {
        if !self.heads {
            return;
        }
        let order = self.layout.ids();
        for (i, id) in order.iter().enumerate() {
            let Some(tile) = self.tiles.get(id) else { continue };
            let name = match (&tile.path, &tile.notebook) {
                (Some(path), _) => path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned()),
                (None, _) if tile.input.is::<vte4::Terminal>() => "Terminal".to_string(),
                (None, _) if tile.buffer.is_some() => "Untitled".to_string(),
                // A panel names itself with its widget name (GTK's default is the type).
                _ => Some(tile.input.widget_name().to_string()).filter(|n| !n.starts_with("Gtk")).unwrap_or_else(|| "Panel".into()),
            };
            let dirty = tile.buffer.as_ref().is_some_and(|b| b.is_modified());
            tile.head.name.set_text(&if dirty { format!("{name} •") } else { name });
            tile.head.position.set_text(&format!("{} of {}", i + 1, order.len()));
        }
    }

    /// `None` toggles between dwindle and the rice's scrolling layout.
    pub fn set_layout(&mut self, kind: Option<LayoutKind>) {
        match kind {
            Some(kind) => self.layout.set_kind(kind, self.area, self.gap),
            None => self.layout.toggle_kind(self.area, self.gap, self.scrolling),
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
        self.tiles.get(&id).is_some_and(|t| t.input.is::<vte4::Terminal>())
    }

    /// Whether `widget` is shown in one of this view's tiles.
    pub fn shows(&self, widget: &gtk::Widget) -> bool {
        self.tiles.values().any(|t| &t.input == widget)
    }

    /// Opens `path` (or focuses its tile) with the cursor on 1-based `line`; returns
    /// the editor for the caller to mark the line.
    pub fn show_line(&mut self, path: &Path, line: u32) -> Option<(sourceview5::View, sourceview5::Buffer)> {
        let at = Position { line: line.saturating_sub(1), column: 0 };
        self.goto(path, at, Encoding::Utf16);
        let tile = self.tiles.get(&self.find(path)?)?;
        Some((tile.input.downcast_ref::<sourceview5::View>()?.clone(), tile.buffer.clone()?))
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
        if let Some(old) = &tile.buffer {
            lsp::detach(old);
        }
        // A replaced notebook's kernel ends with it.
        if let Some(nb) = tile.notebook.take() {
            nb.shutdown();
        }
        lsp::release_popovers(&tile.input);
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
        self.attach_lsp(id);
        self.apply_focus(true);
        (self.on_change)();
    }

    /// Editors with unsaved changes in this view.
    pub fn dirty_count(&self) -> usize {
        self.tiles.keys().filter(|id| self.dirty_name(**id).is_some()).count()
    }

    pub fn terminal_count(&self) -> usize {
        self.tiles.keys().filter(|id| self.is_terminal(**id)).count()
    }

    /// The tile's display name if it is an editor with unsaved changes.
    fn dirty_name(&self, id: TileId) -> Option<String> {
        let tile = self.tiles.get(&id)?;
        if !tile.notebook.as_ref().is_some_and(|n| n.is_dirty()) {
            tile.buffer.as_ref().filter(|b| b.is_modified())?;
        }
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
            if let Some(buffer) = &tile.buffer {
                lsp::detach(buffer);
            }
            if let Some(nb) = &tile.notebook {
                nb.shutdown();
            }
            // Also for views language support never attached to (see `release_popovers`).
            lsp::release_popovers(&tile.input);
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
        if let Some(nb) = &tile.notebook {
            if nb.save()
                && let Some(then) = then
            {
                glib::idle_add_local_once(then);
            }
            return;
        }
        let Some(buffer) = tile.buffer.clone() else { return };
        if let Some(path) = &tile.path {
            let path = path.clone();
            let write_now = move || {
                if write(&path, &buffer)
                    && let Some(then) = then
                {
                    glib::idle_add_local_once(then);
                }
            };
            match tile.input.downcast_ref::<sourceview5::View>().filter(|_| FORMAT_ON_SAVE.get()) {
                Some(view) => lsp::tidy(view, Box::new(write_now)),
                None => write_now(),
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
            if let Some(view) = me.upgrade() {
                let mut view = view.borrow_mut();
                if let Some(tile) = view.tiles.get_mut(&id) {
                    tile.path = Some(path);
                    // Saved under a name: now it has a language (and a server).
                    view.attach_lsp(id);
                }
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
            if tracing() {
                self.glide.trace = Some((std::time::Instant::now(), Vec::new()));
            }
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
        if let Some((_, frames)) = self.glide.trace.as_mut() {
            frames.push(now_us);
        }
        if done {
            self.glide.from.clear();
            if let Some((asked, frames)) = self.glide.trace.take() {
                let mut gaps: Vec<f64> = frames.windows(2).map(|w| (w[1] - w[0]) as f64 / 1000.0).collect();
                gaps.sort_by(f64::total_cmp);
                let first = asked.elapsed().as_secs_f64() * 1000.0 - (now_us - frames[0]) as f64 / 1000.0;
                eprintln!(
                    "trace: glide {} frames, first frame {first:.1} ms after the request, frame gap median {:.1} ms, max {:.1} ms",
                    frames.len(),
                    gaps.get(gaps.len() / 2).copied().unwrap_or(0.0),
                    gaps.last().copied().unwrap_or(0.0)
                );
            }
        }
        self.place();
    }

    /// Headless checks: the code region and every tile's target rect, focused marked.
    pub fn describe(&self) -> String {
        let focused = self.layout.focused();
        let a = self.area;
        let tiles: Vec<String> = self
            .layout
            .layout(a, self.gap, self.layout.scroll())
            .into_iter()
            .map(|(id, r)| format!("{}{id} x {:.0}..{:.0}", if Some(id) == focused { "*" } else { "" }, r.x - a.x, r.x + r.w - a.x))
            .collect();
        format!("area w {:.0}: {}", a.w, tiles.join(", "))
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
        // The canvas paints the blurred backdrop under each tile, so it must follow them.
        if crate::draw::tile_backdrop() {
            (self.on_change)();
        }
    }

    /// Keyboard focus can only be grabbed once the window is shown.
    pub fn grab_focused(&self) {
        self.apply_focus(true);
    }

    fn apply_focus(&self, grab: bool) {
        self.refresh_heads();
        let focused = self.layout.focused();
        for (id, tile) in &self.tiles {
            if Some(*id) == focused {
                tile.frame.add_css_class("focused");
                if grab {
                    match &tile.notebook {
                        Some(nb) => nb.focus(),
                        None => {
                            tile.input.grab_focus();
                        }
                    }
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
            lsp::saved(buffer);
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
/// The editor's colours, all from the theme: text, cursor and selection like the
/// terminal's, syntax from `Theme::syntax` (the terminal's hues), line numbers on no
/// background and the current line as a faint white band. Adwaita (light or dark) is
/// the parent, for the styles a theme has no colour for (search matches, diff marks).
pub fn apply_scheme(buffer: &sourceview5::Buffer, theme: &Theme) {
    let manager = sourceview5::StyleSchemeManager::default();
    let scheme = tileide_scheme(&manager, theme).or_else(|| manager.scheme(if theme.dark { "Adwaita-dark" } else { "Adwaita" }));
    if let Some(scheme) = scheme {
        buffer.set_style_scheme(Some(&scheme));
    }
}

fn tileide_scheme(manager: &sourceview5::StyleSchemeManager, theme: &Theme) -> Option<sourceview5::StyleScheme> {
    let hex = |c: theme::Rgb| format!("#{:02X}{:02X}{:02X}", (c.0 * 255.0).round() as u8, (c.1 * 255.0).round() as u8, (c.2 * 255.0).round() as u8);
    let (text, muted, accent, on_accent, error) = (hex(theme.text), hex(theme.muted), hex(theme.accent), hex(theme.on_accent), hex(theme.error));
    let selection = hex(theme::over(theme.accent, 0.3, theme.tile));
    let s = theme.syntax();
    let (keyword, type_, string, constant, function, preprocessor, builtin, comment) =
        (hex(s.keyword), hex(s.type_), hex(s.string), hex(s.constant), hex(s.function), hex(s.preprocessor), hex(s.builtin), hex(s.comment));
    // White reads as a lift on dark tiles; light tiles need more of it to show.
    let band = if theme.dark { "#FFFFFF12" } else { "#FFFFFF99" };
    let parent = if theme.dark { "Adwaita-dark" } else { "Adwaita" };
    let styles = format!(
        r##"  <style name="text" foreground="{text}"/>
  <style name="selection" foreground="{text}" background="{selection}"/>
  <style name="cursor" foreground="{accent}"/>
  <style name="bracket-match" foreground="{on_accent}" background="{accent}" bold="true"/>
  <style name="current-line" background="{band}"/>
  <style name="current-line-number" foreground="{text}" background="{band}" bold="true"/>
  <style name="line-numbers" foreground="{muted}" background="#00000000"/>
  <style name="def:comment" foreground="{comment}" italic="true"/>
  <style name="def:doc-comment-element" foreground="{comment}" bold="true"/>
  <style name="def:keyword" foreground="{keyword}"/>
  <style name="def:statement" foreground="{keyword}"/>
  <style name="def:type" foreground="{type_}"/>
  <style name="def:string" foreground="{string}"/>
  <style name="def:special-char" foreground="{constant}"/>
  <style name="def:constant" foreground="{constant}"/>
  <style name="def:number" foreground="{constant}"/>
  <style name="def:boolean" foreground="{constant}"/>
  <style name="def:special-constant" foreground="{constant}"/>
  <style name="def:function" foreground="{function}"/>
  <style name="def:preprocessor" foreground="{preprocessor}"/>
  <style name="def:builtin" foreground="{builtin}"/>
  <style name="def:identifier" foreground="{text}"/>
  <style name="def:operator" foreground="{text}"/>
  <style name="def:net-address" foreground="{function}" underline="single"/>
  <style name="def:heading" foreground="{accent}" bold="true"/>
  <style name="def:emphasis" italic="true"/>
  <style name="def:strong-emphasis" bold="true"/>
  <style name="def:error" underline="error" underline-color="{error}"/>
  <style name="def:warning" underline="error" underline-color="{type_}"/>
"##
    );
    // One file per distinct set of colours; the id changes whenever any colour does.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&(parent, &styles), &mut hasher);
    let id = format!("tileide-{:016x}", std::hash::Hasher::finish(&hasher));
    if manager.scheme(&id).is_none() {
        let dir = glib::user_runtime_dir().join("tileide-schemes");
        let xml = format!(
            r##"<?xml version="1.0" encoding="UTF-8"?>
<style-scheme id="{id}" name="TileIDE" version="1.0" parent-scheme="{parent}">
{styles}</style-scheme>
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

/// Loads what the first editor needs (language definitions, the generated colour
/// scheme, the text view's styles) with a throwaway editor, off any animation.
pub fn prewarm(theme: &Theme) {
    let started = std::time::Instant::now();
    let (_, view, buffer) = editor(None, false, theme);
    let manager = sourceview5::LanguageManager::default();
    buffer.set_language(manager.guess_language(Some("a.rs"), None).as_ref());
    let _ = view.measure(gtk::Orientation::Horizontal, -1);
    if tracing() {
        eprintln!("trace: editor prewarm {:.1} ms", started.elapsed().as_secs_f64() * 1000.0);
    }
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
    crate::editing::attach(&view);

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

/// A terminal running the user's shell in `folder`; with a `command`, `sh` shows and
/// runs it first, then becomes the shell (typing it into the shell instead loses it to
/// shells that flush input at start-up).
fn terminal(theme: &Theme, folder: &Path, command: Option<&str>) -> vte4::Terminal {
    let term = vte4::Terminal::new();
    apply_terminal_colours(&term, theme);
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let argv: Vec<&str> = match command {
        Some(command) => vec!["/bin/sh", "-c", r#"printf '$ %s\n' "$1"; eval "$1"; exec "$2""#, "sh", command, &shell],
        None => vec![&shell],
    };
    term.spawn_async(
        vte4::PtyFlags::DEFAULT,
        folder.to_str(),
        &argv,
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
