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
.notebook .outputs {{ border-top: 1px solid {edge}; margin-top: 4px; padding-top: 4px; }}",
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
    outputs: gtk::Box,
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
    next_key: StdCell<u64>,
    folder: Option<PathBuf>,
    theme: RefCell<Rc<Theme>>,
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
    pub fn open(path: &Path, theme: Rc<Theme>, folder: Option<PathBuf>) -> Rc<NotebookView> {
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
            next_key: StdCell::new(1),
            folder,
            theme: RefCell::new(theme),
            me: RefCell::new(Weak::new()),
        });
        *view.me.borrow_mut() = Rc::downgrade(&view);
        let cells = std::mem::take(&mut view.notebook.borrow_mut().cells);
        for (i, cell) in cells.into_iter().enumerate() {
            view.insert(i, cell);
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

    /// Where keyboard focus goes when the tile is focused.
    pub fn focus(&self) {
        if let Some(c) = self.cells.borrow().get(self.current.get()) {
            c.view.grab_focus();
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
        let outputs = gtk::Box::new(gtk::Orientation::Vertical, 2);
        outputs.add_css_class("outputs");
        frame.append(&outputs);

        let ui = CellUi { key, cell, frame: frame.clone(), count, view: view.clone(), buffer: buffer.clone(), outputs };
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
        ui.outputs.set_visible(!ui.cell.outputs.is_empty());
    }

    fn show_outputs(&self, ui: &CellUi) {
        while let Some(child) = ui.outputs.first_child() {
            ui.outputs.remove(&child);
        }
        for output in &ui.cell.outputs {
            let widget: gtk::Widget = match shown(output) {
                Some(Shown::Text(text)) | Some(Shown::Error(text)) if text.is_empty() => continue,
                Some(Shown::Text(text)) => output_label(text.trim_end_matches('\n'), false).upcast(),
                Some(Shown::Error(text)) => output_label(&text, true).upcast(),
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

    /// Shift+Enter: focus the next cell, adding a code cell at the end.
    fn advance(&self, i: usize) {
        if i + 1 >= self.cells.borrow().len() {
            self.insert(i + 1, Cell::new(CellKind::Code));
            self.mark_dirty();
        }
        self.set_current(i + 1);
        let view = self.cells.borrow()[i + 1].view.clone();
        view.grab_focus();
        let frame = self.cells.borrow()[i + 1].frame.clone();
        let scroll = self.scroll.clone();
        glib::idle_add_local_once(move || {
            if let Some(child) = scroll.child()
                && let Some(p) = frame.compute_point(&child, &gtk::graphene::Point::new(0.0, 0.0))
            {
                let adj = scroll.vadjustment();
                let (top, bottom) = (p.y() as f64, p.y() as f64 + frame.height() as f64);
                if bottom > adj.value() + adj.page_size() || top < adj.value() {
                    adj.set_value((bottom - adj.page_size() + 12.0).max(top - 12.0).max(0.0));
                }
            }
        });
    }
}

impl NotebookView {
    fn add_below(&self, kind: CellKind) {
        let at = (self.current.get() + 1).min(self.cells.borrow().len());
        self.insert(at, Cell::new(kind));
        self.mark_dirty();
        self.set_current(at);
        let view = self.cells.borrow()[at].view.clone();
        view.grab_focus();
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
        self.mark_dirty();
        self.set_current(b);
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
        drop(cells);
        self.mark_dirty();
    }

    fn delete(&self, i: usize) {
        if self.cells.borrow().len() <= 1 {
            // A notebook keeps one cell; deleting the last empties it instead.
            let cells = self.cells.borrow();
            cells[0].buffer.set_text("");
            return;
        }
        let ui = self.cells.borrow_mut().remove(i);
        self.list.remove(&ui.frame);
        self.mark_dirty();
        self.set_current(i.min(self.cells.borrow().len() - 1));
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
            if ui.cell.kind != CellKind::Code {
                return;
            }
            ui.cell.outputs.clear();
            ui.count.set_text("[*]");
            (ui.key, ui.buffer.text(&ui.buffer.start_iter(), &ui.buffer.end_iter(), false).to_string())
        };
        {
            let cells = self.cells.borrow();
            self.show_outputs(&cells[i]);
        }
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
        let candidates = kernel::pythons(self.folder.as_deref().or(self.path.parent()));
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(candidates.into_iter().find(|p| kernel::has_jupyter(p)));
        });
        let me = self.me.borrow().clone();
        glib::timeout_add_local(Duration::from_millis(40), move || {
            let python = match rx.try_recv() {
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Ok(python) => python,
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
            return;
        };
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
                    self.style_cell(ui);
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
