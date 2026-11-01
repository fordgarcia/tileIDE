// SPDX-License-Identifier: GPL-3.0-or-later
//! A Jupyter notebook as a tile (v0.4 phase 4): a toolbar (kernel state, run all,
//! interrupt, restart, add cells), then the cells, each with its own editor and its
//! outputs underneath (text, errors, images). `Shift+Enter` runs a cell and moves on,
//! `Ctrl+Enter` runs it in place, `Alt+R` runs every cell. The kernel starts on the
//! first run, through `tileide_notebook::kernel` and the user's Jupyter.

use std::cell::{Cell as StdCell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::{gdk, glib};
use gtk4 as gtk;
use sourceview5::prelude::*;
use tileide_core::theme::{self, Theme};
use tileide_notebook::kernel::{self, Event, Kernel};
use tileide_notebook::{Cell, CellKind, Notebook, Shown, push_output, shown};

pub fn css(t: &Theme) -> String {
    let c = theme::css;
    format!(
        ".notebook .bar {{ padding: 10px 14px 6px 16px; }}
.notebook .kernel {{ font-size: 12px; border-radius: 10px; padding: 2px 10px; background-color: {chip}; color: {muted}; }}
.notebook .kernel.busy {{ background-color: {accent_bg}; color: {accent}; }}
.notebook .kernel.failed {{ background-color: {error_bg}; color: {error}; }}
.notebook button.pill {{ min-height: 24px; padding: 0 10px; border-radius: 12px; border: 1px solid {edge}; background: transparent; box-shadow: none; color: {text}; font-size: 12px; }}
.notebook button.pill:hover, .notebook button.icon:hover {{ background-color: {hover}; }}
.notebook button.icon {{ min-width: 24px; min-height: 24px; padding: 0; border: none; box-shadow: none; background: transparent; color: {muted}; border-radius: 12px; }}
.notebook .cell {{ border-radius: 12px; border: 1px solid transparent; background-color: {cell}; padding: 4px 6px 6px 6px; }}
.notebook .cell.current {{ border-color: {accent_edge}; }}
.notebook .cell .count {{ font-family: monospace; font-size: 12px; color: {muted}; padding-left: 6px; }}
.notebook .cell textview, .notebook .cell textview text {{ background: transparent; }}
.notebook .output {{ font-family: monospace; font-size: 0.95em; color: {text}; padding: 4px 10px; }}
.notebook .output.error {{ color: {error}; }}
.notebook .outputs {{ border-top: 1px solid {edge}; margin-top: 4px; padding-top: 4px; }}
.notebook .outputs.stale {{ opacity: 0.45; }}
.notebook .frame-table {{ margin: 4px 10px; }}
.notebook .frame-table label {{ font-family: monospace; font-size: 0.92em; padding: 3px 10px; color: {text}; }}
.notebook .frame-table label.th {{ font-weight: bold; }}
.notebook .frame-table label.head {{ border-bottom: 1px solid {edge}; }}
.notebook .frame-table label.odd {{ background-color: {chip}; }}
.notebook .frame-table-note {{ color: {muted}; font-size: 12px; margin: 0 10px 4px 10px; }}
.notebook .cell .rendered {{ padding: 2px 10px 6px 10px; color: {text}; }}
.notebook .cell .rendered:focus-visible {{ outline: none; }}
.notebook .cell .rendered link {{ color: {accent}; }}",
        text = c(t.text, 1.0),
        muted = c(t.muted, 1.0),
        edge = c(t.text, 0.10),
        hover = c(t.text, 0.06),
        chip = c(t.text, 0.07),
        cell = c(t.text, 0.035),
        accent = c(t.accent, 1.0),
        accent_bg = c(t.accent, 0.16),
        accent_edge = c(t.accent, 0.55),
        error = c(t.error, 1.0),
        error_bg = c(t.error, 0.16),
    )
}

struct CellUi {
    /// Names the cell to the kernel (stable while it moves).
    key: u64,
    cell: Cell,
    frame: gtk::Box,
    count: gtk::Label,
    view: sourceview5::View,
    buffer: sourceview5::Buffer,
    /// A Markdown cell shown rendered (in place of `view`); focusable so arrows and
    /// Enter work on it.
    rendered: gtk::Box,
    markdown: gtk::Label,
    /// While the cell runs, the old output stays (faded, class `stale`) until the new
    /// arrives, so the page doesn't shrink and jump.
    outputs: gtk::Box,
}

/// Which way an arrow key leaves a cell.
#[derive(Clone, Copy, PartialEq)]
enum Way {
    Up,
    Down,
}

pub struct NotebookView {
    root: gtk::Box,
    list: gtk::Box,
    scroll: gtk::ScrolledWindow,
    chip: gtk::Label,
    path: PathBuf,
    notebook: RefCell<Notebook>,
    cells: RefCell<Vec<CellUi>>,
    current: StdCell<usize>,
    dirty: StdCell<bool>,
    kernel: RefCell<Option<Kernel>>,
    events: RefCell<Option<std::sync::mpsc::Receiver<Event>>>,
    /// Cells sent to the kernel before it was ready.
    waiting: RefCell<Vec<u64>>,
    ready: StdCell<bool>,
    starting: StdCell<bool>,
    /// The Python running the kernel (for install hints).
    python: RefCell<Option<PathBuf>>,
    next_key: StdCell<u64>,
    folder: Option<PathBuf>,
    theme: RefCell<Rc<Theme>>,
    /// Language support: the code cells as one Python file (`lsp::attach_cell`).
    goto: crate::lsp::Goto,
    lsp_cells: RefCell<Option<Rc<crate::lsp::Cells>>>,
    /// Kernel completion requests in flight, by id.
    completions: RefCell<std::collections::HashMap<u64, crate::lsp::KernelReply>>,
    next_completion: StdCell<u64>,
    me: RefCell<Weak<NotebookView>>,
}

fn icon(name: &str, tip: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(name);
    b.add_css_class("icon");
    b.set_tooltip_text(Some(tip));
    b.set_valign(gtk::Align::Center);
    b
}

fn pill(text: &str, tip: &str) -> gtk::Button {
    let b = gtk::Button::with_label(text);
    b.add_css_class("pill");
    b.set_tooltip_text(Some(tip));
    b.set_valign(gtk::Align::Center);
    b
}

impl NotebookView {
    /// Opens `path` (an empty or missing file becomes a new notebook). A file that isn't
    /// a notebook shows why, in place of the cells.
    pub fn open(path: &Path, theme: Rc<Theme>, folder: Option<PathBuf>, goto: crate::lsp::Goto) -> Rc<NotebookView> {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let (notebook, problem) = match Notebook::parse(&text) {
            Ok(nb) => (nb, None),
            Err(e) => (Notebook::new(), Some(e)),
        };
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("notebook");
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.add_css_class("bar");
        let chip = gtk::Label::new(Some("Kernel: not started"));
        chip.add_css_class("kernel");
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        let (run_all, interrupt, restart, add_code, add_md) = (
            pill("Run all", "Run every cell (Alt+R)"),
            pill("Interrupt", "Stop the running cell"),
            pill("Restart", "Restart the kernel (variables are lost)"),
            pill("+ Code", "Add a code cell below the current one"),
            pill("+ Text", "Add a Markdown cell below the current one"),
        );
        bar.append(&chip);
        bar.append(&spacer);
        for b in [&run_all, &interrupt, &restart, &add_code, &add_md] {
            bar.append(b);
        }
        root.append(&bar);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 10);
        list.set_margin_start(12);
        list.set_margin_end(12);
        list.set_margin_bottom(14);
        list.set_margin_top(4);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroll.set_vexpand(true);
        scroll.set_child(Some(&list));
        // TileIDE scrolls itself (`reveal`, `keep_cursor_visible`): GTK's scroll-to-focus
        // moved the page on every click and run.
        if let Some(viewport) = scroll.child().and_downcast::<gtk::Viewport>() {
            viewport.set_scroll_to_focus(false);
        }
        root.append(&scroll);

        let view = Rc::new(NotebookView {
            root,
            list,
            scroll,
            chip,
            path: path.to_path_buf(),
            notebook: RefCell::new(notebook),
            cells: RefCell::new(Vec::new()),
            current: StdCell::new(0),
            dirty: StdCell::new(false),
            kernel: RefCell::new(None),
            events: RefCell::new(None),
            waiting: RefCell::new(Vec::new()),
            ready: StdCell::new(false),
            starting: StdCell::new(false),
            python: RefCell::new(None),
            next_key: StdCell::new(1),
            folder,
            theme: RefCell::new(theme),
            goto,
            lsp_cells: RefCell::new(None),
            completions: RefCell::new(std::collections::HashMap::new()),
            next_completion: StdCell::new(1),
            me: RefCell::new(Weak::new()),
        });
        *view.me.borrow_mut() = Rc::downgrade(&view);
        let cells = std::mem::take(&mut view.notebook.borrow_mut().cells);
        for (i, cell) in cells.into_iter().enumerate() {
            view.insert(i, cell);
        }
        for i in 0..view.cells.borrow().len() {
            view.render_markdown(i);
        }
        // Language support once every cell is in: one document for the servers.
        let (me, me2) = (Rc::downgrade(&view), Rc::downgrade(&view));
        *view.lsp_cells.borrow_mut() = Some(Rc::new(crate::lsp::Cells {
            list: Box::new(move || me.upgrade().map(|v| v.code_buffers()).unwrap_or_default()),
            kernel: Box::new(move |code, cursor, reply| me2.upgrade().is_some_and(|v| v.kernel_complete(code, cursor, reply))),
        }));
        let code: Vec<usize> = (0..view.cells.borrow().len()).collect();
        for i in code {
            view.attach_lsp(i);
        }
        if let Some(problem) = problem {
            view.set_chip(&format!("Can't read: {problem}"), Some("failed"));
        }
        let me = Rc::downgrade(&view);
        let on = move |f: fn(&NotebookView)| {
            let me = me.clone();
            move |_: &gtk::Button| {
                if let Some(v) = me.upgrade() {
                    f(&v);
                }
            }
        };
        run_all.connect_clicked(on(NotebookView::run_all));
        interrupt.connect_clicked(on(|v| v.with_kernel(Kernel::interrupt)));
        restart.connect_clicked(on(|v| {
            v.with_kernel(Kernel::restart);
            v.waiting.borrow_mut().clear();
        }));
        add_code.connect_clicked(on(|v| v.add_below(CellKind::Code)));
        add_md.connect_clicked(on(|v| v.add_below(CellKind::Markdown)));
        view.set_current(0);
        view
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// The current cell's editor (language actions: hover, completion, …).
    pub fn current_view(&self) -> Option<sourceview5::View> {
        self.cells.borrow().get(self.current.get()).map(|c| c.view.clone())
    }

    /// Asks the running kernel for completions; false (and no reply) without one.
    fn kernel_complete(&self, code: &str, cursor: usize, reply: crate::lsp::KernelReply) -> bool {
        if !self.ready.get() {
            return false;
        }
        let id = self.next_completion.get();
        self.next_completion.set(id + 1);
        let Some(k) = self.kernel.borrow_mut().as_mut().map(|k| k.complete(id, code, cursor)) else { return false };
        if k.is_err() {
            return false;
        }
        self.completions.borrow_mut().insert(id, reply);
        true
    }

    fn code_buffers(&self) -> Vec<sourceview5::Buffer> {
        self.cells.borrow().iter().filter(|c| c.cell.kind == CellKind::Code).map(|c| c.buffer.clone()).collect()
    }

    /// Language support for cell `i`, if it is code (and the notebook is loaded).
    fn attach_lsp(&self, i: usize) {
        let Some(cells) = self.lsp_cells.borrow().clone() else { return };
        // The servers are Python's; other kernels' cells get none.
        if self.notebook.borrow().language() != "python3" {
            return;
        }
        let Some((buffer, view)) = self.cells.borrow().get(i).filter(|c| c.cell.kind == CellKind::Code).map(|c| (c.buffer.clone(), c.view.clone())) else { return };
        crate::lsp::attach_cell(&buffer, &view, &self.path, self.folder.as_deref(), self.goto.clone(), cells);
    }

    /// Cells moved, came or went: the servers' copy follows.
    fn lsp_resync(&self) {
        if let Some(buffer) = self.code_buffers().first() {
            crate::lsp::cells_changed(buffer);
        }
    }

    /// Where keyboard focus goes when the tile is focused.
    pub fn focus(&self) {
        if let Some(c) = self.cells.borrow().get(self.current.get()) {
            focus_target(c).grab_focus();
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty.get()
    }

    fn mark_dirty(&self) {
        self.dirty.set(true);
    }

    pub fn set_theme(&self, theme: Rc<Theme>) {
        for c in self.cells.borrow().iter() {
            crate::tile_view::apply_scheme(&c.buffer, &theme);
        }
        *self.theme.borrow_mut() = theme;
    }

    /// Writes the notebook (nbformat 4). Returns whether it was saved.
    pub fn save(&self) -> bool {
        let mut nb = self.notebook.borrow().clone();
        nb.cells = self
            .cells
            .borrow()
            .iter()
            .map(|c| {
                let mut cell = c.cell.clone();
                cell.source = c.buffer.text(&c.buffer.start_iter(), &c.buffer.end_iter(), false).to_string();
                cell
            })
            .collect();
        let tmp = self.path.with_extension("ipynb.tmp");
        match std::fs::write(&tmp, nb.to_json()).and_then(|()| std::fs::rename(&tmp, &self.path)) {
            Ok(()) => {
                self.dirty.set(false);
                true
            }
            Err(e) => {
                self.set_chip(&format!("Can't save: {e}"), Some("failed"));
                false
            }
        }
    }

    /// Stops the kernel (the tile is closing).
    pub fn shutdown(&self) {
        if let Some(k) = self.kernel.take() {
            k.shutdown();
        }
        self.events.take();
        // Also unparents the cells' popovers (a view disposed with one would hang GTK).
        for buffer in self.code_buffers() {
            crate::lsp::detach(&buffer);
        }
    }

    fn set_chip(&self, text: &str, class: Option<&str>) {
        self.chip.set_text(text);
        for c in ["busy", "failed"] {
            self.chip.remove_css_class(c);
        }
        if let Some(c) = class {
            self.chip.add_css_class(c);
        }
    }

    fn with_kernel(&self, f: fn(&mut Kernel) -> Result<(), String>) {
        if let Some(k) = self.kernel.borrow_mut().as_mut()
            && let Err(e) = f(k)
        {
            self.set_chip(&e, Some("failed"));
        }
    }

    fn insert(&self, at: usize, cell: Cell) {
        let key = self.next_key.get();
        self.next_key.set(key + 1);
        let frame = gtk::Box::new(gtk::Orientation::Vertical, 2);
        frame.add_css_class("cell");
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        let count = gtk::Label::new(None);
        count.add_css_class("count");
        count.set_xalign(0.0);
        count.set_hexpand(true);
        head.append(&count);
        let (run, up, down, kind, delete) = (
            icon("media-playback-start-symbolic", "Run (Ctrl+Enter; Shift+Enter runs and moves on)"),
            icon("go-up-symbolic", "Move up"),
            icon("go-down-symbolic", "Move down"),
            icon("format-text-rich-symbolic", "Switch between code and text"),
            icon("user-trash-symbolic", "Delete the cell"),
        );
        for b in [&run, &up, &down, &kind, &delete] {
            head.append(b);
        }
        frame.append(&head);

        let buffer = sourceview5::Buffer::new(None::<&gtk::TextTagTable>);
        crate::tile_view::apply_scheme(&buffer, &self.theme.borrow());
        buffer.set_text(&cell.source);
        let view = sourceview5::View::with_buffer(&buffer);
        view.set_monospace(true);
        view.set_auto_indent(true);
        view.set_tab_width(4);
        view.set_wrap_mode(gtk::WrapMode::WordChar);
        view.set_left_margin(8);
        view.set_top_margin(4);
        view.set_bottom_margin(6);
        frame.append(&view);
        let markdown = gtk::Label::new(None);
        markdown.set_wrap(true);
        markdown.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        markdown.set_xalign(0.0);
        let rendered = gtk::Box::new(gtk::Orientation::Vertical, 0);
        rendered.add_css_class("rendered");
        rendered.append(&markdown);
        rendered.set_focusable(true);
        rendered.set_visible(false);
        frame.append(&rendered);
        let outputs = gtk::Box::new(gtk::Orientation::Vertical, 2);
        outputs.add_css_class("outputs");
        frame.append(&outputs);

        let ui = CellUi { key, cell, frame: frame.clone(), count, view: view.clone(), buffer: buffer.clone(), rendered: rendered.clone(), markdown, outputs };
        self.style_cell(&ui);
        self.show_outputs(&ui);
        let sibling = if at == 0 { None } else { self.cells.borrow().get(at - 1).map(|c| c.frame.clone()) };
        self.list.insert_child_after(&frame, sibling.as_ref());
        self.cells.borrow_mut().insert(at, ui);

        let me = self.me.borrow().clone();
        buffer.connect_changed(move |_| {
            if let Some(v) = me.upgrade() {
                v.mark_dirty();
            }
        });
        let me = self.me.borrow().clone();
        let focus = gtk::EventControllerFocus::new();
        focus.connect_enter(move |_| {
            if let Some(v) = me.upgrade()
                && let Some(i) = v.index(key)
            {
                v.set_current(i);
            }
        });
        view.add_controller(focus);
        let me = self.me.borrow().clone();
        let focus = gtk::EventControllerFocus::new();
        focus.connect_enter(move |_| {
            if let Some(v) = me.upgrade()
                && let Some(i) = v.index(key)
            {
                v.set_current(i);
            }
        });
        rendered.add_controller(focus);
        // The page follows the cursor only when it would leave the screen.
        let me = self.me.borrow().clone();
        let v = view.downgrade();
        buffer.connect_cursor_position_notify(move |_| {
            if let (Some(nb), Some(view)) = (me.upgrade(), v.upgrade())
                && view.has_focus()
            {
                nb.keep_cursor_visible(&view);
            }
        });
        // ↑ on the first line / ↓ on the last goes to the next cell (as in JupyterLab).
        let arrows = gtk::EventControllerKey::new();
        arrows.set_propagation_phase(gtk::PropagationPhase::Capture);
        let me = self.me.borrow().clone();
        arrows.connect_key_pressed(move |c, k, _, mods| {
            let Some(v) = me.upgrade() else { return glib::Propagation::Proceed };
            let way = match k {
                gdk::Key::Up | gdk::Key::KP_Up => Way::Up,
                gdk::Key::Down | gdk::Key::KP_Down => Way::Down,
                _ => return glib::Propagation::Proceed,
            };
            let Some(view) = c.widget().and_downcast::<sourceview5::View>() else { return glib::Propagation::Proceed };
            if !(mods & gtk::accelerator_get_default_mod_mask()).is_empty() || list_open(&view) || !at_edge(&view, way) {
                return glib::Propagation::Proceed;
            }
            let Some(i) = v.index(key) else { return glib::Propagation::Proceed };
            if v.leave(i, way) { glib::Propagation::Stop } else { glib::Propagation::Proceed }
        });
        view.add_controller(arrows);
        // A rendered Markdown cell: arrows move on, Enter (or a double click) edits it.
        let keys = gtk::EventControllerKey::new();
        let me = self.me.borrow().clone();
        keys.connect_key_pressed(move |_, k, _, mods| {
            let Some(v) = me.upgrade() else { return glib::Propagation::Proceed };
            let Some(i) = v.index(key) else { return glib::Propagation::Proceed };
            match k {
                gdk::Key::Up | gdk::Key::KP_Up => {
                    v.leave(i, Way::Up);
                }
                gdk::Key::Down | gdk::Key::KP_Down => {
                    v.leave(i, Way::Down);
                }
                gdk::Key::Return | gdk::Key::KP_Enter if mods.contains(gdk::ModifierType::SHIFT_MASK) => v.advance(i),
                gdk::Key::Return | gdk::Key::KP_Enter if !mods.contains(gdk::ModifierType::CONTROL_MASK) => v.edit_markdown(i),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        rendered.add_controller(keys);
        let me = self.me.borrow().clone();
        let double = gtk::GestureClick::new();
        double.connect_pressed(move |_, presses, _, _| {
            if presses == 2
                && let Some(v) = me.upgrade()
                && let Some(i) = v.index(key)
            {
                v.edit_markdown(i);
            }
        });
        rendered.add_controller(double);
        // A click anywhere in the cell (its buttons, its outputs) makes it current too.
        let me = self.me.borrow().clone();
        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        click.connect_pressed(move |_, _, _, _| {
            if let Some(v) = me.upgrade()
                && let Some(i) = v.index(key)
            {
                v.set_current(i);
            }
        });
        frame.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        let me = self.me.borrow().clone();
        keys.connect_key_pressed(move |_, k, _, mods| {
            let Some(v) = me.upgrade() else { return glib::Propagation::Proceed };
            if !matches!(k, gdk::Key::Return | gdk::Key::KP_Enter) {
                return glib::Propagation::Proceed;
            }
            let Some(i) = v.index(key) else { return glib::Propagation::Proceed };
            if mods.contains(gdk::ModifierType::SHIFT_MASK) {
                v.run_cell(i);
                v.advance(i);
            } else if mods.contains(gdk::ModifierType::CONTROL_MASK) {
                v.run_cell(i);
            } else {
                return glib::Propagation::Proceed;
            }
            glib::Propagation::Stop
        });
        view.add_controller(keys);
        let me = self.me.borrow().clone();
        let act = move |f: fn(&NotebookView, usize)| {
            let me = me.clone();
            move |_: &gtk::Button| {
                if let Some(v) = me.upgrade()
                    && let Some(i) = v.index(key)
                {
                    f(&v, i);
                }
            }
        };
        run.connect_clicked(act(NotebookView::run_cell));
        up.connect_clicked(act(|v, i| v.swap(i, i.wrapping_sub(1))));
        down.connect_clicked(act(|v, i| v.swap(i, i + 1)));
        kind.connect_clicked(act(NotebookView::toggle_kind));
        delete.connect_clicked(act(NotebookView::delete));
    }

    fn index(&self, key: u64) -> Option<usize> {
        self.cells.borrow().iter().position(|c| c.key == key)
    }

    fn style_cell(&self, ui: &CellUi) {
        let language = match ui.cell.kind {
            CellKind::Code => self.notebook.borrow().language(),
            CellKind::Markdown => "markdown".into(),
            CellKind::Raw => String::new(),
        };
        ui.buffer.set_language(sourceview5::LanguageManager::default().language(&language).as_ref());
        ui.count.set_text(&match (ui.cell.kind, ui.cell.count) {
            (CellKind::Code, Some(n)) => format!("[{n}]"),
            (CellKind::Code, None) => "[ ]".into(),
            (CellKind::Markdown, _) => "Text".into(),
            (CellKind::Raw, _) => "Raw".into(),
        });
    }

    fn show_outputs(&self, ui: &CellUi) {
        ui.outputs.remove_css_class("stale");
        while let Some(child) = ui.outputs.first_child() {
            ui.outputs.remove(&child);
        }
        for output in &ui.cell.outputs {
            let widget: gtk::Widget = match shown(output) {
                Some(Shown::Text(text)) | Some(Shown::Error(text)) if text.is_empty() => continue,
                Some(Shown::Text(text)) => output_label(text.trim_end_matches('\n'), false).upcast(),
                Some(Shown::Error(text)) => output_label(&text, true).upcast(),
                Some(Shown::Table(table, _)) => table_widget(&table),
                Some(Shown::Image(png)) => match gdk::Texture::from_bytes(&glib::Bytes::from_owned(png)) {
                    Ok(texture) => {
                        let picture = gtk::Picture::for_paintable(&texture);
                        picture.set_can_shrink(true);
                        picture.set_content_fit(gtk::ContentFit::ScaleDown);
                        picture.set_halign(gtk::Align::Start);
                        picture.set_size_request(-1, texture.height().min(360));
                        picture.upcast()
                    }
                    Err(_) => output_label("(an image TileIDE couldn't decode)", true).upcast(),
                },
                None => continue,
            };
            ui.outputs.append(&widget);
        }
        ui.outputs.set_visible(ui.outputs.first_child().is_some());
    }

    fn set_current(&self, i: usize) {
        let cells = self.cells.borrow();
        for (n, c) in cells.iter().enumerate() {
            if n == i {
                c.frame.add_css_class("current");
            } else {
                c.frame.remove_css_class("current");
            }
        }
        self.current.set(i.min(cells.len().saturating_sub(1)));
    }

    /// Shift+Enter: focus the next cell, adding a code cell at the end. The page moves
    /// only if that cell's first line would be off screen.
    fn advance(&self, i: usize) {
        if i + 1 >= self.cells.borrow().len() {
            self.insert(i + 1, Cell::new(CellKind::Code));
            self.attach_lsp(i + 1);
            self.mark_dirty();
        }
        self.set_current(i + 1);
        self.reveal(i + 1, true, false);
    }

    /// Makes cell `i` current, then scrolls the whole cell into view and focuses it once
    /// it is laid out ("+ Code", delete, move). Focusing a cell GTK hasn't allocated yet
    /// would scroll to the top.
    fn focus_cell(&self, i: usize) {
        self.set_current(i);
        self.reveal(self.current.get(), true, true);
    }

    /// Scrolls cell `i` into view once it is laid out: all of it (`whole`), or just its
    /// head and first line; nothing moves if that part already shows. Focuses its editor
    /// (or rendered text) if `focus`. Polls briefly: a new cell has no frame clock yet.
    fn reveal(&self, i: usize, focus: bool, whole: bool) {
        let Some((target, frame)) = self.cells.borrow().get(i).map(|c| (focus_target(c), c.frame.clone())) else { return };
        let scroll = self.scroll.clone();
        let started = std::time::Instant::now();
        glib::timeout_add_local(Duration::from_millis(8), move || {
            if frame.parent().is_none() || started.elapsed() > Duration::from_secs(2) {
                return glib::ControlFlow::Break;
            }
            if frame.height() <= 0 {
                return glib::ControlFlow::Continue;
            }
            if let Some(top) = content_y(&scroll, frame.upcast_ref(), 0.0) {
                let height = if whole { frame.height() as f64 } else { (frame.height() as f64).min(80.0) };
                show_span(&scroll, top, top + height);
            }
            if focus {
                target.grab_focus();
            }
            glib::ControlFlow::Break
        });
    }

    /// Scrolls just enough to show `view`'s cursor (after the edit's layout lands).
    fn keep_cursor_visible(&self, view: &sourceview5::View) {
        let (scroll, view) = (self.scroll.clone(), view.downgrade());
        let started = std::time::Instant::now();
        glib::timeout_add_local(Duration::from_millis(8), move || {
            let Some(view) = view.upgrade() else { return glib::ControlFlow::Break };
            let buffer = view.buffer();
            let r = view.iter_location(&buffer.iter_at_mark(&buffer.get_insert()));
            let (_, y) = view.buffer_to_window_coords(gtk::TextWindowType::Widget, r.x(), r.y());
            let Some(top) = content_y(&scroll, view.upcast_ref(), y as f64) else { return glib::ControlFlow::Break };
            let bottom = top + r.height() as f64;
            let adj = scroll.vadjustment();
            // A new line below the page: wait until the page has grown to hold it.
            if bottom + 12.0 > adj.upper() && started.elapsed() < Duration::from_millis(500) {
                return glib::ControlFlow::Continue;
            }
            show_span(&scroll, top, bottom);
            glib::ControlFlow::Break
        });
    }

    /// Arrow `way` out of cell `i`: into the neighbour (its editor, cursor on the near
    /// line; or its rendered text). False at the notebook's ends.
    fn leave(&self, i: usize, way: Way) -> bool {
        let target = match way {
            Way::Up => i.checked_sub(1),
            Way::Down => Some(i + 1).filter(|&n| n < self.cells.borrow().len()),
        };
        let Some(n) = target else { return false };
        let column = self.cells.borrow().get(i).map_or(0, |c| c.buffer.iter_at_mark(&c.buffer.get_insert()).line_offset());
        self.set_current(n);
        let cells = self.cells.borrow();
        let c = &cells[n];
        if c.rendered.is_visible() {
            c.rendered.grab_focus();
            drop(cells);
            self.reveal(n, false, false);
            return true;
        }
        let line = match way {
            Way::Up => c.buffer.line_count() - 1,
            Way::Down => 0,
        };
        if let Some(mut it) = c.buffer.iter_at_line(line) {
            let mut end = it;
            if !end.ends_line() {
                end.forward_to_line_end();
            }
            it.set_line_offset(column.min(end.line_offset()));
            c.buffer.place_cursor(&it);
        }
        let view = c.view.clone();
        drop(cells);
        view.grab_focus();
        self.keep_cursor_visible(&view);
        true
    }

    /// Shows Markdown cell `i` rendered (empty ones stay editable).
    fn render_markdown(&self, i: usize) {
        let cells = self.cells.borrow();
        let Some(c) = cells.get(i).filter(|c| c.cell.kind == CellKind::Markdown) else { return };
        let source = c.buffer.text(&c.buffer.start_iter(), &c.buffer.end_iter(), false).to_string();
        if source.trim().is_empty() {
            return;
        }
        let markup = tileide_notebook::markdown::to_markup(&source);
        // Markup the renderer got wrong still shows, as plain text. (Links are GtkLabel's
        // own addition: Pango's parser doesn't know `<a>`.)
        if gtk::pango::parse_markup(&without_links(&markup), '\0').is_ok() {
            c.markdown.set_markup(&markup);
        } else {
            c.markdown.set_text(&source);
        }
        let had_focus = c.view.has_focus();
        c.view.set_visible(false);
        c.rendered.set_visible(true);
        if had_focus {
            c.rendered.grab_focus();
        }
    }

    /// Back to editing Markdown cell `i`.
    fn edit_markdown(&self, i: usize) {
        let view = {
            let cells = self.cells.borrow();
            let Some(c) = cells.get(i) else { return };
            c.rendered.set_visible(false);
            c.view.set_visible(true);
            c.view.clone()
        };
        self.set_current(i);
        view.grab_focus();
        self.keep_cursor_visible(&view);
    }

    /// Where "+ Code" / "+ Text" adds: below the current cell, or, when that cell is
    /// scrolled out of sight, below the last cell whose top is in view.
    fn add_anchor(&self) -> usize {
        let cells = self.cells.borrow();
        let adj = self.scroll.vadjustment();
        let Some(child) = self.scroll.child() else { return self.current.get() };
        let span = |c: &CellUi| c.frame.compute_point(&child, &gtk::graphene::Point::new(0.0, 0.0)).map(|p| (p.y() as f64, p.y() as f64 + c.frame.height() as f64));
        let page = adj.page_size();
        let in_view = |(top, bottom): (f64, f64)| bottom > 0.0 && top < page;
        if cells.get(self.current.get()).and_then(span).is_some_and(in_view) {
            return self.current.get();
        }
        cells.iter().rposition(|c| span(c).is_some_and(|(top, _)| (0.0..page).contains(&top))).unwrap_or(self.current.get())
    }

    pub fn add_below(&self, kind: CellKind) {
        let at = (self.add_anchor() + 1).min(self.cells.borrow().len());
        self.insert(at, Cell::new(kind));
        self.attach_lsp(at);
        self.mark_dirty();
        self.focus_cell(at);
    }

    fn swap(&self, a: usize, b: usize) {
        let len = self.cells.borrow().len();
        if a >= len || b >= len || a == b {
            return;
        }
        let (lo, hi) = (a.min(b), a.max(b));
        {
            let cells = self.cells.borrow();
            // Move the lower one's frame after the higher one's.
            self.list.reorder_child_after(&cells[lo].frame, Some(&cells[hi].frame));
        }
        self.cells.borrow_mut().swap(lo, hi);
        self.lsp_resync();
        self.mark_dirty();
        self.focus_cell(b);
    }

    fn toggle_kind(&self, i: usize) {
        let mut cells = self.cells.borrow_mut();
        let Some(ui) = cells.get_mut(i) else { return };
        let kind = if ui.cell.kind == CellKind::Code { CellKind::Markdown } else { CellKind::Code };
        ui.cell.set_kind(kind);
        drop(cells);
        let cells = self.cells.borrow();
        self.style_cell(&cells[i]);
        self.show_outputs(&cells[i]);
        cells[i].rendered.set_visible(false);
        cells[i].view.set_visible(true);
        let buffer = cells[i].buffer.clone();
        drop(cells);
        if kind == CellKind::Code {
            self.attach_lsp(i);
        } else {
            crate::lsp::detach(&buffer);
            self.lsp_resync();
        }
        self.mark_dirty();
    }

    /// Headless checks (`--test-actions cell:<verb>`): `add`, `delete`, `run`, `shift-enter`
    /// (as the key on the current cell), `arrow-up`/`arrow-down` (as the keys at a cell's
    /// edge), `focus-<n>`, `scroll-end`, `state` (printed).
    pub fn test_action(&self, verb: &str) {
        let i = self.current.get();
        match verb {
            "add" => self.add_below(CellKind::Code),
            "delete" => self.delete_current(),
            "shift-enter" => {
                self.run_cell(i);
                self.advance(i);
            }
            "run" => self.run_cell(i),
            // As the keys: only at the editor's first/last line, else nothing.
            "arrow-up" | "arrow-down" => {
                let way = if verb == "arrow-up" { Way::Up } else { Way::Down };
                let view = self.cells.borrow().get(i).map(|c| (c.view.clone(), c.rendered.is_visible()));
                if let Some((view, rendered)) = view
                    && (rendered || at_edge(&view, way))
                {
                    self.leave(i, way);
                }
            }
            "scroll-end" => {
                let adj = self.scroll.vadjustment();
                adj.set_value(adj.upper() - adj.page_size());
            }
            "state" => {
                let adj = self.scroll.vadjustment();
                let focused = self.cells.borrow().iter().position(|c| c.view.has_focus() || c.rendered.has_focus());
                println!("notebook: {} cells, current {i}, focused {focused:?}, scroll {:.0}/{:.0} (page {:.0})", self.cells.borrow().len(), adj.value(), adj.upper(), adj.page_size());
            }
            v if v.starts_with("focus-") => {
                if let Some(n) = v[6..].parse().ok().filter(|&n: &usize| n < self.cells.borrow().len()) {
                    self.focus_cell(n);
                }
            }
            _ => eprintln!("test action: unknown cell verb {verb}"),
        }
    }

    /// The current cell's delete button.
    pub fn delete_current(&self) {
        self.delete(self.current.get());
    }

    fn delete(&self, i: usize) {
        if self.cells.borrow().len() <= 1 {
            // A notebook keeps one cell; deleting the last empties it instead.
            let cells = self.cells.borrow();
            cells[0].buffer.set_text("");
            return;
        }
        let ui = self.cells.borrow_mut().remove(i);
        // Before the view goes: unparents its popovers (see `lsp::release_popovers`).
        crate::lsp::detach(&ui.buffer);
        crate::lsp::release_popovers(ui.view.upcast_ref());
        self.lsp_resync();
        self.list.remove(&ui.frame);
        self.mark_dirty();
        self.focus_cell(i.min(self.cells.borrow().len() - 1));
    }

    pub fn run_all(&self) {
        let count = self.cells.borrow().len();
        for i in 0..count {
            self.run_cell(i);
        }
    }

    /// Sends cell `i` to the kernel (starting it first if needed). Text cells just
    /// count as done.
    pub fn run_cell(&self, i: usize) {
        let (key, code) = {
            let mut cells = self.cells.borrow_mut();
            let Some(ui) = cells.get_mut(i) else { return };
            match ui.cell.kind {
                CellKind::Code => {}
                CellKind::Markdown => {
                    drop(cells);
                    self.render_markdown(i);
                    return;
                }
                CellKind::Raw => return,
            }
            // The old output stays, faded, until the new one replaces it (no jump).
            ui.cell.outputs.clear();
            ui.outputs.add_css_class("stale");
            ui.count.set_text("[*]");
            (ui.key, ui.buffer.text(&ui.buffer.start_iter(), &ui.buffer.end_iter(), false).to_string())
        };
        self.mark_dirty();
        if self.ready.get() {
            if let Some(k) = self.kernel.borrow_mut().as_mut()
                && let Err(e) = k.execute(key, &code)
            {
                self.set_chip(&e, Some("failed"));
            }
        } else {
            self.waiting.borrow_mut().push(key);
            self.start_kernel();
        }
    }

    /// Finds a Python with Jupyter (off the main thread), then starts the kernel.
    fn start_kernel(&self) {
        if self.starting.get() || self.kernel.borrow().is_some() {
            return;
        }
        self.starting.set(true);
        self.set_chip("Kernel: starting…", Some("busy"));
        let candidates = kernel::pythons(self.path.parent().unwrap_or(Path::new(".")), self.folder.as_deref());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let found = candidates.iter().find(|p| kernel::has_jupyter(p)).cloned();
            // A virtualenv was there but lacks Jupyter: the kernel runs elsewhere, without its packages.
            let skipped = found.as_ref().and_then(|f| candidates.iter().take_while(|p| *p != f).find(|p| p.is_absolute() && !p.starts_with("/usr")).cloned());
            let _ = tx.send((found, skipped));
        });
        let me = self.me.borrow().clone();
        glib::timeout_add_local(Duration::from_millis(40), move || {
            let python = match rx.try_recv() {
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Ok((python, skipped)) => {
                    if let Some(venv) = skipped {
                        let using = python.as_ref().map_or_else(String::new, |p| p.display().to_string());
                        crate::alerts::missing(format!("{} has no Jupyter, so the kernel runs on {using}, without that environment's packages. To use it: {} -m pip install ipykernel", venv.display(), venv.display()));
                    }
                    python
                }
                Err(_) => None,
            };
            if let Some(v) = me.upgrade() {
                v.launch(python);
            }
            glib::ControlFlow::Break
        });
    }

    fn launch(&self, python: Option<PathBuf>) {
        self.starting.set(false);
        let Some(python) = python else {
            self.set_chip("Kernel: Jupyter not found", Some("failed"));
            self.fail_waiting(kernel::INSTALL_HINT);
            crate::alerts::missing(kernel::INSTALL_HINT.to_string());
            return;
        };
        *self.python.borrow_mut() = Some(python.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        let script = glib::user_runtime_dir().join("tileide-kernel").join("bridge.py");
        let cwd = self.path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        let kernel_name = self.notebook.borrow().kernel();
        match Kernel::start(&python, &script, &kernel_name, &cwd, move |e| {
            let _ = tx.send(e);
        }) {
            Ok(k) => {
                *self.kernel.borrow_mut() = Some(k);
                *self.events.borrow_mut() = Some(rx);
                self.poll();
            }
            Err(e) => {
                self.set_chip("Kernel: failed", Some("failed"));
                self.fail_waiting(&e);
            }
        }
    }

    /// Waiting cells get `message` as an error output.
    fn fail_waiting(&self, message: &str) {
        let keys: Vec<u64> = self.waiting.borrow_mut().drain(..).collect();
        for key in keys {
            self.apply(Event::Output { cell: key, output: serde_json::json!({"output_type": "error", "ename": "Kernel", "evalue": message, "traceback": [message]}) });
            self.apply(Event::Done { cell: key });
        }
    }

    /// Drains kernel events on the main loop while the kernel runs.
    fn poll(&self) {
        let me = self.me.borrow().clone();
        glib::timeout_add_local(Duration::from_millis(30), move || {
            let Some(v) = me.upgrade() else { return glib::ControlFlow::Break };
            let events: Vec<Event> = match v.events.borrow().as_ref() {
                Some(rx) => rx.try_iter().collect(),
                None => return glib::ControlFlow::Break,
            };
            let mut alive = true;
            for e in events {
                alive &= !matches!(e, Event::Exited | Event::Fatal(_));
                v.apply(e);
            }
            if alive { glib::ControlFlow::Continue } else { glib::ControlFlow::Break }
        });
    }

    fn apply(&self, event: Event) {
        let with_cell = |key: u64, f: &dyn Fn(&mut CellUi)| {
            let mut cells = self.cells.borrow_mut();
            if let Some(ui) = cells.iter_mut().find(|c| c.key == key) {
                f(ui);
            }
        };
        match event {
            Event::Ready { display } => {
                self.ready.set(true);
                self.set_chip(&format!("{display} · idle"), None);
                let waiting: Vec<u64> = self.waiting.borrow_mut().drain(..).collect();
                for key in waiting {
                    let code = self.cells.borrow().iter().find(|c| c.key == key).map(|c| c.buffer.text(&c.buffer.start_iter(), &c.buffer.end_iter(), false).to_string());
                    if let (Some(code), Some(k)) = (code, self.kernel.borrow_mut().as_mut()) {
                        let _ = k.execute(key, &code);
                    }
                }
            }
            Event::Status(state) => {
                let name = self.chip.text().split(" · ").next().unwrap_or("Kernel").to_string();
                self.set_chip(&format!("{name} · {state}"), (state == "busy").then_some("busy"));
            }
            Event::Count { cell, count } => with_cell(cell, &|ui| ui.cell.count = Some(count)),
            Event::Output { cell, output } => {
                if output["ename"] == "ModuleNotFoundError"
                    && let Some(module) = output["evalue"].as_str().and_then(tileide_core::python::missing_module)
                {
                    let python = self.python.borrow().clone().unwrap_or_else(|| PathBuf::from("python3"));
                    crate::alerts::missing(tileide_core::python::install_hint(&module, &python));
                }
                with_cell(cell, &|ui| push_output(&mut ui.cell.outputs, output.clone()));
                if let Some(ui) = self.cells.borrow().iter().find(|c| c.key == cell) {
                    self.show_outputs(ui);
                }
            }
            Event::Clear { cell } => {
                with_cell(cell, &|ui| ui.cell.outputs.clear());
                if let Some(ui) = self.cells.borrow().iter().find(|c| c.key == cell) {
                    self.show_outputs(ui);
                }
            }
            Event::Done { cell } => {
                if let Some(ui) = self.cells.borrow().iter().find(|c| c.key == cell) {
                    // No new output: the old one goes now.
                    if ui.outputs.has_css_class("stale") {
                        self.show_outputs(ui);
                    }
                    self.style_cell(ui);
                }
            }
            Event::Completions { id, matches, start, end } => {
                let reply = self.completions.borrow_mut().remove(&id);
                if let Some(reply) = reply {
                    reply(matches, start, end);
                }
            }
            Event::Restarted => {
                let name = self.chip.text().split(" · ").next().unwrap_or("Kernel").to_string();
                self.set_chip(&format!("{name} · restarted"), None);
                for ui in self.cells.borrow().iter() {
                    self.style_cell(ui);
                }
            }
            Event::Fatal(message) => {
                self.set_chip("Kernel: stopped", Some("failed"));
                self.ready.set(false);
                self.kernel.take();
                // Cells still marked running get the reason.
                let running: Vec<u64> = self.cells.borrow().iter().filter(|c| c.count.text() == "[*]").map(|c| c.key).collect();
                self.waiting.borrow_mut().extend(running);
                self.fail_waiting(&message);
            }
            Event::Exited => {
                if self.ready.get() {
                    self.set_chip("Kernel: stopped", Some("failed"));
                }
                self.ready.set(false);
                self.kernel.take();
            }
        }
    }
}

fn output_label(text: &str, error: bool) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("output");
    if error {
        l.add_css_class("error");
    }
    l.set_xalign(0.0);
    l.set_selectable(true);
    l.set_wrap(true);
    l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    l
}

/// Where focus goes in a cell: its editor, or its rendered Markdown.
fn focus_target(c: &CellUi) -> gtk::Widget {
    if c.rendered.is_visible() { c.rendered.clone().upcast() } else { c.view.clone().upcast() }
}

/// `widget`'s point (0, `y`) in the scrolled content's coordinates (the viewport's
/// own are already scrolled).
fn content_y(scroll: &gtk::ScrolledWindow, widget: &gtk::Widget, y: f64) -> Option<f64> {
    let child = scroll.child()?;
    let p = widget.compute_point(&child, &gtk::graphene::Point::new(0.0, y as f32))?;
    Some(p.y() as f64 + scroll.vadjustment().value())
}

/// Scrolls the least to show `top..bottom` (its top if it is taller than the page).
fn show_span(scroll: &gtk::ScrolledWindow, top: f64, bottom: f64) {
    let adj = scroll.vadjustment();
    let (value, page) = (adj.value(), adj.page_size());
    if top < value {
        adj.set_value((top - 12.0).max(0.0));
    } else if bottom > value + page {
        adj.set_value((bottom - page + 12.0).min(top - 12.0).max(0.0));
    }
}

/// The completion or a pick list is open on `view` (its arrows are for that list).
fn list_open(view: &sourceview5::View) -> bool {
    let mut child = view.first_child();
    while let Some(c) = child {
        if c.is::<gtk::Popover>() && c.is_visible() && c.has_css_class("tileide-completion") {
            return true;
        }
        child = c.next_sibling();
    }
    false
}

/// The cursor is on the first (`Up`) or last (`Down`) display line of `view`.
fn at_edge(view: &sourceview5::View, way: Way) -> bool {
    let buffer = view.buffer();
    let cursor = view.iter_location(&buffer.iter_at_mark(&buffer.get_insert())).y();
    let edge = match way {
        Way::Up => buffer.start_iter(),
        Way::Down => buffer.end_iter(),
    };
    cursor == view.iter_location(&edge).y()
}

/// `markup` with `<a href="…">` / `</a>` tags taken out (their text kept).
fn without_links(markup: &str) -> String {
    let mut out = String::with_capacity(markup.len());
    let mut rest = markup;
    while let Some(i) = rest.find("<a href=\"") {
        out.push_str(&rest[..i]);
        rest = rest[i..].find("\">").map_or("", |j| &rest[i + j + 2..]);
    }
    out.push_str(rest);
    out.replace("</a>", "")
}

/// An HTML table output (a DataFrame) as a grid: header rows, the index in bold, zebra
/// rows; wide tables scroll sideways. pandas' "N rows × M columns" goes under it.
fn table_widget(table: &tileide_notebook::table::Table) -> gtk::Widget {
    let grid = gtk::Grid::new();
    grid.add_css_class("frame-table");
    grid.set_halign(gtk::Align::Start);
    let rows = table.head.iter().map(|r| (r, true)).chain(table.body.iter().map(|r| (r, false)));
    for (y, (row, head)) in rows.enumerate() {
        let odd = !head && (y - table.head.len()) % 2 == 1;
        for (x, cell) in row.iter().enumerate() {
            let label = gtk::Label::new(Some(&cell.text));
            label.set_xalign(if cell.header && !head { 0.0 } else { 1.0 });
            label.set_selectable(true);
            label.set_can_focus(false);
            for (on, class) in [(cell.header || head, "th"), (head, "head"), (odd, "odd")] {
                if on {
                    label.add_css_class(class);
                }
            }
            grid.attach(&label, x as i32, y as i32, 1, 1);
        }
    }
    let scroll = gtk::ScrolledWindow::new();
    // Pandas already shortens long frames; only width scrolls (the page keeps the wheel).
    scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
    scroll.set_propagate_natural_height(true);
    scroll.set_child(Some(&grid));
    let column = gtk::Box::new(gtk::Orientation::Vertical, 2);
    column.append(&scroll);
    if let Some(note) = &table.note {
        let label = gtk::Label::new(Some(note));
        label.add_css_class("frame-table-note");
        label.set_xalign(0.0);
        column.append(&label);
    }
    column.upcast()
}
