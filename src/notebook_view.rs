// SPDX-License-Identifier: GPL-3.0-or-later
//! A Jupyter notebook as a tile (v0.4 phase 4): a toolbar (kernel state, run all,
//! interrupt, restart, add cells, a menu with run above/below and clearing outputs),
//! then the cells, each with its own editor and its outputs underneath (text, errors,
//! images) and how long its last run took. `Shift+Enter` runs a cell and moves on,
//! `Ctrl+Enter` runs it in place, `Alt+R` runs every cell. The kernel starts on the
//! first run, through `tileide_notebook::kernel` and the user's Jupyter.
//!
//! Command mode (v0.5 phase 6), as in JupyterLab: `Esc` leaves the editor and the cell
//! itself takes the keys: `↑`/`↓` (`K`/`J`) select, `Shift+↑/↓` select several, `Enter`
//! edits, `A`/`B` add above/below, `DD` deletes, `Z` brings it back, `M`/`Y` make it
//! text/code, `C`/`X`/`V` copy/cut/paste (`Shift+V` above), `II` interrupts.

use std::cell::{Cell as StdCell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gtk::{gdk, glib};
use gtk4 as gtk;
use sourceview5::prelude::*;
use tileide_core::theme::{self, Theme};
use tileide_notebook::kernel::{self, Event, Kernel};
use tileide_lsp::protocol::Position;
use tileide_lsp::text::Encoding;
use tileide_notebook::{Cell, CellKind, Frame, Notebook, Shown, frame, push_output, shown};

pub fn css(t: &Theme) -> String {
    let c = theme::css;
    format!(
        ".notebook .bar {{ padding: 10px 14px 6px 16px; }}
.notebook button.chip {{ padding: 0; min-height: 0; border: none; box-shadow: none; background: transparent; }}
.notebook button.chip:hover .kernel {{ background-color: {hover}; color: {text}; }}
.notebook popover.kernels button {{ padding: 4px 12px; }}
.notebook popover.kernels .detail {{ color: {muted}; font-size: 12px; padding: 2px 12px; }}
.notebook .kernel {{ font-size: 12px; border-radius: 10px; padding: 2px 10px; background-color: {chip}; color: {muted}; }}
.notebook .kernel.busy {{ background-color: {accent_bg}; color: {accent}; }}
.notebook .kernel.failed {{ background-color: {error_bg}; color: {error}; }}
.notebook button.pill {{ min-height: 24px; padding: 0 10px; border-radius: 12px; border: 1px solid {edge}; background: transparent; box-shadow: none; color: {text}; font-size: 12px; }}
.notebook button.pill:hover, .notebook button.icon:hover, .notebook menubutton.icon > button:hover {{ background-color: {hover}; }}
.notebook button.icon, .notebook menubutton.icon > button {{ min-width: 24px; min-height: 24px; padding: 0; border: none; box-shadow: none; background: transparent; color: {muted}; border-radius: 12px; }}
.notebook .cell {{ border-radius: 12px; border: 1px solid transparent; background-color: {cell}; padding: 4px 6px 6px 6px; }}
.notebook .cell.current {{ border-color: {accent_edge}; }}
.notebook .cell:focus, .notebook .cell:focus-visible {{ outline: none; }}
.notebook .cell.current:focus {{ border-color: {accent}; }}
.notebook .cell.selected {{ background-color: {accent_bg}; }}
.notebook popover.more button {{ padding: 4px 12px; }}
.notebook .cell .count {{ font-family: monospace; font-size: 12px; color: {muted}; padding-left: 6px; }}
.notebook .cell textview, .notebook .cell textview text {{ background: transparent; }}
.notebook .output {{ color: {text}; padding: 4px 10px; }}
.notebook .output.error {{ color: {error}; }}
.notebook .output link, .notebook .frame-table link {{ color: {accent}; }}
.notebook button.fold {{ min-height: 22px; padding: 0 10px; margin: 2px 10px; border-radius: 11px; border: 1px solid {edge}; background: transparent; box-shadow: none; color: {muted}; font-size: 12px; }}
.notebook .outputs {{ border-top: 1px solid {edge}; margin-top: 4px; padding-top: 4px; }}
.notebook .outputs.stale {{ opacity: 0.45; }}
.notebook .frame-table {{ margin: 4px 10px; }}
.notebook .frame-table label {{ padding: 3px 10px; color: {text}; }}
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
    /// When the kernel began the current run, and how long the last one took.
    started: Option<Instant>,
    took: Option<Duration>,
}

/// A toolbar menu entry's action.
type Action = fn(&NotebookView);

/// Opens a widget in a tile beside the notebook (the variable view, a table).
pub type OpenPanel = Rc<dyn Fn(gtk::Widget)>;

/// What to do with a silent evaluation's result.
type EvalReply = Box<dyn FnOnce(&NotebookView, Result<String, String>)>;

/// Text outputs longer than this many lines show their first `FOLD_SHOWN` lines and a
/// button for the rest.
const FOLD_LINES: usize = 60;
const FOLD_SHOWN: usize = 30;

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
    /// The chip as a button: picks the Python or kernel (`pick_kernel`).
    chip_button: gtk::Button,
    /// The open picker, closed with the notebook (a popover left on a removed widget
    /// crashes GTK).
    kernel_popover: RefCell<Option<gtk::Popover>>,
    path: PathBuf,
    notebook: RefCell<Notebook>,
    cells: RefCell<Vec<CellUi>>,
    current: StdCell<usize>,
    /// The other end of a Shift+↑/↓ selection (`None`: just the current cell).
    anchor: StdCell<Option<usize>>,
    /// Command mode: the cell, not its editor, has the keys.
    command: StdCell<bool>,
    /// The last command key and when, for `DD` and `II`.
    last_key: StdCell<Option<(gdk::Key, Instant)>>,
    /// Cells copied or cut (`C`/`X`), pasted with `V`.
    clipboard: RefCell<Vec<Cell>>,
    /// Deleted cells and where they were, newest last (`Z`).
    deleted: RefCell<Vec<(usize, Vec<Cell>)>>,
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
    /// Silent evaluations in flight, by id (ids shared with completions).
    evals: RefCell<std::collections::HashMap<u64, EvalReply>>,
    variables: RefCell<Option<Rc<crate::variables_view::VariablesView>>>,
    open_panel: OpenPanel,
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
    pub fn open(path: &Path, theme: Rc<Theme>, folder: Option<PathBuf>, goto: crate::lsp::Goto, open_panel: OpenPanel) -> Rc<NotebookView> {
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
        chip.set_ellipsize(gtk::pango::EllipsizeMode::End);
        chip.set_max_width_chars(34);
        let chip_button = gtk::Button::new();
        chip_button.add_css_class("chip");
        chip_button.set_child(Some(&chip));
        chip_button.set_valign(gtk::Align::Center);
        chip_button.set_tooltip_text(Some("Pick the Python or kernel (Alt+P)"));
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        let (run_all, interrupt, restart, add_code, add_md) = (
            pill("Run all", "Run every cell (Alt+R)"),
            pill("Interrupt", "Stop the running cell"),
            pill("Restart", "Restart the kernel (variables are lost)"),
            pill("+ Code", "Add a code cell below the current one"),
            pill("+ Text", "Add a Markdown cell below the current one"),
        );
        let more_list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let more_items = [
            ("Run cells above", NotebookView::run_above as Action),
            ("Run this cell and below", NotebookView::run_below),
            ("Clear this cell's output", NotebookView::clear_selected),
            ("Clear all outputs", NotebookView::clear_all),
            ("Show variables", NotebookView::show_variables),
        ];
        let more_buttons: Vec<(gtk::Button, Action)> = more_items
            .into_iter()
            .map(|(text, f)| {
                let b = gtk::Button::with_label(text);
                b.add_css_class("flat");
                if let Some(l) = b.child().and_downcast::<gtk::Label>() {
                    l.set_xalign(0.0);
                }
                more_list.append(&b);
                (b, f)
            })
            .collect();
        let more_popover = gtk::Popover::new();
        more_popover.add_css_class("more");
        more_popover.set_child(Some(&more_list));
        let more = gtk::MenuButton::new();
        more.set_icon_name("view-more-symbolic");
        more.add_css_class("icon");
        more.set_tooltip_text(Some("Run above or below, clear outputs, variables"));
        more.set_valign(gtk::Align::Center);
        more.set_popover(Some(&more_popover));
        bar.append(&chip_button);
        bar.append(&spacer);
        for b in [&run_all, &interrupt, &restart, &add_code, &add_md] {
            bar.append(b);
        }
        bar.append(&more);
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
            chip_button,
            kernel_popover: RefCell::new(None),
            path: path.to_path_buf(),
            notebook: RefCell::new(notebook),
            cells: RefCell::new(Vec::new()),
            current: StdCell::new(0),
            anchor: StdCell::new(None),
            command: StdCell::new(false),
            last_key: StdCell::new(None),
            clipboard: RefCell::new(Vec::new()),
            deleted: RefCell::new(Vec::new()),
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
            evals: RefCell::new(std::collections::HashMap::new()),
            variables: RefCell::new(None),
            open_panel,
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
        view.chip_button.connect_clicked(on(NotebookView::pick_kernel));
        interrupt.connect_clicked(on(|v| v.with_kernel(Kernel::interrupt)));
        restart.connect_clicked(on(|v| {
            v.with_kernel(Kernel::restart);
            v.waiting.borrow_mut().clear();
        }));
        add_code.connect_clicked(on(|v| v.add_below(CellKind::Code)));
        add_md.connect_clicked(on(|v| v.add_below(CellKind::Markdown)));
        for (button, f) in more_buttons {
            let (me, popover) = (Rc::downgrade(&view), more_popover.downgrade());
            button.connect_clicked(move |_| {
                if let Some(p) = popover.upgrade() {
                    p.popdown();
                }
                if let Some(v) = me.upgrade() {
                    f(&v);
                }
            });
        }
        // Command mode: keys that reach the list while no editor has the focus.
        let keys = gtk::EventControllerKey::new();
        let me = Rc::downgrade(&view);
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(v) = me.upgrade() else { return glib::Propagation::Proceed };
            if !v.command.get() || v.editor_has_focus() {
                return glib::Propagation::Proceed;
            }
            if v.command_key(key, mods) { glib::Propagation::Stop } else { glib::Propagation::Proceed }
        });
        view.list.add_controller(keys);
        view.set_current(0);
        view
    }

    fn editor_has_focus(&self) -> bool {
        self.cells.borrow().iter().any(|c| c.view.has_focus())
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
            self.target(c).grab_focus();
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
        nb.cells = (0..self.cells.borrow().len()).filter_map(|i| self.snapshot(i)).collect();
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
        self.close_kernel_popover();
        if let Some(k) = self.kernel.take() {
            k.shutdown();
        }
        self.events.take();
        // Also unparents the cells' popovers (a view disposed with one would hang GTK).
        for buffer in self.code_buffers() {
            crate::lsp::detach(&buffer);
        }
    }

    /// Evaluates `expr` silently in the kernel; `then` gets the result (an error if
    /// the kernel isn't running).
    fn eval(&self, expr: &str, then: impl FnOnce(&NotebookView, Result<String, String>) + 'static) {
        if !self.ready.get() {
            then(self, Err("The kernel isn't running: run a cell first.".into()));
            return;
        }
        let id = self.next_completion.get();
        self.next_completion.set(id + 1);
        let sent = self.kernel.borrow_mut().as_mut().map(|k| k.eval(id, expr));
        match sent {
            Some(Ok(())) => {
                self.evals.borrow_mut().insert(id, Box::new(then));
            }
            Some(Err(e)) => then(self, Err(e)),
            None => then(self, Err("The kernel isn't running: run a cell first.".into())),
        }
    }

    /// The toolbar menu's "Show variables": the variable view in a tile beside this one.
    pub fn show_variables(&self) {
        let existing = self.variables.borrow().clone();
        let view = existing.unwrap_or_else(|| {
            let name = self.path.file_name().map_or_else(|| "notebook".into(), |n| n.to_string_lossy().into_owned());
            let (me, me2) = (self.me.borrow().clone(), self.me.borrow().clone());
            let view = crate::variables_view::VariablesView::new(
                &name,
                Rc::new(move |var| {
                    if let Some(v) = me.upgrade() {
                        v.open_table(&var);
                    }
                }),
                move || {
                    if let Some(v) = me2.upgrade() {
                        v.refresh_variables(true);
                    }
                },
            );
            *self.variables.borrow_mut() = Some(view.clone());
            view
        });
        if !view.is_shown() {
            (self.open_panel)(view.widget().clone().upcast());
        }
        self.refresh_variables(true);
    }

    /// Asks the kernel for its variables, if the view is open (or `force`).
    fn refresh_variables(&self, force: bool) {
        let Some(view) = self.variables.borrow().clone() else { return };
        if !force && !view.is_shown() {
            return;
        }
        if self.notebook.borrow().language() != "python3" {
            view.message("Variables are listed for Python kernels.");
            return;
        }
        self.eval(crate::variables_view::LIST, move |_, result| match result.and_then(|json| crate::variables_view::parse(&json)) {
            Ok(vars) => view.set(&vars),
            Err(e) => view.message(&e),
        });
    }

    /// A DataFrame (or anything with `to_html`) from the kernel, whole, in its own tile.
    fn open_table(&self, name: &str) {
        let name = name.to_string();
        self.eval(&crate::variables_view::table_expr(&name), move |v, result| {
            let table = result.and_then(|html| tileide_notebook::table::parse(&html).ok_or_else(|| format!("{name} didn't come back as a table")));
            match table {
                Ok(table) => {
                    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
                    root.add_css_class("notebook");
                    root.set_widget_name(&name);
                    root.set_focusable(true);
                    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                    bar.add_css_class("bar");
                    let size = table.note.clone().unwrap_or_else(|| format!("{} rows", table.body.len()));
                    let title = gtk::Label::new(Some(&format!("{name} · {size}")));
                    title.add_css_class("kernel");
                    bar.append(&title);
                    root.append(&bar);
                    let scroll = gtk::ScrolledWindow::new();
                    scroll.set_vexpand(true);
                    scroll.set_child(Some(&table_widget(&table)));
                    root.append(&scroll);
                    (v.open_panel)(root.upcast());
                }
                Err(e) => {
                    if let Some(view) = v.variables.borrow().as_ref() {
                        view.message(&e);
                    }
                }
            }
        });
    }

    /// The Pythons and kernels this notebook can run on, in a popover under the chip.
    pub fn pick_kernel(&self) {
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let looking = gtk::Label::new(Some("Looking for Pythons…"));
        looking.add_css_class("detail");
        list.append(&looking);
        let popover = gtk::Popover::new();
        popover.add_css_class("kernels");
        popover.set_child(Some(&list));
        self.close_kernel_popover();
        popover.set_parent(&self.chip_button);
        popover.connect_closed(|p| {
            let p = p.clone();
            // After the click that closed it is handled.
            glib::idle_add_local_once(move || {
                if p.parent().is_some() {
                    p.unparent();
                }
            });
        });
        popover.popup();
        *self.kernel_popover.borrow_mut() = Some(popover.clone());
        let me = self.me.borrow().clone();
        let python = self.python.borrow().clone();
        let kernel = self.notebook.borrow().kernel();
        crate::interpreter::gather(&self.path, self.folder.as_deref(), python.as_deref(), Some(&kernel), move |rows| {
            looking.set_text("Python for this project, or a Jupyter kernel");
            for row in rows {
                let b = gtk::Button::with_label(&crate::interpreter::label(&row));
                b.add_css_class("flat");
                if let Some(l) = b.child().and_downcast::<gtk::Label>() {
                    l.set_xalign(0.0);
                }
                let (me, popover) = (me.clone(), popover.downgrade());
                b.connect_clicked(move |_| {
                    if let Some(p) = popover.upgrade() {
                        p.popdown();
                    }
                    if let Some(v) = me.upgrade() {
                        v.picked(&row);
                    }
                });
                list.append(&b);
            }
        });
    }

    fn close_kernel_popover(&self) {
        if let Some(p) = self.kernel_popover.take()
            && p.parent().is_some()
        {
            p.popdown();
            p.unparent();
        }
    }

    fn picked(&self, row: &crate::interpreter::Row) {
        use crate::interpreter::Pick;
        match &row.pick {
            Pick::Python(python) => {
                crate::interpreter::use_python(&self.path, self.folder.as_deref(), python);
                if self.notebook.borrow().kernel() != "python3" {
                    self.notebook.borrow_mut().set_kernel("python3", "Python 3 (ipykernel)", "python");
                    self.mark_dirty();
                }
                self.switch_kernel();
            }
            Pick::Kernel(spec) => {
                self.notebook.borrow_mut().set_kernel(&spec.name, &spec.display, &spec.language);
                self.mark_dirty();
                self.switch_kernel();
            }
            Pick::Setup { command, dir, python } => {
                crate::interpreter::setup(&self.path, self.folder.as_deref(), command, dir, python);
                self.set_chip("Kernel: restart it once the install is done", None);
            }
        }
    }

    /// Stops the kernel, if any, and starts the one now asked for (variables are lost).
    fn switch_kernel(&self) {
        if let Some(k) = self.kernel.take() {
            k.shutdown();
        }
        self.events.take();
        self.ready.set(false);
        self.waiting.borrow_mut().clear();
        *self.python.borrow_mut() = None;
        self.start_kernel();
    }

    /// The chip's text without its state ("Python 3 (ipykernel) · .venv").
    fn chip_name(&self) -> String {
        let text = self.chip.text();
        text.rsplit_once(" · ").map_or(text.as_str(), |(name, _)| name).to_string()
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
        // Command mode focuses the cell itself; a click still goes to what's under it.
        frame.set_focusable(true);
        frame.set_focus_on_click(false);
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
        crate::editing::attach(&view);
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

        let ui = CellUi { key, cell, frame: frame.clone(), count, view: view.clone(), buffer: buffer.clone(), rendered: rendered.clone(), markdown, outputs, started: None, took: None };
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
        // Editing mode in the editor; a rendered text cell is read in command mode.
        for (widget, command) in [(view.upcast_ref::<gtk::Widget>(), false), (rendered.upcast_ref(), true)] {
            let me = self.me.borrow().clone();
            let focus = gtk::EventControllerFocus::new();
            focus.connect_enter(move |_| {
                if let Some(v) = me.upgrade()
                    && let Some(i) = v.index(key)
                {
                    v.command.set(command);
                    if !(command && v.anchor.get().is_some()) {
                        v.set_current(i);
                    }
                }
            });
            widget.add_controller(focus);
        }
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
                gdk::Key::BackSpace if (mods & gtk::accelerator_get_default_mod_mask()).is_empty() => {
                    let Some(i) = v.index(key) else { return glib::Propagation::Proceed };
                    return if v.backspace_empty(i) { glib::Propagation::Stop } else { glib::Propagation::Proceed };
                }
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
        // A rendered Markdown cell takes command keys (arrows move on, Enter edits it);
        // a double click edits it too.
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
            if k == gdk::Key::Escape && (mods & gtk::accelerator_get_default_mod_mask()).is_empty() {
                let Some(i) = v.index(key) else { return glib::Propagation::Proceed };
                v.enter_command(i);
                return glib::Propagation::Stop;
            }
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
        let count = match (ui.cell.kind, ui.cell.count) {
            (CellKind::Code, Some(n)) => format!("[{n}]"),
            (CellKind::Code, None) => "[ ]".into(),
            (CellKind::Markdown, _) => "Text".into(),
            (CellKind::Raw, _) => "Raw".into(),
        };
        ui.count.set_text(&match ui.took.filter(|_| ui.cell.kind == CellKind::Code) {
            Some(took) => format!("{count}  {}", run_time(took)),
            None => count,
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
                Some(Shown::Text(text)) => folded(text.trim_end_matches('\n')),
                Some(Shown::Error(text)) => self.error_output(&text).upcast(),
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

    /// A traceback whose frame lines (`File …:12`, `Cell In[3], line 5`) are links.
    fn error_output(&self, text: &str) -> gtk::Label {
        let mut markup = String::new();
        let mut frames = Vec::new();
        for (n, line) in text.lines().enumerate() {
            if n > 0 {
                markup.push('\n');
            }
            match frame(line) {
                Some((span, f)) => {
                    markup += &glib::markup_escape_text(&line[..span.start]);
                    markup += &format!("<a href=\"frame:{}\">{}</a>", frames.len(), glib::markup_escape_text(&line[span.clone()]));
                    markup += &glib::markup_escape_text(&line[span.end..]);
                    frames.push(f);
                }
                None => markup += &glib::markup_escape_text(line),
            }
        }
        let label = output_label("", true);
        label.set_markup(&markup);
        let me = self.me.borrow().clone();
        label.connect_activate_link(move |_, uri| {
            let f = uri.strip_prefix("frame:").and_then(|i| i.parse::<usize>().ok()).and_then(|i| frames.get(i));
            if let (Some(v), Some(f)) = (me.upgrade(), f) {
                v.open_frame(f);
            }
            glib::Propagation::Stop
        });
        label
    }

    /// Shows where a traceback frame points: a file's line in its tile, or a cell's line.
    fn open_frame(&self, f: &Frame) {
        match f {
            Frame::File(path, line) => {
                let path = crate::workspaces::expand_home(path);
                // `<frozen …>` and the like are no files.
                if path.is_file() {
                    (self.goto)(path, Position { line: line.saturating_sub(1), column: 0 }, Encoding::Utf16);
                }
            }
            Frame::Cell(count, line) => {
                let found = self.cells.borrow().iter().position(|c| c.cell.kind == CellKind::Code && c.cell.count == Some(*count));
                let Some(i) = found else {
                    println!("notebook: no cell ran as [{count}] any more");
                    return;
                };
                if let Some(c) = self.cells.borrow().get(i)
                    && let Some(at) = c.buffer.iter_at_line(*line as i32 - 1)
                {
                    c.buffer.place_cursor(&at);
                }
                self.edit(i);
            }
        }
    }

    /// Makes cell `i` current and the only one selected.
    fn set_current(&self, i: usize) {
        self.anchor.set(None);
        self.paint(i);
    }

    /// Cell `i` current; the cells between it and `anchor` selected.
    fn paint(&self, i: usize) {
        let cells = self.cells.borrow();
        let i = i.min(cells.len().saturating_sub(1));
        self.current.set(i);
        let selected = self.selection_of(i, cells.len());
        for (n, c) in cells.iter().enumerate() {
            if n == i {
                c.frame.add_css_class("current");
            } else {
                c.frame.remove_css_class("current");
            }
            if selected.contains(&n) && selected.start() != selected.end() {
                c.frame.add_css_class("selected");
            } else {
                c.frame.remove_css_class("selected");
            }
        }
    }

    fn selection_of(&self, current: usize, len: usize) -> std::ops::RangeInclusive<usize> {
        let anchor = self.anchor.get().unwrap_or(current).min(len.saturating_sub(1));
        anchor.min(current)..=anchor.max(current)
    }

    /// The selected cells (at least the current one).
    fn selection(&self) -> std::ops::RangeInclusive<usize> {
        self.selection_of(self.current.get(), self.cells.borrow().len())
    }

    /// `Esc` in cell `i`'s editor: the cell takes the keys.
    fn enter_command(&self, i: usize) {
        self.command.set(true);
        self.set_current(i);
        if let Some(frame) = self.cells.borrow().get(i).map(|c| c.frame.clone()) {
            frame.grab_focus();
        }
    }

    /// What gets the focus in cell `c`: the cell itself in command mode.
    fn target(&self, c: &CellUi) -> gtk::Widget {
        if self.command.get() { c.frame.clone().upcast() } else { focus_target(c) }
    }

    /// A key in command mode; true when it was one.
    fn command_key(&self, key: gdk::Key, mods: gdk::ModifierType) -> bool {
        let mods = mods & gtk::accelerator_get_default_mod_mask();
        let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
        if mods.intersects(gdk::ModifierType::ALT_MASK | gdk::ModifierType::SUPER_MASK) {
            return false;
        }
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let key = key.to_lower();
        // `DD`, `II`: the same key twice within a moment.
        let twice = |k: gdk::Key| {
            let again = self.last_key.get().is_some_and(|(last, at)| last == k && at.elapsed() < Duration::from_millis(800));
            self.last_key.set(if again { None } else { Some((k, Instant::now())) });
            again
        };
        let i = self.current.get();
        match key {
            gdk::Key::Return | gdk::Key::KP_Enter if shift => {
                let selection = self.selection();
                self.run_cells(selection.clone());
                self.advance(*selection.end());
            }
            gdk::Key::Return | gdk::Key::KP_Enter if ctrl => self.run_cells(self.selection()),
            gdk::Key::Return | gdk::Key::KP_Enter => self.edit(i),
            _ if ctrl => return false,
            gdk::Key::Up | gdk::Key::KP_Up | gdk::Key::k => self.select(Way::Up, shift),
            gdk::Key::Down | gdk::Key::KP_Down | gdk::Key::j => self.select(Way::Down, shift),
            gdk::Key::a => self.add_at(*self.selection().start(), CellKind::Code),
            gdk::Key::b => self.add_at(*self.selection().end() + 1, CellKind::Code),
            gdk::Key::d => {
                if twice(key) {
                    self.delete_selected();
                }
            }
            gdk::Key::i => {
                if twice(key) {
                    self.with_kernel(Kernel::interrupt);
                }
            }
            gdk::Key::z => self.undo_delete(),
            gdk::Key::m => self.set_kind_selected(CellKind::Markdown),
            gdk::Key::y => self.set_kind_selected(CellKind::Code),
            gdk::Key::c => self.copy_selected(),
            gdk::Key::x => {
                self.copy_selected();
                self.delete_selected();
            }
            gdk::Key::v => self.paste(if shift { *self.selection().start() } else { *self.selection().end() + 1 }),
            gdk::Key::Escape => {}
            _ => return false,
        }
        true
    }

    /// `↑`/`↓` in command mode: the next cell becomes current (`extend`: selected too).
    fn select(&self, way: Way, extend: bool) {
        let len = self.cells.borrow().len();
        let i = self.current.get();
        let n = match way {
            Way::Up => i.saturating_sub(1),
            Way::Down => (i + 1).min(len.saturating_sub(1)),
        };
        if extend {
            if self.anchor.get().is_none() {
                self.anchor.set(Some(i));
            }
            self.paint(n);
        } else {
            self.set_current(n);
        }
        self.reveal(n, true, false);
    }

    /// `Enter` in command mode: edit cell `i`.
    fn edit(&self, i: usize) {
        self.command.set(false);
        let rendered = self.cells.borrow().get(i).is_some_and(|c| c.rendered.is_visible());
        if rendered {
            self.edit_markdown(i);
        } else {
            self.focus_cell(i);
        }
    }

    /// A copy of cell `i` as it stands (its editor's text).
    fn snapshot(&self, i: usize) -> Option<Cell> {
        let cells = self.cells.borrow();
        let c = cells.get(i)?;
        let mut cell = c.cell.clone();
        cell.source = c.buffer.text(&c.buffer.start_iter(), &c.buffer.end_iter(), false).to_string();
        Some(cell)
    }

    fn add_at(&self, at: usize, kind: CellKind) {
        let at = at.min(self.cells.borrow().len());
        self.insert(at, Cell::new(kind));
        self.attach_lsp(at);
        self.mark_dirty();
        self.focus_cell(at);
    }

    /// `DD`: the selected cells go (`Z` brings them back). A notebook keeps one cell.
    fn delete_selected(&self) {
        let selection = self.selection();
        let taken: Vec<Cell> = selection.clone().filter_map(|i| self.snapshot(i)).collect();
        self.deleted.borrow_mut().push((*selection.start(), taken));
        for i in selection.clone().rev() {
            if self.cells.borrow().len() == 1 {
                self.insert(1, Cell::new(CellKind::Code));
                self.attach_lsp(1);
            }
            self.remove(i);
        }
        self.focus_cell((*selection.start()).min(self.cells.borrow().len() - 1));
    }

    /// `Z`: the last deleted cells come back where they were.
    fn undo_delete(&self) {
        let Some((at, cells)) = self.deleted.borrow_mut().pop() else { return };
        let at = at.min(self.cells.borrow().len());
        let count = cells.len();
        for (n, cell) in cells.into_iter().enumerate() {
            self.insert(at + n, cell);
            self.attach_lsp(at + n);
            self.render_markdown(at + n);
        }
        self.mark_dirty();
        self.focus_cell(at);
        if count > 1 {
            self.anchor.set(Some(at + count - 1));
            self.paint(at);
        }
    }

    fn copy_selected(&self) {
        *self.clipboard.borrow_mut() = self.selection().filter_map(|i| self.snapshot(i)).collect();
    }

    /// `V`: the copied cells, each with its own id, from `at` on.
    fn paste(&self, at: usize) {
        let cells: Vec<Cell> = self.clipboard.borrow().iter().map(Cell::duplicate).collect();
        if cells.is_empty() {
            return;
        }
        let at = at.min(self.cells.borrow().len());
        for (n, cell) in cells.into_iter().enumerate() {
            self.insert(at + n, cell);
            self.attach_lsp(at + n);
            self.render_markdown(at + n);
        }
        self.mark_dirty();
        self.focus_cell(at);
    }

    /// `M`/`Y`: the selected cells become text or code.
    fn set_kind_selected(&self, kind: CellKind) {
        let selection = self.selection();
        let current = self.current.get();
        for i in selection.clone() {
            if self.cells.borrow().get(i).is_some_and(|c| c.cell.kind != kind) {
                self.toggle_kind(i);
            }
        }
        self.paint(current);
        if let Some(frame) = self.cells.borrow().get(current).map(|c| c.frame.clone()) {
            frame.grab_focus();
        }
    }

    fn run_cells(&self, range: impl Iterator<Item = usize>) {
        for i in range {
            self.run_cell(i);
        }
    }

    fn run_above(&self) {
        self.run_cells(0..self.current.get());
    }

    fn run_below(&self) {
        // Not inline: the borrow would last through the runs.
        let len = self.cells.borrow().len();
        self.run_cells(self.current.get()..len);
    }

    fn clear_outputs(&self, range: impl Iterator<Item = usize>) {
        for i in range {
            let mut cells = self.cells.borrow_mut();
            let Some(ui) = cells.get_mut(i) else { continue };
            if ui.cell.outputs.is_empty() && ui.took.is_none() {
                continue;
            }
            ui.cell.outputs.clear();
            ui.took = None;
            drop(cells);
            let cells = self.cells.borrow();
            self.show_outputs(&cells[i]);
            self.style_cell(&cells[i]);
            self.mark_dirty();
        }
    }

    fn clear_selected(&self) {
        self.clear_outputs(self.selection());
    }

    fn clear_all(&self) {
        let len = self.cells.borrow().len();
        self.clear_outputs(0..len);
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
        let Some((target, frame)) = self.cells.borrow().get(i).map(|c| (self.target(c), c.frame.clone())) else { return };
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
    /// (as the key on the current cell), `backspace` (as the key in the current cell, any
    /// cursor position), `arrow-up`/`arrow-down` (as the keys at a cell's
    /// edge), `focus-<n>`, `scroll-end`, `state` (printed), `key-<[shift-|ctrl-]name>`
    /// (a key as the focused cell gets it: `key-esc` in an editor enters command mode,
    /// the rest go to command mode, e.g. `key-a`, `key-shift-Down`), `cells` (printed),
    /// the toolbar menu's `run-above`, `run-below`, `clear`, `clear-all`.
    pub fn test_action(&self, verb: &str) {
        let i = self.current.get();
        match verb {
            "add" => self.add_below(CellKind::Code),
            "delete" => self.delete_current(),
            "backspace" => println!("notebook: backspace {}", if self.backspace_empty(i) { "deleted the cell" } else { "edits" }),
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
                let focused = self.cells.borrow().iter().position(|c| c.view.has_focus() || c.rendered.has_focus() || c.frame.has_focus());
                println!("notebook: {} cells, current {i}, focused {focused:?}, scroll {:.0}/{:.0} (page {:.0})", self.cells.borrow().len(), adj.value(), adj.upper(), adj.page_size());
            }
            "key-esc" => self.enter_command(i),
            "run-above" => self.run_above(),
            "run-below" => self.run_below(),
            "clear" => self.clear_selected(),
            "clear-all" => self.clear_all(),
            "variables" => self.show_variables(),
            "variables-state" => println!("notebook: variables {}", self.variables.borrow().as_ref().map_or_else(|| "not open".into(), |v| v.describe())),
            v if v.starts_with("table:") => self.open_table(&v["table:".len()..]),
            // `frame:N`: follows the Nth frame link of the current cell's error output.
            v if v.starts_with("frame:") => {
                let n: usize = v["frame:".len()..].parse().unwrap_or(0);
                let frames: Vec<Frame> = self.cells.borrow().get(i).map_or_else(Vec::new, |c| {
                    c.cell.outputs.iter().filter_map(|o| match shown(o) {
                        Some(Shown::Error(t)) => Some(t),
                        _ => None,
                    }).flat_map(|t| t.lines().filter_map(|l| frame(l).map(|(_, f)| f)).collect::<Vec<_>>()).collect()
                });
                println!("notebook: frames {frames:?}");
                if let Some(f) = frames.get(n) {
                    self.open_frame(f);
                }
            }
            // `kernels[:N]`: the picker's rows printed, then row N picked.
            v if v == "kernels" || v.starts_with("kernels:") => {
                let pick = v.strip_prefix("kernels:").and_then(|n| n.parse::<usize>().ok());
                let me = self.me.borrow().clone();
                let python = self.python.borrow().clone();
                crate::interpreter::gather(&self.path, self.folder.as_deref(), python.as_deref(), Some(&self.notebook.borrow().kernel()), move |rows| {
                    for r in &rows {
                        println!("notebook: kernel row {}", crate::interpreter::label(r));
                    }
                    if let (Some(v), Some(row)) = (me.upgrade(), pick.and_then(|i| rows.get(i))) {
                        v.picked(row);
                    }
                });
            }
            v if v.starts_with("key-") => {
                let mut mods = gdk::ModifierType::empty();
                let mut name = &v[4..];
                for (prefix, m) in [("shift-", gdk::ModifierType::SHIFT_MASK), ("ctrl-", gdk::ModifierType::CONTROL_MASK)] {
                    if let Some(rest) = name.strip_prefix(prefix) {
                        mods |= m;
                        name = rest;
                    }
                }
                match gdk::Key::from_name(name) {
                    Some(key) if self.command.get() => println!("notebook: {v} {}", if self.command_key(key, mods) { "taken" } else { "ignored" }),
                    Some(_) => println!("notebook: {v} ignored (editing)"),
                    None => eprintln!("test action: unknown key {name}"),
                }
            }
            "cells" => {
                let cells = self.cells.borrow();
                let selection = self.selection();
                let list: Vec<String> = cells
                    .iter()
                    .enumerate()
                    .map(|(n, c)| {
                        let text = c.buffer.text(&c.buffer.start_iter(), &c.buffer.end_iter(), false);
                        let mark = if n == i { ">" } else if selection.contains(&n) { "+" } else { " " };
                        format!("{mark}{n} {:?} {:?} {:?}", c.cell.kind, c.count.text(), text.lines().next().unwrap_or(""))
                    })
                    .collect();
                println!("notebook: {} mode, cells:\n  {}", if self.command.get() { "command" } else { "edit" }, list.join("\n  "));
            }
            v if v.starts_with("focus-") => {
                if let Some(n) = v[6..].parse().ok().filter(|&n: &usize| n < self.cells.borrow().len()) {
                    self.focus_cell(n);
                }
            }
            _ => eprintln!("test action: unknown cell verb {verb}"),
        }
    }

    /// Backspace in cell `i`: an empty cell with a cell above is deleted, and typing
    /// goes on at the end of the cell above. False (the key edits as usual) otherwise.
    fn backspace_empty(&self, i: usize) -> bool {
        let empty = self.cells.borrow().get(i).is_some_and(|c| c.buffer.char_count() == 0);
        if !empty || i == 0 {
            return false;
        }
        self.remove(i);
        let cells = self.cells.borrow();
        let above = &cells[i - 1];
        above.buffer.place_cursor(&above.buffer.end_iter());
        drop(cells);
        self.focus_cell(i - 1);
        true
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
        self.remove(i);
        self.focus_cell(i.min(self.cells.borrow().len() - 1));
    }

    /// Takes cell `i` out (there must be another).
    fn remove(&self, i: usize) {
        let ui = self.cells.borrow_mut().remove(i);
        // Before the view goes: unparents its popovers (see `lsp::release_popovers`).
        crate::lsp::detach(&ui.buffer);
        crate::lsp::release_popovers(ui.view.upcast_ref());
        self.lsp_resync();
        self.list.remove(&ui.frame);
        self.mark_dirty();
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
                // Which Python a Python kernel runs on ("Python 3 (ipykernel) · .venv").
                let python = self.python.borrow().clone();
                let name = match python {
                    Some(p) if self.notebook.borrow().kernel() == "python3" => {
                        self.chip_button.set_tooltip_text(Some(&format!("{} · Alt+P picks another", p.display())));
                        format!("{display} · {}", crate::interpreter::short_name(&p))
                    }
                    _ => display,
                };
                self.set_chip(&format!("{name} · idle"), None);
                let waiting: Vec<u64> = self.waiting.borrow_mut().drain(..).collect();
                for key in waiting {
                    let code = self.cells.borrow().iter().find(|c| c.key == key).map(|c| c.buffer.text(&c.buffer.start_iter(), &c.buffer.end_iter(), false).to_string());
                    if let (Some(code), Some(k)) = (code, self.kernel.borrow_mut().as_mut()) {
                        let _ = k.execute(key, &code);
                    }
                }
            }
            Event::Status(state) => {
                self.set_chip(&format!("{} · {state}", self.chip_name()), (state == "busy").then_some("busy"));
            }
            Event::Count { cell, count } => {
                let mut cells = self.cells.borrow_mut();
                if let Some(ui) = cells.iter_mut().find(|c| c.key == cell) {
                    ui.cell.count = Some(count);
                    ui.started = Some(Instant::now());
                }
            }
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
                if let Some(ui) = self.cells.borrow_mut().iter_mut().find(|c| c.key == cell) {
                    ui.took = ui.started.take().map(|s| s.elapsed());
                }
                if let Some(ui) = self.cells.borrow().iter().find(|c| c.key == cell) {
                    // No new output: the old one goes now.
                    if ui.outputs.has_css_class("stale") {
                        self.show_outputs(ui);
                    }
                    self.style_cell(ui);
                }
                self.refresh_variables(false);
            }
            Event::Eval { id, result } => {
                let reply = self.evals.borrow_mut().remove(&id);
                if let Some(reply) = reply {
                    reply(self, result);
                }
            }
            Event::Completions { id, matches, start, end } => {
                let reply = self.completions.borrow_mut().remove(&id);
                if let Some(reply) = reply {
                    reply(matches, start, end);
                }
            }
            Event::Restarted => {
                self.set_chip(&format!("{} · restarted", self.chip_name()), None);
                for ui in self.cells.borrow().iter() {
                    self.style_cell(ui);
                }
                self.refresh_variables(false);
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

/// A run's duration beside the count: `85 ms`, `2.4 s`, `3 min 5 s`.
fn run_time(took: Duration) -> String {
    let ms = took.as_millis();
    match ms {
        0 => "< 1 ms".into(),
        1..1000 => format!("{ms} ms"),
        1000..60_000 => format!("{:.1} s", took.as_secs_f64()),
        _ => format!("{} min {} s", ms / 60_000, ms / 1000 % 60),
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

/// A text output; past `FOLD_LINES` lines, the first `FOLD_SHOWN` and a button for all.
fn folded(text: &str) -> gtk::Widget {
    let lines = text.lines().count();
    if lines <= FOLD_LINES {
        return output_label(text, false).upcast();
    }
    let head: String = text.lines().take(FOLD_SHOWN).collect::<Vec<_>>().join("\n");
    let label = output_label(&head, false);
    let more = gtk::Button::with_label(&format!("Show all {lines} lines"));
    more.add_css_class("fold");
    more.set_halign(gtk::Align::Start);
    let (full, short) = (text.to_string(), head);
    let shown_all = StdCell::new(false);
    let l = label.clone();
    more.connect_clicked(move |b| {
        let all = !shown_all.get();
        shown_all.set(all);
        l.set_text(if all { &full } else { &short });
        b.set_label(&if all { "Show less".to_string() } else { format!("Show all {lines} lines") });
    });
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 0);
    box_.append(&label);
    box_.append(&more);
    box_.upcast()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_times_read_short() {
        assert_eq!(run_time(Duration::from_micros(400)), "< 1 ms");
        assert_eq!(run_time(Duration::from_millis(85)), "85 ms");
        assert_eq!(run_time(Duration::from_millis(2_440)), "2.4 s");
        assert_eq!(run_time(Duration::from_secs(185)), "3 min 5 s");
    }
}
