// SPDX-License-Identifier: GPL-3.0-or-later
//! Language servers in the editor (v0.3 phase 6). One server runs per workspace folder
//! and language, started when the first matching file opens; a workspace without a
//! folder starts none. Server threads hand messages to the main loop
//! (`MainContext::invoke`), so nothing polls and the UI never waits on a server.
//!
//! Shown: diagnostics (a squiggle plus a gutter icon, the message on hover: never by
//! colour alone; unused code is faded), hover text with signatures (mouse tooltip, or
//! `Alt+I` at the cursor), completion (`completion.rs`), signature help while typing a
//! call, formatting (`Ctrl+Shift+I`) and go to definition (`F12`, Ctrl+click).
//!
//! A file can have linters beside its main server (`servers::LINTERS`, e.g. Ruff for
//! Python): they get the same text and add diagnostics and formatting. A notebook's
//! code cells are one virtual Python file (`notebook_path`), each cell an offset in it.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::{gdk, glib, pango};
use gtk4 as gtk;
use sourceview5::prelude::*;
use tileide_config::model::Lsp as Setting;
use tileide_core::theme::{self, Theme};
use tileide_lsp::Value;
use tileide_lsp::client::{Connection, Incoming};
use tileide_lsp::protocol::{self, Capabilities, Diagnostic, Position, Severity};
use tileide_lsp::servers::{self, Server as Spec};
use tileide_lsp::text::{self, Encoding};

use crate::completion::Popup;

/// Typing settles this long before the server gets the new text.
const CHANGE_DELAY: Duration = Duration::from_millis(150);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
const HOVER_MAX_CHARS: usize = 1500;
/// After a change reaches the servers: time for them to analyse it before hints are asked.
const HINTS_DELAY: Duration = Duration::from_millis(600);
/// Tag and mark category per severity (hints share the info look).
/// Faded text for unused code.
const UNUSED: &str = "lsp-unused";
const KINDS: [(Severity, &str, &str); 3] = [(Severity::Error, "lsp-error", "dialog-error-symbolic"), (Severity::Warning, "lsp-warning", "dialog-warning-symbolic"), (Severity::Info, "lsp-info", "dialog-information-symbolic")];

/// Opens a definition: (file, position, the server's column encoding).
pub type Goto = Rc<dyn Fn(PathBuf, Position, Encoding)>;
type Reply = Box<dyn FnOnce(&Value)>;

struct Server {
    root: PathBuf,
    spec: &'static Spec,
    program: String,
    conn: Connection,
    /// `None` until `initialize` returns; messages wait in `queue` until then.
    caps: Option<Capabilities>,
    queue: Vec<Value>,
    pending: HashMap<i64, Reply>,
    /// One version per file, however many buffers show it.
    versions: HashMap<PathBuf, i32>,
}

/// A notebook's code cells, in order (asked at each sync, so it follows adds, moves
/// and deletes).
pub struct Cells {
    pub list: Box<dyn Fn() -> Vec<sourceview5::Buffer>>,
    /// Completion from the notebook's running kernel: (cell code, cursor in chars, reply
    /// with matches and the replaced char range). False when no kernel is running.
    pub kernel: KernelComplete,
}

pub type KernelReply = Box<dyn FnOnce(Vec<String>, usize, usize)>;
pub type KernelComplete = Box<dyn Fn(&str, usize, KernelReply) -> bool>;

/// Completion answers gathered for one request: the server's, then the kernel's.
#[derive(Default)]
struct Gathered {
    server: Option<(Vec<protocol::Completion>, bool)>,
    kernel: Vec<protocol::Completion>,
    waiting: usize,
}

/// Rules that don't fit notebook cells: a bare expression ends a cell to show its
/// value, imports go in any cell (and aren't sorted across cells), `;` hides an
/// output, top-level `await` works.
const NOTEBOOK_QUIET: &[&str] = &["reportUnusedExpression", "reportUnusedCallResult", "reportUnusedCoroutine", "E402", "I001", "E703", "B018", "F704", "PLE1142"];

struct Doc {
    buffer: glib::WeakRef<sourceview5::Buffer>,
    view: glib::WeakRef<sourceview5::View>,
    /// The file, or a notebook's virtual Python file.
    path: PathBuf,
    /// The main server first, then linters.
    servers: Vec<usize>,
    goto: Goto,
    popup: Rc<Popup>,
    signature: Option<gtk::Popover>,
    timer: Option<glib::SourceId>,
    /// Per server, in this buffer's lines.
    diagnostics: BTreeMap<usize, Vec<Diagnostic>>,
    /// The hover markup for the word starting at (line, offset), once asked (`None` = nothing to show).
    hover: Option<((i32, i32), Option<String>)>,
    /// Bumped per completion / signature request, so a late reply never replaces a newer one.
    completion_generation: u64,
    signature_generation: u64,
    /// A notebook cell: its notebook's cells, and where it sits in the virtual file.
    cells: Option<Rc<Cells>>,
    offset: u32,
    lines: u32,
    handlers: Vec<glib::SignalHandlerId>,
    view_handlers: Vec<(glib::WeakRef<gtk::Widget>, glib::SignalHandlerId)>,
    controllers: Vec<gtk::EventController>,
    /// Inlay hints, one annotation per line (`editor.inlay_hints`).
    hints: sourceview5::AnnotationProvider,
    hints_timer: Option<glib::SourceId>,
    /// Hints were shown since they were turned on; their lines and texts (test actions).
    hinted: bool,
    hint_texts: Vec<(u32, String)>,
}

#[derive(Default)]
struct Hub {
    servers: Vec<Option<Server>>,
    docs: Vec<Doc>,
    /// `tileide.lsp(…)` by canonical server name.
    settings: BTreeMap<&'static str, Setting>,
    /// Folder and server pairs that failed: not retried until the config changes.
    failed: HashSet<(PathBuf, &'static str)>,
    notice: Option<Rc<dyn Fn(String)>>,
    theme: Option<Rc<Theme>>,
}

thread_local! {
    static HUB: RefCell<Hub> = RefCell::new(Hub::default());
    static INLAY_HINTS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Applies `tileide.lsp` settings; returns warnings for unknown server names.
pub fn configure(settings: &BTreeMap<String, Setting>, notice: Rc<dyn Fn(String)>) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut known = BTreeMap::new();
    for (name, setting) in settings {
        match servers::by_name(name) {
            Some(spec) => {
                known.insert(spec.name, setting.clone());
            }
            None => warnings.push(format!("lsp: unknown server \"{name}\" (built in: rust, c, python, typescript, java, ruff)")),
        }
    }
    HUB.with_borrow_mut(|hub| {
        if hub.settings != known {
            hub.failed.clear();
        }
        hub.settings = known;
        hub.notice = Some(notice);
    });
    warnings
}

pub fn set_theme(theme: Rc<Theme>) {
    let buffers: Vec<_> = HUB.with_borrow_mut(|hub| {
        hub.theme = Some(theme.clone());
        hub.docs.iter().filter_map(|d| d.buffer.upgrade()).collect()
    });
    for buffer in buffers {
        color_tags(&buffer, &theme);
    }
}

/// Starts language support for an editor on `path` (again, if it was attached before).
/// `root`: the workspace's folder; `None` starts nothing.
pub fn attach(buffer: &sourceview5::Buffer, view: &sourceview5::View, path: &Path, root: Option<&Path>, goto: Goto) {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    attach_doc(buffer, view, path, root, goto, None);
}

/// The virtual Python file standing for a notebook's code cells (never written).
pub fn notebook_path(notebook: &Path) -> PathBuf {
    let notebook = notebook.canonicalize().unwrap_or_else(|_| notebook.to_path_buf());
    let name = notebook.file_name().map_or_else(|| "notebook".into(), |n| n.to_string_lossy().into_owned());
    notebook.with_file_name(format!(".{name}.py"))
}

/// Language support for one code cell of the notebook at `notebook`.
pub fn attach_cell(buffer: &sourceview5::Buffer, view: &sourceview5::View, notebook: &Path, root: Option<&Path>, goto: Goto, cells: Rc<Cells>) {
    attach_doc(buffer, view, notebook_path(notebook), root, goto, Some(cells));
}

/// A notebook's cells were added, moved or removed: the servers get the new layout.
pub fn cells_changed(any_cell: &sourceview5::Buffer) {
    changed(any_cell);
}

fn attach_doc(buffer: &sourceview5::Buffer, view: &sourceview5::View, path: PathBuf, root: Option<&Path>, goto: Goto, cells: Option<Rc<Cells>>) {
    detach(buffer);
    let (Some(root), Some((spec, language))) = (root, servers::for_file(&path)) else { return };
    let project = (spec.name == "python").then(|| tileide_core::python::project_root(&path, root)).flatten();
    let root = project.as_deref().unwrap_or(root);
    let Some(servers) = HUB.with_borrow_mut(|hub| {
        let main = start(hub, root, spec)?;
        Some(std::iter::once(main).chain(servers::linters_for(&path, root).into_iter().filter_map(|l| start(hub, root, l))).collect::<Vec<usize>>())
    }) else {
        return;
    };

    let theme = HUB.with_borrow(|hub| hub.theme.clone()).unwrap_or_else(|| Rc::new(Theme::builtin()));
    color_tags(buffer, &theme);
    view.set_show_line_marks(true);
    for (i, (_, category, icon)) in KINDS.iter().enumerate() {
        let attrs = sourceview5::MarkAttributes::new();
        attrs.set_icon_name(icon);
        attrs.connect_query_tooltip_text(|_, mark| mark.buffer().and_then(|b| line_messages(b.downcast_ref()?, mark_line(mark))).unwrap_or_default());
        view.set_mark_attributes(category, &attrs, (KINDS.len() - i) as i32);
    }
    view.set_has_tooltip(true);
    let popup = Popup::new(view, completion_hooks(buffer));

    let mut handlers = Vec::new();
    let b = buffer.downgrade();
    handlers.push(buffer.connect_changed(move |_| {
        if let Some(buffer) = b.upgrade() {
            changed(&buffer);
        }
    }));
    let b = buffer.downgrade();
    handlers.push(buffer.connect_insert_text(move |_, _, text| {
        // After the insert lands: the cursor and the word are then up to date.
        let (b, text) = (b.clone(), text.to_string());
        glib::idle_add_local_once(move || {
            if let Some(buffer) = b.upgrade() {
                typed(&buffer, &text);
            }
        });
    }));
    let b = buffer.downgrade();
    handlers.push(buffer.connect_delete_range(move |_, _, _| {
        let b = b.clone();
        glib::idle_add_local_once(move || {
            if let Some(popup) = b.upgrade().and_then(|buffer| with_doc(&buffer, |d| d.popup.clone())) {
                popup.refilter();
            }
        });
    }));
    // The cursor moved (arrows, a click): an open signature card follows or closes.
    let b = buffer.downgrade();
    handlers.push(buffer.connect_cursor_position_notify(move |_| {
        if let Some(buffer) = b.upgrade()
            && let Some(view) = with_doc(&buffer, |d| d.signature.as_ref().is_some_and(|p| p.is_visible()).then(|| d.view.upgrade()).flatten()).flatten()
        {
            signature(&view);
        }
    }));

    let tooltip = view.connect_query_tooltip(|view, x, y, keyboard, tooltip| !keyboard && query_tooltip(view, x, y, tooltip));
    let view_handlers = vec![(view.clone().upcast::<gtk::Widget>().downgrade(), tooltip)];

    // Ctrl+click: go to definition.
    let click = gtk::GestureClick::new();
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    click.connect_pressed(|gesture, _, x, y| {
        if !gesture.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
            return;
        }
        let Some(view) = gesture.widget().and_downcast::<sourceview5::View>() else { return };
        let (bx, by) = view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        if let Some(iter) = view.iter_at_location(bx, by) {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            definition_at(&view, &iter);
        }
    });
    view.add_controller(click.clone());
    // Tab after a name or a `.` completes (as in Jupyter); elsewhere it indents. Added
    // after the completion list's own keys, so an open list still takes Tab first.
    let tab = gtk::EventControllerKey::new();
    tab.set_propagation_phase(gtk::PropagationPhase::Capture);
    tab.connect_key_pressed(|c, key, _, mods| {
        let Some(view) = c.widget().and_downcast::<sourceview5::View>() else { return glib::Propagation::Proceed };
        if key != gdk::Key::Tab || !(mods & gtk::accelerator_get_default_mod_mask()).is_empty() || !tab_key(&view) {
            return glib::Propagation::Proceed;
        }
        glib::Propagation::Stop
    });
    view.add_controller(tab.clone());
    // Esc closes the signature card; leaving the editor too.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let b = buffer.downgrade();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape && b.upgrade().is_some_and(|buffer| hide_signature(&buffer)) {
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    view.add_controller(keys.clone());
    let focus = gtk::EventControllerFocus::new();
    let b = buffer.downgrade();
    focus.connect_leave(move |_| {
        if let Some(buffer) = b.upgrade() {
            hide_signature(&buffer);
        }
    });
    view.add_controller(focus.clone());
    let hints = sourceview5::AnnotationProvider::new();
    view.annotations().add_provider(&hints);

    let first = HUB.with_borrow_mut(|hub| {
        let first = !hub.docs.iter().any(|d| d.servers[0] == servers[0] && d.path == path);
        hub.docs.push(Doc {
            buffer: buffer.downgrade(),
            view: view.downgrade(),
            path: path.clone(),
            servers: servers.clone(),
            goto,
            popup,
            signature: None,
            timer: None,
            diagnostics: BTreeMap::new(),
            hover: None,
            completion_generation: 0,
            signature_generation: 0,
            cells,
            offset: 0,
            lines: 0,
            handlers,
            view_handlers,
            controllers: vec![click.upcast(), tab.upcast(), keys.upcast(), focus.upcast()],
            hints,
            hints_timer: None,
            hinted: false,
            hint_texts: Vec::new(),
        });
        first
    });
    let text = compose(buffer);
    if !first {
        // Another cell of an open notebook: one sync for however many attach together.
        changed(buffer);
        return;
    }
    HUB.with_borrow_mut(|hub| {
        for &id in &servers {
            if let Some(s) = hub.servers[id].as_mut() {
                let version = *s.versions.entry(path.clone()).or_insert(1);
                send(s, protocol::did_open(&path, language, version, &text));
                pull_diagnostics(id, s, &path);
            }
        }
    });
}

/// Asks a server that doesn't send diagnostics for `path`'s (ESLint); nothing for one
/// still starting (`initialized` asks then) or one that sends them.
fn pull_diagnostics(id: usize, s: &mut Server, path: &Path) {
    if !s.caps.as_ref().is_some_and(|c| c.pull_diagnostics) {
        return;
    }
    let Ok(rid) = s.conn.request("textDocument/diagnostic", protocol::document_diagnostic(path)) else { return };
    let path = path.to_path_buf();
    s.pending.insert(
        rid,
        Box::new(move |m: &Value| {
            if let Some(list) = protocol::pulled_diagnostics(&path, &m["result"]) {
                show_diagnostics(id, &path, list);
            }
        }),
    );
}

/// Every open file's diagnostics from server `id`, asked again.
fn pull_all(hub: &mut Hub, id: usize) {
    let paths: Vec<PathBuf> = hub.docs.iter().filter(|d| d.servers.contains(&id)).map(|d| d.path.clone()).collect();
    if let Some(s) = hub.servers[id].as_mut() {
        for path in paths {
            pull_diagnostics(id, s, &path);
        }
    }
}

/// Ends language support for `buffer` (tile closed or replaced, file renamed).
pub fn detach(buffer: &sourceview5::Buffer) {
    let Some(doc) = HUB.with_borrow_mut(|hub| {
        let i = hub.docs.iter().position(|d| d.buffer.upgrade().as_ref() == Some(buffer))?;
        let doc = hub.docs.remove(i);
        let last = !hub.docs.iter().any(|d| d.servers[0] == doc.servers[0] && d.path == doc.path);
        if last {
            for &id in &doc.servers {
                if let Some(s) = hub.servers[id].as_mut() {
                    s.versions.remove(&doc.path);
                    send(s, protocol::did_close(&doc.path));
                }
            }
        }
        Some(doc)
    }) else {
        return;
    };
    for timer in [doc.timer, doc.hints_timer].into_iter().flatten() {
        timer.remove();
    }
    if let Some(view) = doc.view.upgrade() {
        view.annotations().remove_provider(&doc.hints);
    }
    for handler in doc.handlers {
        buffer.disconnect(handler);
    }
    for (widget, handler) in doc.view_handlers {
        if let Some(widget) = widget.upgrade() {
            widget.disconnect(handler);
        }
    }
    if let Some(view) = doc.view.upgrade() {
        for controller in doc.controllers {
            view.remove_controller(&controller);
        }
    }
    doc.popup.hide();
    if let Some(view) = doc.view.upgrade() {
        release_popovers(view.upcast_ref());
    }
    clear_marks(buffer);
}

/// After a successful write.
pub fn saved(buffer: &sourceview5::Buffer) {
    flush(buffer);
    HUB.with_borrow_mut(|hub| {
        let Some(d) = hub.docs.iter().find(|d| d.buffer.upgrade().as_ref() == Some(buffer)) else { return };
        let path = d.path.clone();
        for id in d.servers.clone() {
            if let Some(s) = hub.servers[id].as_mut() {
                send(s, protocol::did_save(&path));
            }
        }
    });
}

/// Stops servers whose folder no workspace uses any more, and forgets dropped editors.
pub fn retain_roots(roots: &[PathBuf]) {
    let stopped: Vec<Server> = HUB.with_borrow_mut(|hub| {
        hub.docs.retain(|d| d.buffer.upgrade().is_some());
        let docs = &hub.docs;
        hub.servers
            .iter_mut()
            .enumerate()
            .filter(|(i, s)| s.as_ref().is_some_and(|s| !roots.contains(&s.root) && !docs.iter().any(|d| d.servers.contains(i))))
            .filter_map(|(_, s)| s.take())
            .collect()
    });
    for server in stopped {
        server.conn.shutdown(SHUTDOWN_GRACE);
    }
}

/// On quit.
pub fn shutdown_all() {
    let all: Vec<Server> = HUB.with_borrow_mut(|hub| hub.servers.iter_mut().filter_map(Option::take).collect());
    for server in all {
        server.conn.shutdown(SHUTDOWN_GRACE);
    }
}

/// `Alt+I`: diagnostics and hover text for the cursor's position, in a popover.
pub fn hover_at_cursor(view: &sourceview5::View) {
    let buffer: sourceview5::Buffer = view.buffer().downcast().expect("source buffer");
    let cursor = buffer.iter_at_mark(&buffer.get_insert());
    let problems = messages_at(&buffer, &cursor);
    let v = view.downgrade();
    let shown = request_at(&buffer, &cursor, "textDocument/hover", |server| server.caps.as_ref().is_some_and(|c| c.hover), move |result| {
        let Some(view) = v.upgrade() else { return };
        let text = [problems.map(|p| escape(&p)), hover_markup(result)].into_iter().flatten().collect::<Vec<_>>().join("\n\n");
        show_info(&view, if text.is_empty() { "Nothing to show here." } else { &text });
    });
    if !shown {
        let text = messages_at(&buffer, &cursor).map_or_else(|| "No language server for this file.".into(), |m| escape(&m));
        show_info(view, &text);
    }
}

/// `F12`: the definition of what is under the cursor.
pub fn definition_at_cursor(view: &sourceview5::View) {
    let buffer = view.buffer();
    let cursor = buffer.iter_at_mark(&buffer.get_insert());
    definition_at(view, &cursor);
}

fn definition_at(view: &sourceview5::View, iter: &gtk::TextIter) {
    let buffer: sourceview5::Buffer = view.buffer().downcast().expect("source buffer");
    let Some((goto, encoding, path)) = with_doc(&buffer, |d| (d.goto.clone(), server_encoding(d.servers[0]), d.path.clone())) else { return };
    request_at(&buffer, iter, "textDocument/definition", |s| s.caps.as_ref().is_some_and(|c| c.definition), move |result| {
        let Some(location) = protocol::definition(result) else { return };
        if location.path == path && reveal_in_cells(&path, location.start, encoding) {
            return;
        }
        goto(location.path, location.start, encoding);
    });
}

/// What asked for completion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ask {
    /// Typing (a trigger character, or a word growing): the list shows as answers come.
    Typing,
    /// `Ctrl+Space` / `Alt+Tab`: like typing, but an empty answer closes the list.
    Manual,
    /// Tab after a name or a `.` (as in Jupyter): waits for every source, then fills in
    /// a single match at once or shows the list.
    Tab,
}

/// Completion at the cursor. In a notebook cell the running kernel is asked too
/// (runtime names such as a DataFrame's columns); its matches follow the server's,
/// without repeats.
pub fn complete(view: &sourceview5::View, ask: Ask) {
    let buffer: sourceview5::Buffer = view.buffer().downcast().expect("source buffer");
    let cursor = buffer.iter_at_mark(&buffer.get_insert());
    let anchor = word_start(&cursor);
    let Some((generation, popup, encoding, cells)) = HUB.with_borrow_mut(|hub| {
        let d = hub.docs.iter_mut().find(|d| d.buffer.upgrade().as_ref() == Some(&buffer))?;
        d.completion_generation += 1;
        Some((d.completion_generation, d.popup.clone(), hub.servers[d.servers[0]].as_ref()?.caps.as_ref()?.encoding, d.cells.clone()))
    }) else {
        return;
    };
    let anchor_mark = buffer.create_mark(None, &anchor, true);
    let gathered = Rc::new(RefCell::new(Gathered::default()));
    // Shows what has come so far; the last answer removes the anchor mark.
    let arrived = {
        let (b, gathered) = (buffer.downgrade(), gathered.clone());
        Rc::new(move || {
            let Some(buffer) = b.upgrade() else { return };
            let last = {
                let mut g = gathered.borrow_mut();
                g.waiting -= 1;
                g.waiting == 0
            };
            let anchor = buffer.iter_at_mark(&anchor_mark);
            if last {
                buffer.delete_mark(&anchor_mark);
            }
            if with_doc(&buffer, |d| d.completion_generation) != Some(generation) {
                return;
            }
            if ask == Ask::Tab && !last {
                return;
            }
            let g = gathered.borrow();
            let (mut items, incomplete) = g.server.clone().unwrap_or_default();
            let known: HashSet<String> = items.iter().map(|i| i.insert.clone()).collect();
            items.extend(g.kernel.iter().filter(|k| !known.contains(&k.insert)).cloned());
            if items.is_empty() {
                if ask != Ask::Typing && last {
                    popup.hide();
                }
                return;
            }
            drop(g);
            popup.show(items, incomplete, &anchor, encoding);
            if ask == Ask::Tab {
                popup.take_single();
            }
        })
    };

    gathered.borrow_mut().waiting += 1;
    let (b, g, done) = (buffer.downgrade(), gathered.clone(), arrived.clone());
    let sent = request_at(&buffer, &cursor, "textDocument/completion", |s| s.caps.as_ref().is_some_and(|c| c.completion), move |result| {
        let (mut items, incomplete) = protocol::completions(result);
        // Ranges in the notebook's virtual file → this cell's lines.
        let offset = b.upgrade().and_then(|buffer| with_doc(&buffer, |d| d.offset)).unwrap_or(0);
        for item in &mut items {
            item.replace = item.replace.filter(|r| r.start.line >= offset).map(|mut r| {
                r.start.line -= offset;
                r.end.line = r.end.line.saturating_sub(offset);
                r
            });
        }
        g.borrow_mut().server = Some((items, incomplete));
        done();
    });
    if !sent {
        arrived();
    }

    if let Some(cells) = cells {
        let code = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string();
        gathered.borrow_mut().waiting += 1;
        let (b, g, done) = (buffer.downgrade(), gathered.clone(), arrived.clone());
        let asked = (cells.kernel)(
            &code,
            cursor.offset() as usize,
            Box::new(move |matches, start, end| {
                if let Some(buffer) = b.upgrade() {
                    let at = |chars: usize| position_of(buffer.upcast_ref(), &buffer.iter_at_offset(chars as i32), encoding);
                    let replace = protocol::Range { start: at(start), end: at(end) };
                    g.borrow_mut().kernel = matches
                        .into_iter()
                        .map(|m| protocol::Completion {
                            label: m.clone(),
                            detail: Some("kernel".into()),
                            filter: m.trim_matches(|c: char| !is_word_char(c)).to_string(),
                            sort: format!("~{m}"),
                            kind: None,
                            doc: Vec::new(),
                            edits: Vec::new(),
                            raw: Value::Null,
                            insert: m,
                            replace: Some(replace),
                        })
                        .collect();
                }
                done();
            }),
        );
        if !asked {
            arrived();
        }
    }
}

/// Tab: completes when the cursor follows a name or a `.` (no selection) and returns
/// true; after whitespace, at a line's start or with text selected, returns false so
/// Tab indents as usual. An open completion list takes Tab before this runs.
pub fn tab_key(view: &sourceview5::View) -> bool {
    let buffer = view.buffer();
    if buffer.has_selection() {
        return false;
    }
    let mut before = buffer.iter_at_mark(&buffer.get_insert());
    if !(before.backward_char() && (is_word_char(before.char()) || before.char() == '.')) {
        return false;
    }
    complete(view, Ask::Tab);
    true
}

/// The editor position as LSP's, in `encoding`.
pub fn position_of(buffer: &gtk::TextBuffer, iter: &gtk::TextIter, encoding: Encoding) -> Position {
    let line = iter.line();
    Position { line: line as u32, column: text::to_lsp(&line_text(buffer, line), iter.line_offset() as usize, encoding) }
}

/// LSP's position as an editor iterator, clamped to the buffer.
pub fn iter_at(buffer: &gtk::TextBuffer, p: Position, encoding: Encoding) -> gtk::TextIter {
    // Past the last line (an edit's end at "the next line"): the end of the text.
    if p.line as i32 >= buffer.line_count() {
        return buffer.end_iter();
    }
    let line = (p.line as i32).max(0);
    let column = text::from_lsp(&line_text(buffer, line), p.column, encoding);
    buffer.iter_at_line_offset(line, column as i32).unwrap_or_else(|| buffer.end_iter())
}

/// Line `line`'s text without its line break.
fn line_text(buffer: &gtk::TextBuffer, line: i32) -> String {
    let Some(start) = buffer.iter_at_line(line) else { return String::new() };
    let mut end = start;
    if !end.ends_line() {
        end.forward_to_line_end();
    }
    buffer.text(&start, &end, true).to_string()
}

pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Unparents the popovers TileIDE attached to a text view (completion, `Alt+I` info).
/// Must run before the view is disposed: GtkTextView's dispose removes children with
/// `gtk_text_view_remove`, which only knows its own, warns about any other and leaves
/// it in place, so its `while (first_child)` loop never ends and the app freezes.
pub fn release_popovers(view: &gtk::Widget) {
    let mut child = view.first_child();
    while let Some(c) = child {
        child = c.next_sibling();
        if c.is::<gtk::Popover>() {
            c.unparent();
        }
    }
}

/// The start of the word that ends at `iter` (or `iter` itself).
pub fn word_start(iter: &gtk::TextIter) -> gtk::TextIter {
    let mut start = *iter;
    loop {
        let mut back = start;
        if !back.backward_char() || !is_word_char(back.char()) {
            return start;
        }
        start = back;
    }
}

// ---- servers ----

/// Python servers analyse with the Python chosen for the project, else its nearest
/// virtualenv (from the root up to home), so its packages resolve.
fn python_settings(spec: &Spec, root: &Path) -> Value {
    if spec.name != "python" {
        return Value::Null;
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let python = tileide_core::python::chosen(root)
        .or_else(|| tileide_core::python::venvs(root, None, std::env::var_os("VIRTUAL_ENV").map(PathBuf::from).as_deref(), home.as_deref()).into_iter().next());
    python.map_or(Value::Null, |p| serde_json::json!({"python": {"pythonPath": p}}))
}

/// TypeScript's inlay hints are off unless asked for: TypeScript 7's server reads them
/// as settings (VS Code's names), typescript-language-server as `preferences`.
fn typescript_settings() -> Value {
    let hints = serde_json::json!({"inlayHints": {
        "parameterNames": {"enabled": "all", "suppressWhenArgumentMatchesName": true},
        "variableTypes": {"enabled": true},
        "propertyDeclarationTypes": {"enabled": true},
        "functionLikeReturnTypes": {"enabled": true},
        "enumMemberValues": {"enabled": true},
    }});
    serde_json::json!({"typescript": hints, "javascript": hints, "js/ts": hints})
}

/// jdtls keeps signature help off unless asked; parameter-name hints cover every
/// argument, as for the other languages (its default is literals only).
fn java_settings() -> Value {
    serde_json::json!({"java": {"signatureHelp": {"enabled": true}, "inlayHints": {"parameterNames": {"enabled": "all"}}}})
}

/// rust-analyzer checks on save with Clippy when it's installed (lints beyond the
/// compiler's), else with `cargo check` as before. rustup has a `cargo-clippy` proxy
/// even without the component, so ask Clippy itself, once.
fn rust_settings() -> Value {
    thread_local! {
        static CLIPPY: bool = std::process::Command::new("cargo")
            .args(["clippy", "--version"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
    }
    if CLIPPY.with(|c| *c) { serde_json::json!({"check": {"command": "clippy"}}) } else { Value::Null }
}

/// What vscode-eslint-language-server needs answered (it asks for the whole object):
/// lint as you type, no formatting (the TypeScript server formats).
fn eslint_settings(root: &Path) -> Value {
    let name = root.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    serde_json::json!({
        "validate": "on", "run": "onType", "format": false, "quiet": false, "onIgnoredFiles": "off",
        "nodePath": null, "packageManager": "npm", "useESLintClass": false, "experimental": {},
        "rulesCustomizations": [], "problems": {"shortenToSingleLine": false},
        "codeActionOnSave": {"enable": false, "mode": "all"},
        "codeAction": {"disableRuleComment": {"enable": true, "location": "separateLine"}, "showDocumentation": {"enable": false}},
        "workingDirectory": {"mode": "location"},
        "workspaceFolder": {"uri": tileide_lsp::text::path_to_uri(root), "name": name},
    })
}

fn typescript_preferences() -> Value {
    serde_json::json!({
        "includeInlayParameterNameHints": "all",
        "includeInlayParameterNameHintsWhenArgumentMatchesName": false,
        "includeInlayVariableTypeHints": true,
        "includeInlayPropertyDeclarationTypeHints": true,
        "includeInlayFunctionLikeReturnTypeHints": true,
        "includeInlayEnumMemberValueHints": true,
    })
}

/// The Python chosen for `project` changed: its Python servers (and those of folders
/// inside it) analyse with the new one.
pub fn python_changed(project: &Path) {
    HUB.with_borrow(|hub| {
        for s in hub.servers.iter().flatten().filter(|s| s.spec.name == "python" && (s.root.starts_with(project) || project.starts_with(&s.root))) {
            if let Err(e) = s.conn.set_settings(python_settings(s.spec, &s.root)) {
                eprintln!("lsp: {e}");
            }
        }
    });
}

fn start(hub: &mut Hub, root: &Path, spec: &'static Spec) -> Option<usize> {
    if let Some(i) = hub.servers.iter().position(|s| s.as_ref().is_some_and(|s| s.root == root && s.spec.name == spec.name)) {
        return Some(i);
    }
    let key = (root.to_path_buf(), spec.name);
    if hub.failed.contains(&key) {
        return None;
    }
    let setting = hub.settings.get(spec.name);
    if setting.is_some_and(|s| !s.enabled) {
        return None;
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    // From the server's root up to home: a workspace folder can sit inside the project
    // whose virtualenv is above it.
    let venvs = tileide_core::python::venvs(root, None, std::env::var_os("VIRTUAL_ENV").map(PathBuf::from).as_deref(), home.as_deref());
    let venv_bins: Vec<PathBuf> = tileide_core::python::chosen(root).iter().chain(&venvs).filter_map(|p| p.parent().map(Path::to_path_buf)).collect();
    let extra = servers::user_dirs(home.as_deref());
    let override_cmd = setting.and_then(|s| s.cmd.as_deref());
    let typescript = (spec.name == "typescript" && override_cmd.is_none()).then(|| {
        let path_dirs = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
        servers::typescript(root, &path_dirs.into_iter().chain(extra.iter().cloned()).collect::<Vec<_>>())
    });
    let cmd = match typescript.as_ref().and_then(Option::as_ref) {
        Some(servers::TypeScript::Native(tsc)) => Some(vec![tsc.to_string_lossy().into_owned(), "--lsp".into(), "--stdio".into()]),
        _ => servers::command(spec, override_cmd, |p| servers::locate(p, &venv_bins, &extra)),
    };
    let Some(cmd) = cmd else {
        hub.failed.insert(key);
        crate::alerts::missing(format!("No {} language server found. To get one: {}.", spec.label, spec.install));
        return None;
    };
    let settings = match spec.name {
        "typescript" => typescript_settings(),
        "java" => java_settings(),
        "rust" => serde_json::json!({"rust-analyzer": rust_settings()}),
        "eslint" => eslint_settings(root),
        _ => python_settings(spec, root),
    };
    let id = hub.servers.len();
    let deliver = move |message: Incoming| glib::MainContext::default().invoke(move || receive(id, message));
    println!("lsp: starting {} for {}", cmd.join(" "), root.display());
    let mut conn = match Connection::spawn(&cmd, root, settings, deliver) {
        Ok(conn) => conn,
        Err(e) => {
            hub.failed.insert(key);
            crate::alerts::missing(format!("{} language server: {e}. {}.", spec.label, spec.install));
            return None;
        }
    };
    let options = match typescript.flatten() {
        Some(servers::TypeScript::Classic(Some(tsserver))) => serde_json::json!({"tsserver": {"path": tsserver}, "preferences": typescript_preferences()}),
        Some(servers::TypeScript::Native(_)) => Value::Null,
        _ if spec.name == "typescript" => serde_json::json!({"preferences": typescript_preferences()}),
        // Read once at start (signature help is registered then or never).
        _ if spec.name == "java" => serde_json::json!({"settings": java_settings()}),
        _ if spec.name == "rust" => rust_settings(),
        _ => Value::Null,
    };
    let Ok(request) = conn.request("initialize", protocol::initialize_params(root, options)) else {
        hub.failed.insert(key);
        return None;
    };
    let mut server = Server { root: root.to_path_buf(), spec, program: cmd[0].clone(), conn, caps: None, queue: Vec::new(), pending: HashMap::new(), versions: HashMap::new() };
    server.pending.insert(request, Box::new(move |reply: &Value| initialized(id, reply)));
    hub.servers.push(Some(server));
    Some(id)
}

fn initialized(id: usize, reply: &Value) {
    HUB.with_borrow_mut(|hub| {
        let Some(s) = hub.servers[id].as_mut() else { return };
        if let Some(error) = reply.get("error") {
            let message = format!("{} language server ({}) refused to start: {}", s.spec.label, s.program, error["message"].as_str().unwrap_or("unknown error"));
            notify(hub, message);
            return;
        }
        s.caps = Some(Capabilities::parse(&reply["result"]));
        let _ = s.conn.send(&protocol::notification("initialized", serde_json_empty()));
        for message in std::mem::take(&mut s.queue) {
            let _ = s.conn.send(&message);
        }
        pull_all(hub, id);
    });
}

fn serde_json_empty() -> Value {
    Value::Object(Default::default())
}

/// A message from server `id`, on the main thread.
fn receive(id: usize, incoming: Incoming) {
    let message = match incoming {
        Incoming::Message(m) => m,
        Incoming::Exited => return exited(id),
    };
    if message.get("method").is_none() {
        let reply = message["id"].as_i64().and_then(|rid| HUB.with_borrow_mut(|hub| hub.servers.get_mut(id)?.as_mut()?.pending.remove(&rid)));
        if let Some(reply) = reply {
            reply(&message);
        }
        return;
    }
    if message["method"] == "textDocument/publishDiagnostics"
        && let Some((path, list)) = protocol::diagnostics(&message["params"])
    {
        show_diagnostics(id, &path, list);
    }
    // A command we ran (quick fix, organize imports) edits through us.
    if message["method"] == "workspace/applyEdit" {
        apply_workspace_edit(&message["params"]["edit"], server_encoding(id));
    }
    let method = message["method"].as_str().unwrap_or("");
    if method == "client/registerCapability" || method == "client/unregisterCapability" {
        HUB.with_borrow_mut(|hub| {
            if let Some(caps) = hub.servers.get_mut(id).and_then(Option::as_mut).and_then(|s| s.caps.as_mut()) {
                caps.register(&message["params"], method == "client/registerCapability");
            }
        });
    }
    if method == "workspace/diagnostic/refresh" {
        HUB.with_borrow_mut(|hub| pull_all(hub, id));
    }
    // The server's hints changed without an edit (rust-analyzer once it has indexed).
    if message["method"] == "workspace/inlayHint/refresh" {
        let buffers: Vec<sourceview5::Buffer> = HUB.with_borrow(|hub| hub.docs.iter().filter(|d| d.servers.contains(&id)).filter_map(|d| d.buffer.upgrade()).collect());
        for buffer in buffers {
            schedule_hints(&buffer, Duration::ZERO);
        }
    }
}

fn exited(id: usize) {
    let buffers: Vec<sourceview5::Buffer> = HUB.with_borrow_mut(|hub| {
        let Some(s) = hub.servers.get_mut(id).and_then(Option::take) else { return Vec::new() };
        hub.failed.insert((s.root.clone(), s.spec.name));
        if s.caps.is_none() {
            crate::alerts::missing(format!("{} language server ({}) exited while starting. To get one: {}.", s.spec.label, s.program, s.spec.install));
        } else {
            notify(hub, format!("{} language server ({}) stopped. It restarts when the config changes.", s.spec.label, s.program));
        }
        hub.docs.iter_mut().filter(|d| d.servers.contains(&id)).filter_map(|d| {
            d.diagnostics.remove(&id);
            d.buffer.upgrade()
        }).collect()
    });
    for buffer in buffers {
        render_diagnostics(&buffer);
    }
}

fn notify(hub: &Hub, message: String) {
    eprintln!("lsp: {message}");
    if let Some(notice) = hub.notice.clone() {
        // Not inside the hub's borrow: the app may redraw and call back in.
        glib::idle_add_local_once(move || notice(message));
    }
}

fn send(server: &mut Server, message: Value) {
    if server.caps.is_none() {
        server.queue.push(message);
    } else if let Err(e) = server.conn.send(&message) {
        eprintln!("lsp: {e}");
    }
}

fn server_encoding(server: usize) -> Encoding {
    HUB.with_borrow(|hub| hub.servers.get(server).and_then(|s| s.as_ref()?.caps.as_ref().map(|c| c.encoding))).unwrap_or(Encoding::Utf16)
}

/// Development (`--test-actions accept`): Enter on the open completion list.
/// Test actions: the open completion list's labels, best first.
pub fn test_completions(view: &sourceview5::View) -> Vec<String> {
    let buffer: sourceview5::Buffer = view.buffer().downcast().expect("source buffer");
    with_doc(&buffer, |d| d.popup.shown_labels()).unwrap_or_default()
}

pub fn test_accept(view: &sourceview5::View) {
    let buffer: sourceview5::Buffer = view.buffer().downcast().expect("source buffer");
    if let Some(popup) = with_doc(&buffer, |d| d.popup.clone()).filter(|p| p.is_visible()) {
        popup.accept();
    }
}

/// What the completion list asks of the server and the buffer.
fn completion_hooks(buffer: &sourceview5::Buffer) -> crate::completion::Hooks {
    let (b0, b1, b2) = (buffer.downgrade(), buffer.downgrade(), buffer.downgrade());
    crate::completion::Hooks {
        can_resolve: Box::new(move || {
            let Some(buffer) = b0.upgrade() else { return false };
            HUB.with_borrow(|hub| {
                let d = hub.docs.iter().find(|d| d.buffer.upgrade().as_ref() == Some(&buffer))?;
                Some(hub.servers[d.servers[0]].as_ref()?.caps.as_ref()?.completion_resolve)
            })
            .unwrap_or(false)
        }),
        resolve: Box::new(move |raw, reply| {
            let Some(buffer) = b1.upgrade() else { return false };
            HUB.with_borrow_mut(|hub| {
                let Some(d) = hub.docs.iter().find(|d| d.buffer.upgrade().as_ref() == Some(&buffer)) else { return false };
                let Some(s) = hub.servers[d.servers[0]].as_mut().filter(|s| s.caps.as_ref().is_some_and(|c| c.completion_resolve)) else { return false };
                match s.conn.request("completionItem/resolve", raw.clone()) {
                    Ok(rid) => {
                        s.pending.insert(
                            rid,
                            Box::new(move |m: &Value| {
                                if let Some(item) = protocol::completion_item(&m["result"]) {
                                    reply(item);
                                }
                            }),
                        );
                        true
                    }
                    Err(e) => {
                        eprintln!("lsp: {e}");
                        false
                    }
                }
            })
        }),
        edits: Box::new(move |edits, encoding| {
            if let Some(buffer) = b2.upgrade() {
                apply_extra_edits(&buffer, edits, encoding);
            }
        }),
    }
}

/// An accepted completion's extra edits (ranges in the file, or in a notebook's
/// virtual file: each goes to the cell holding its lines). The cursor stays with its
/// text rather than its line number.
fn apply_extra_edits(buffer: &sourceview5::Buffer, edits: Vec<(protocol::Range, String)>, encoding: Encoding) {
    let Some(cells) = with_doc(buffer, |d| d.cells.clone()).flatten() else {
        replace_ranges(buffer, edits, encoding);
        return;
    };
    let placed: Vec<(sourceview5::Buffer, u32, u32)> = HUB.with_borrow(|hub| {
        hub.docs.iter().filter(|d| d.cells.as_ref().is_some_and(|c| Rc::ptr_eq(c, &cells))).filter_map(|d| Some((d.buffer.upgrade()?, d.offset, d.lines))).collect()
    });
    for (cell, offset, lines) in placed {
        replace_ranges(&cell, in_cell(&edits, offset, lines), encoding);
    }
}

/// The edits that fall inside a cell placed at `offset` (`lines` long), in its lines.
fn in_cell(edits: &[(protocol::Range, String)], offset: u32, lines: u32) -> Vec<(protocol::Range, String)> {
    edits
        .iter()
        .filter(|(r, _)| (offset..offset + lines).contains(&r.start.line) && r.end.line < offset + lines)
        .map(|(r, t)| {
            let mut r = *r;
            r.start.line -= offset;
            r.end.line -= offset;
            (r, t.clone())
        })
        .collect()
}

/// Sends `method` for the position `iter` if the buffer's server is ready and `offered`
/// says it supports it; `reply` gets the result. Returns whether it was sent.
fn request_at(buffer: &sourceview5::Buffer, iter: &gtk::TextIter, method: &str, offered: impl Fn(&Server) -> bool, reply: impl FnOnce(&Value) + 'static) -> bool {
    request_with(buffer, iter, method, offered, protocol::at, move |m| reply(&m["result"]))
}

/// `request_at` with other params (made from the file and the server-side position);
/// `reply` gets the whole reply, errors included.
fn request_with(buffer: &sourceview5::Buffer, iter: &gtk::TextIter, method: &str, offered: impl Fn(&Server) -> bool, params: impl FnOnce(&Path, Position) -> Value, reply: impl FnOnce(&Value) + 'static) -> bool {
    flush(buffer);
    HUB.with_borrow_mut(|hub| {
        let Some(d) = hub.docs.iter().find(|d| d.buffer.upgrade().as_ref() == Some(buffer)) else { return false };
        let (path, offset) = (d.path.clone(), d.offset);
        let Some(s) = hub.servers[d.servers[0]].as_mut().filter(|s| offered(s)) else { return false };
        let encoding = s.caps.as_ref().map_or(Encoding::Utf16, |c| c.encoding);
        let mut at = position_of(buffer.upcast_ref(), iter, encoding);
        at.line += offset;
        match s.conn.request(method, params(&path, at)) {
            Ok(rid) => {
                s.pending.insert(rid, Box::new(reply));
                true
            }
            Err(e) => {
                eprintln!("lsp: {e}");
                false
            }
        }
    })
}

fn with_doc<T>(buffer: &sourceview5::Buffer, f: impl FnOnce(&Doc) -> T) -> Option<T> {
    HUB.with_borrow(|hub| hub.docs.iter().find(|d| d.buffer.upgrade().as_ref() == Some(buffer)).map(f))
}

// ---- document sync ----

fn changed(buffer: &sourceview5::Buffer) {
    let b = buffer.downgrade();
    HUB.with_borrow_mut(|hub| {
        // A notebook syncs as a whole: one timer for all its cells.
        let group = hub.docs.iter().find(|d| d.buffer.upgrade().as_ref() == Some(buffer)).and_then(|d| d.cells.clone());
        if let Some(group) = group {
            for d in hub.docs.iter_mut().filter(|d| d.cells.as_ref().is_some_and(|c| Rc::ptr_eq(c, &group))) {
                if let Some(timer) = d.timer.take() {
                    timer.remove();
                }
            }
        }
        let Some(d) = hub.docs.iter_mut().find(|d| d.buffer.upgrade().as_ref() == Some(buffer)) else { return };
        d.hover = None;
        if let Some(timer) = d.timer.take() {
            timer.remove();
        }
        d.timer = Some(glib::timeout_add_local_once(CHANGE_DELAY, move || {
            if let Some(buffer) = b.upgrade() {
                HUB.with_borrow_mut(|hub| {
                    if let Some(d) = hub.docs.iter_mut().find(|d| d.buffer.upgrade().as_ref() == Some(&buffer)) {
                        d.timer = None;
                    }
                });
                sync_text(&buffer);
            }
        }));
    });
}

/// Sends a pending change now (before a request, so positions match the server's text).
fn flush(buffer: &sourceview5::Buffer) {
    // A notebook's pending sync may sit on another of its cells.
    let pending = HUB.with_borrow_mut(|hub| {
        let group = hub.docs.iter().find(|d| d.buffer.upgrade().as_ref() == Some(buffer))?.cells.clone();
        hub.docs.iter_mut().filter(|d| d.buffer.upgrade().as_ref() == Some(buffer) || group.as_ref().is_some_and(|g| d.cells.as_ref().is_some_and(|c| Rc::ptr_eq(c, g)))).find_map(|d| d.timer.take())
    });
    if let Some(timer) = pending {
        timer.remove();
        sync_text(buffer);
    }
}

fn sync_text(buffer: &sourceview5::Buffer) {
    let text = compose(buffer);
    HUB.with_borrow_mut(|hub| {
        let Some(d) = hub.docs.iter().find(|d| d.buffer.upgrade().as_ref() == Some(buffer)) else { return };
        let (path, ids) = (d.path.clone(), d.servers.clone());
        for id in ids {
            let Some(s) = hub.servers[id].as_mut() else { continue };
            let version = s.versions.entry(path.clone()).or_insert(1);
            *version += 1;
            let version = *version;
            send(s, protocol::did_change(&path, version, &text));
            pull_diagnostics(id, s, &path);
        }
    });
    schedule_hints(buffer, HINTS_DELAY);
}

/// The text the servers see for `buffer`: the buffer, or for a notebook cell every code
/// cell joined (`python_view`), which also places each cell (offset, line count).
fn compose(buffer: &sourceview5::Buffer) -> String {
    let Some(cells) = with_doc(buffer, |d| d.cells.clone()).flatten() else {
        return buffer.text(&buffer.start_iter(), &buffer.end_iter(), true).to_string();
    };
    let list = (cells.list)();
    let mut text = String::new();
    let mut placed = Vec::with_capacity(list.len());
    let mut line = 0;
    for (i, b) in list.iter().enumerate() {
        if i > 0 {
            text.push('\n');
        }
        text.push_str(&tileide_notebook::python_view(&b.text(&b.start_iter(), &b.end_iter(), true)));
        let lines = b.line_count() as u32;
        placed.push((b.clone(), line, lines));
        line += lines;
    }
    HUB.with_borrow_mut(|hub| {
        for d in hub.docs.iter_mut().filter(|d| d.cells.as_ref().is_some_and(|c| Rc::ptr_eq(c, &cells))) {
            if let Some((_, offset, lines)) = placed.iter().find(|(b, ..)| d.buffer.upgrade().as_ref() == Some(b)) {
                d.offset = *offset;
                d.lines = *lines;
            }
        }
    });
    text
}

/// A definition inside the notebook itself: the cell holding it gets the cursor.
fn reveal_in_cells(path: &Path, at: Position, encoding: Encoding) -> bool {
    let found = HUB.with_borrow(|hub| {
        hub.docs.iter().filter(|d| d.cells.is_some() && d.path == path).find(|d| (d.offset..d.offset + d.lines).contains(&at.line)).and_then(|d| Some((d.buffer.upgrade()?, d.view.upgrade()?, d.offset)))
    });
    let Some((buffer, view, offset)) = found else { return false };
    let iter = iter_at(buffer.upcast_ref(), Position { line: at.line - offset, column: at.column }, encoding);
    buffer.place_cursor(&iter);
    view.grab_focus();
    view.scroll_to_iter(&mut iter.clone(), 0.1, false, 0.0, 0.0);
    true
}

/// Completion while typing: trigger characters open it, word characters filter an
/// open list (or open one after two of them), anything else closes it.
fn typed(buffer: &sourceview5::Buffer, text: &str) {
    let Some((view, popup, triggers, signature_triggers, signature_open)) = HUB.with_borrow(|hub| {
        let d = hub.docs.iter().find(|d| d.buffer.upgrade().as_ref() == Some(buffer))?;
        let caps = hub.servers[d.servers[0]].as_ref()?.caps.as_ref()?;
        let open = d.signature.as_ref().is_some_and(|p| p.is_visible());
        Some((d.view.upgrade()?, d.popup.clone(), caps.triggers.clone(), if caps.signature { caps.signature_triggers.clone() } else { Vec::new() }, open))
    }) else {
        return;
    };
    let mut chars = text.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        popup.hide();
        if signature_open {
            signature(&view);
        }
        return;
    };
    if !view.has_focus() {
        return;
    }
    if signature_open || signature_triggers.contains(&c) {
        signature(&view);
    }
    if triggers.contains(&c) {
        complete(&view, Ask::Typing);
    } else if is_word_char(c) {
        if popup.is_visible() {
            if popup.refilter() && popup.incomplete() {
                complete(&view, Ask::Typing);
            }
        } else {
            let cursor = buffer.iter_at_mark(&buffer.get_insert());
            if cursor.line_offset() - word_start(&cursor).line_offset() >= 2 {
                complete(&view, Ask::Typing);
            }
        }
    } else {
        popup.hide();
    }
}

// ---- diagnostics ----

fn color_tags(buffer: &sourceview5::Buffer, theme: &Theme) {
    let rgba = |(r, g, b): theme::Rgb| gdk::RGBA::new(r as f32, g as f32, b as f32, 1.0);
    let table = buffer.tag_table();
    let unused = table.lookup(UNUSED).unwrap_or_else(|| {
        let tag = gtk::TextTag::new(Some(UNUSED));
        table.add(&tag);
        tag
    });
    unused.set_foreground_rgba(Some(&rgba(theme.muted)));
    let colours = [(theme.error, pango::Underline::Error), (theme.syntax().type_, pango::Underline::Error), (theme.muted, pango::Underline::Single)];
    for ((_, name, _), (colour, underline)) in KINDS.iter().zip(colours) {
        let tag = table.lookup(name).unwrap_or_else(|| {
            let tag = gtk::TextTag::new(Some(name));
            table.add(&tag);
            tag
        });
        tag.set_underline(underline);
        tag.set_underline_rgba(Some(&rgba(colour)));
    }
}

fn kind(severity: Severity) -> usize {
    match severity {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Info | Severity::Hint => 2,
    }
}

fn clear_marks(buffer: &sourceview5::Buffer) {
    let (start, end) = buffer.bounds();
    buffer.remove_tag_by_name(UNUSED, &start, &end);
    for (_, name, _) in KINDS {
        buffer.remove_tag_by_name(name, &start, &end);
        buffer.remove_source_marks(&start, &end, Some(name));
    }
}

fn show_diagnostics(server: usize, path: &Path, list: Vec<Diagnostic>) {
    let buffers: Vec<sourceview5::Buffer> = HUB.with_borrow_mut(|hub| {
        hub.docs
            .iter_mut()
            .filter(|d| d.servers.contains(&server) && d.path == path)
            .filter_map(|d| {
                let mine = match d.cells {
                    None => list.clone(),
                    Some(_) => list.iter().filter(|x| (d.offset..d.offset + d.lines).contains(&x.range.start.line) && !notebook_quiet(x)).map(|x| shift(x, d.offset)).collect(),
                };
                d.diagnostics.insert(server, mine);
                d.buffer.upgrade()
            })
            .collect()
    });
    for buffer in buffers {
        render_diagnostics(&buffer);
        // Diagnostics mean the server has analysed the file: the first hints can come.
        if with_doc(&buffer, |d| !d.hinted).unwrap_or(false) {
            schedule_hints(&buffer, Duration::ZERO);
        }
    }
}

fn notebook_quiet(d: &Diagnostic) -> bool {
    d.code.as_deref().is_some_and(|c| NOTEBOOK_QUIET.contains(&c)) || d.message.contains("allowed only within async function")
}

/// A diagnostic in the virtual file → the same one in a cell `offset` lines down.
fn shift(d: &Diagnostic, offset: u32) -> Diagnostic {
    let mut d = d.clone();
    d.range.start.line -= offset;
    d.range.end.line = d.range.end.line.saturating_sub(offset);
    d
}

/// Every server's diagnostics on `buffer`: underlines and the worst per line in the
/// gutter; unused code (a hint tagged unnecessary) is only faded.
fn render_diagnostics(buffer: &sourceview5::Buffer) {
    let Some(all) = with_doc(buffer, |d| d.diagnostics.iter().map(|(&server, list)| (server_encoding(server), list.clone())).collect::<Vec<_>>()) else { return };
    clear_marks(buffer);
    // Above the syntax colours, which the highlighter adds as it goes.
    let table = buffer.tag_table();
    if let Some(tag) = table.lookup(UNUSED) {
        tag.set_priority(table.size() - 1);
    }
    let mut worst: BTreeMap<i32, usize> = BTreeMap::new();
    for (encoding, list) in all {
        for d in &list {
            let mut start = iter_at(buffer.upcast_ref(), d.range.start, encoding);
            let mut end = iter_at(buffer.upcast_ref(), d.range.end, encoding);
            // An empty range still gets one visible character.
            if start == end && !end.forward_char() {
                start.backward_char();
            }
            if d.unnecessary {
                buffer.apply_tag_by_name(UNUSED, &start, &end);
                if d.severity == Severity::Hint {
                    continue;
                }
            }
            let k = kind(d.severity);
            buffer.apply_tag_by_name(KINDS[k].1, &start, &end);
            let entry = worst.entry(start.line()).or_insert(k);
            *entry = (*entry).min(k);
        }
    }
    for (line, k) in worst {
        if let Some(at) = buffer.iter_at_line(line) {
            buffer.create_source_mark(None, KINDS[k].1, &at);
        }
    }
}

fn severity_label(s: Severity) -> &'static str {
    match s {
        Severity::Error => "Error",
        Severity::Warning => "Warning",
        Severity::Info => "Info",
        Severity::Hint => "Hint",
    }
}

fn describe(d: &Diagnostic) -> String {
    match &d.source {
        Some(source) => format!("{}: {} ({source})", severity_label(d.severity), d.message),
        None => format!("{}: {}", severity_label(d.severity), d.message),
    }
}

fn mark_line(mark: &sourceview5::Mark) -> i32 {
    mark.buffer().map_or(0, |b| b.iter_at_mark(mark).line())
}

/// Every diagnostic on `line`, for the gutter icon's tooltip.
/// Every diagnostic on `buffer`, as `line: message` (headless checks).
pub fn all_messages(buffer: &sourceview5::Buffer) -> Vec<String> {
    with_doc(buffer, |d| d.diagnostics.values().flatten().map(|x| format!("{}: {}", x.range.start.line + 1, describe(x))).collect()).unwrap_or_default()
}

fn line_messages(buffer: &sourceview5::Buffer, line: i32) -> Option<String> {
    let list = with_doc(buffer, |d| d.diagnostics.values().flatten().filter(|x| x.range.start.line as i32 <= line && line <= x.range.end.line as i32).map(describe).collect::<Vec<_>>())?;
    (!list.is_empty()).then(|| list.join("\n"))
}

/// Diagnostics whose range covers `iter`.
fn messages_at(buffer: &sourceview5::Buffer, iter: &gtk::TextIter) -> Option<String> {
    let encoding = with_doc(buffer, |d| server_encoding(d.servers[0]))?;
    let here = position_of(buffer.upcast_ref(), iter, encoding);
    let list = with_doc(buffer, |d| d.diagnostics.values().flatten().filter(|x| x.range.start <= here && (here <= x.range.end || x.range.start == x.range.end && here.line == x.range.start.line)).map(describe).collect::<Vec<_>>())?;
    (!list.is_empty()).then(|| list.join("\n"))
}

// ---- hover ----

fn clip(text: &str) -> String {
    if text.chars().count() <= HOVER_MAX_CHARS {
        return text.to_string();
    }
    format!("{}…", text.chars().take(HOVER_MAX_CHARS).collect::<String>())
}

/// The mouse tooltip: diagnostics under the pointer, then the hover text. A hover not
/// asked yet is requested, and the tooltip re-queried when it arrives.
fn query_tooltip(view: &sourceview5::View, x: i32, y: i32, tooltip: &gtk::Tooltip) -> bool {
    let buffer: sourceview5::Buffer = view.buffer().downcast().expect("source buffer");
    let (bx, by) = view.window_to_buffer_coords(gtk::TextWindowType::Widget, x, y);
    let Some(iter) = view.iter_at_location(bx, by) else { return false };
    let problems = messages_at(&buffer, &iter);
    let hover = if is_word_char(iter.char()) {
        let start = word_start(&iter);
        let key = (start.line(), start.line_offset());
        match with_doc(&buffer, |d| d.hover.clone()).flatten() {
            Some((k, text)) if k == key => text,
            _ => {
                let (b, v) = (buffer.downgrade(), view.downgrade());
                request_at(&buffer, &iter, "textDocument/hover", |s| s.caps.as_ref().is_some_and(|c| c.hover), move |result| {
                    let text = hover_markup(result);
                    if let Some(buffer) = b.upgrade() {
                        HUB.with_borrow_mut(|hub| {
                            if let Some(d) = hub.docs.iter_mut().find(|d| d.buffer.upgrade().as_ref() == Some(&buffer)) {
                                d.hover = Some((key, text));
                            }
                        });
                    }
                    if let Some(view) = v.upgrade() {
                        view.trigger_tooltip_query();
                    }
                });
                None
            }
        }
    } else {
        None
    };
    let text = [problems.map(|p| escape(&p)), hover].into_iter().flatten().collect::<Vec<_>>().join("\n\n");
    if text.is_empty() {
        return false;
    }
    tooltip.set_markup(Some(&text));
    true
}

fn escape(text: &str) -> String {
    glib::markup_escape_text(text).to_string()
}

/// Hover contents as Pango markup: signatures in the code font, then the docs.
fn hover_markup(result: &Value) -> Option<String> {
    let parts: Vec<String> = protocol::hover_blocks(result)
        .into_iter()
        .map(|(code, text)| if code { format!("<tt>{}</tt>", escape(&clip(&text))) } else { escape(&clip(&text)) })
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// A small card at the cursor (keyboard hover). It doesn't take the keyboard (a grab
/// would hand focus to whatever window is under the mouse when it ends): the next key,
/// a click or leaving the editor closes it, and Esc is used up doing so.
/// `markup`: Pango markup (escape plain text first).
fn show_info(view: &sourceview5::View, markup: &str) {
    let buffer = view.buffer();
    let cursor = buffer.iter_at_mark(&buffer.get_insert());
    let label = gtk::Label::new(None);
    label.set_markup(markup);
    label.set_wrap(true);
    label.set_max_width_chars(80);
    label.set_xalign(0.0);
    let popover = gtk::Popover::new();
    popover.add_css_class("tileide-info");
    popover.set_has_arrow(false);
    popover.set_autohide(false);
    popover.set_can_focus(false);
    popover.set_child(Some(&label));
    popover.set_parent(view);
    popover.set_pointing_to(Some(&cursor_rect(view, &cursor)));

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let p = popover.downgrade();
    keys.connect_key_pressed(move |_, key, _, _| {
        if let Some(p) = p.upgrade() {
            p.popdown();
        }
        if key == gdk::Key::Escape { glib::Propagation::Stop } else { glib::Propagation::Proceed }
    });
    let click = gtk::GestureClick::new();
    let p = popover.downgrade();
    click.connect_pressed(move |_, _, _, _| {
        if let Some(p) = p.upgrade() {
            p.popdown();
        }
    });
    let focus = gtk::EventControllerFocus::new();
    let p = popover.downgrade();
    focus.connect_leave(move |_| {
        if let Some(p) = p.upgrade() {
            p.popdown();
        }
    });
    let controllers: [gtk::EventController; 3] = [keys.upcast(), click.upcast(), focus.upcast()];
    for c in &controllers {
        view.add_controller(c.clone());
    }
    let v = view.downgrade();
    popover.connect_closed(move |p| {
        let (p, v, controllers) = (p.clone(), v.clone(), controllers.clone());
        // After the close finishes, never during it.
        glib::idle_add_local_once(move || {
            if let Some(view) = v.upgrade() {
                for c in &controllers {
                    view.remove_controller(c);
                }
            }
            p.unparent();
        });
    });
    popover.popup();
}

// ---- signature help ----

/// Asks for the signature of the call around the cursor; the card shows it with the
/// current parameter in bold, or closes when the cursor is outside any call.
fn signature(view: &sourceview5::View) {
    let buffer: sourceview5::Buffer = view.buffer().downcast().expect("source buffer");
    let cursor = buffer.iter_at_mark(&buffer.get_insert());
    let Some(generation) = HUB.with_borrow_mut(|hub| {
        let d = hub.docs.iter_mut().find(|d| d.buffer.upgrade().as_ref() == Some(&buffer))?;
        d.signature_generation += 1;
        Some(d.signature_generation)
    }) else {
        return;
    };
    let b = buffer.downgrade();
    request_at(&buffer, &cursor, "textDocument/signatureHelp", |s| s.caps.as_ref().is_some_and(|c| c.signature), move |result| {
        let Some(buffer) = b.upgrade() else { return };
        if with_doc(&buffer, |d| d.signature_generation) != Some(generation) {
            return;
        }
        match protocol::signature_help(result) {
            Some(sig) => show_signature(&buffer, &sig),
            None => {
                hide_signature(&buffer);
            }
        }
    });
}

fn show_signature(buffer: &sourceview5::Buffer, sig: &protocol::Signature) {
    let chars: Vec<char> = sig.label.chars().collect();
    let piece = |a: usize, b: usize| escape(&chars[a.min(chars.len())..b.min(chars.len())].iter().collect::<String>());
    let mut markup = match sig.active {
        Some((a, b)) => format!("<tt>{}<b><u>{}</u></b>{}</tt>", piece(0, a), piece(a, b), piece(b, chars.len())),
        None => format!("<tt>{}</tt>", escape(&sig.label)),
    };
    if let Some(doc) = &sig.doc {
        markup.push_str(&format!("\n<small>{}</small>", escape(&clip(doc))));
    }
    let Some((view, popover)) = HUB.with_borrow_mut(|hub| {
        let d = hub.docs.iter_mut().find(|d| d.buffer.upgrade().as_ref() == Some(buffer))?;
        let view = d.view.upgrade()?;
        let popover = d
            .signature
            .get_or_insert_with(|| {
                let label = gtk::Label::new(None);
                label.set_wrap(true);
                label.set_max_width_chars(90);
                label.set_xalign(0.0);
                let popover = gtk::Popover::new();
                popover.add_css_class("tileide-info");
                popover.set_has_arrow(false);
                popover.set_autohide(false);
                popover.set_can_focus(false);
                popover.set_position(gtk::PositionType::Top);
                popover.set_child(Some(&label));
                // Unparented by `release_popovers` (see there).
                popover.set_parent(&view);
                popover
            })
            .clone();
        Some((view, popover))
    }) else {
        return;
    };
    if let Some(label) = popover.child().and_downcast::<gtk::Label>() {
        label.set_markup(&markup);
    }
    let cursor = buffer.iter_at_mark(&buffer.get_insert());
    popover.set_pointing_to(Some(&cursor_rect(&view, &cursor)));
    if !popover.is_visible() {
        popover.popup();
    }
}

/// Closes the signature card; returns whether it was open.
fn hide_signature(buffer: &sourceview5::Buffer) -> bool {
    let Some(popover) = with_doc(buffer, |d| d.signature.clone()).flatten().filter(|p| p.is_visible()) else { return false };
    popover.popdown();
    true
}

// ---- rename, references, quick fixes ----

/// The word around `iter` (empty when it isn't on one).
fn word_at(iter: &gtk::TextIter) -> String {
    let start = word_start(iter);
    let mut end = *iter;
    while is_word_char(end.char()) && end.forward_char() {}
    start.buffer().text(&start, &end, false).to_string()
}

/// `F2`: renames the symbol at the cursor everywhere the server knows of it. Open
/// editors change (unsaved); files that aren't open are rewritten on disk.
pub fn rename_at_cursor(view: &sourceview5::View) {
    let buffer: sourceview5::Buffer = view.buffer().downcast().expect("source buffer");
    let cursor = buffer.iter_at_mark(&buffer.get_insert());
    let word = word_at(&cursor);
    if word.is_empty() || !with_doc(&buffer, |d| hub_offers(d.servers[0], |c| c.rename)).unwrap_or(false) {
        show_info(view, if word.is_empty() { "Put the cursor on a name to rename it." } else { "This file's language server can't rename." });
        return;
    }
    let mark = buffer.create_mark(None, &cursor, true);
    let (b, v) = (buffer.downgrade(), view.downgrade());
    let (title, initial) = (format!("Rename {word}"), word.clone());
    crate::dialog::prompt(&title, "Every use the language server knows of changes. Files that aren't open are saved at once.", &initial, "Rename", move |name| {
        let (Some(buffer), Some(view)) = (b.upgrade(), v.upgrade()) else { return };
        let at = buffer.iter_at_mark(&mark);
        buffer.delete_mark(&mark);
        if let Some(name) = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty() && *n != word) {
            rename_to(&view, &at, &name);
        }
    });
}

/// Renames the symbol at `at` to `name` (F2 after the prompt).
pub fn rename_to(view: &sourceview5::View, at: &gtk::TextIter, name: &str) {
    let buffer: sourceview5::Buffer = view.buffer().downcast().expect("source buffer");
    let encoding = with_doc(&buffer, |d| server_encoding(d.servers[0])).unwrap_or(Encoding::Utf16);
    let v = view.downgrade();
    request_with(&buffer, at, "textDocument/rename", |s| s.caps.as_ref().is_some_and(|c| c.rename), |path, p| protocol::rename(path, p, name), move |m| {
        let Some(view) = v.upgrade() else { return };
        if let Some(message) = m["error"]["message"].as_str() {
            show_info(&view, &escape(&format!("Can't rename: {message}")));
            return;
        }
        match apply_workspace_edit(&m["result"], encoding) {
            0 => show_info(&view, "Nothing to rename here."),
            1 => {}
            n => notice(format!("Renamed in {n} files. Open ones have unsaved changes.")),
        }
    });
}

/// `Shift+F12`: every use of the symbol at the cursor, in a list; picking one goes there.
pub fn references_at_cursor(view: &sourceview5::View) {
    let buffer: sourceview5::Buffer = view.buffer().downcast().expect("source buffer");
    let cursor = buffer.iter_at_mark(&buffer.get_insert());
    let Some((goto, encoding, path, root)) = HUB.with_borrow(|hub| {
        let d = hub.docs.iter().find(|d| d.buffer.upgrade().as_ref() == Some(&buffer))?;
        Some((d.goto.clone(), server_encoding(d.servers[0]), d.path.clone(), hub.servers[d.servers[0]].as_ref()?.root.clone()))
    }) else {
        show_info(view, "No language server for this file.");
        return;
    };
    let word = word_at(&cursor);
    let v = view.downgrade();
    let sent = request_with(&buffer, &cursor, "textDocument/references", |s| s.caps.as_ref().is_some_and(|c| c.references), protocol::references, move |m| {
        let Some(view) = v.upgrade() else { return };
        let found = protocol::locations(&m["result"]);
        if found.is_empty() {
            show_info(&view, "No references found.");
            return;
        }
        let rows: Vec<String> = found.iter().map(|l| describe_location(l, &path, &root)).collect();
        let title = format!("{} reference{} to {word}", found.len(), if found.len() == 1 { "" } else { "s" });
        let path = path.clone();
        crate::pick_card::show(&view, &title, &rows, move |i| {
            let l = &found[i];
            if !(l.path == path && reveal_in_cells(&path, l.start, encoding)) {
                goto(l.path.clone(), l.start, encoding);
            }
        });
    });
    if !sent {
        show_info(view, "This file's language server can't list references.");
    }
}

/// `Ctrl+Shift+O`: the file's outline (in a notebook, every cell's), filtered by
/// typing; picking a name goes there.
pub fn outline_at_cursor(view: &sourceview5::View) {
    let buffer: sourceview5::Buffer = view.buffer().downcast().expect("source buffer");
    let Some((goto, encoding, path)) = with_doc(&buffer, |d| (d.goto.clone(), server_encoding(d.servers[0]), d.path.clone())) else {
        show_info(view, "No language server for this file.");
        return;
    };
    let v = view.downgrade();
    let sent = request_with(&buffer, &buffer.start_iter(), "textDocument/documentSymbol", |s| s.caps.as_ref().is_some_and(|c| c.symbols), |p, _| protocol::document(p), move |m| {
        let Some(view) = v.upgrade() else { return };
        let found = protocol::symbols(&m["result"]);
        if found.is_empty() {
            show_info(&view, "No names in this file.");
            return;
        }
        let rows: Vec<String> = found
            .iter()
            .map(|s| {
                let place = match cell_at(&path, s.start.line) {
                    Some((n, offset)) => format!("cell {n}:{}", s.start.line - offset + 1),
                    None => format!("{}", s.start.line + 1),
                };
                format!("{}{}  {} · {place}", "    ".repeat(s.depth), s.name, s.kind)
            })
            .collect();
        let keys = found.iter().map(|s| s.name.clone()).collect();
        let path = path.clone();
        crate::pick_card::show_filtered(&view, "Go to name", &rows, keys, move |i| {
            let at = found[i].start;
            if !reveal_in_cells(&path, at, encoding) {
                goto(path.clone(), at, encoding);
            }
        });
    });
    if !sent {
        show_info(view, "This file's language server has no outline.");
    }
}

/// The notebook cell (number from 1, first line in the virtual file) holding `line`.
fn cell_at(path: &Path, line: u32) -> Option<(usize, u32)> {
    let (cells, buffer, offset) = HUB.with_borrow(|hub| {
        hub.docs.iter().filter(|d| d.cells.is_some() && d.path == path).find(|d| (d.offset..d.offset + d.lines).contains(&line)).and_then(|d| Some((d.cells.as_ref()?.clone(), d.buffer.upgrade()?, d.offset)))
    })?;
    Some(((cells.list)().iter().position(|b| *b == buffer).map_or(0, |i| i + 1), offset))
}

/// "app.py:12  x = area(3)", or "cell 3:2  …" inside the notebook itself.
fn describe_location(l: &protocol::Location, doc_path: &Path, root: &Path) -> String {
    let line_of = |buffer: &sourceview5::Buffer, line: u32| line_text(buffer.upcast_ref(), line as i32).trim().to_string();
    if l.path == doc_path
        && let Some((n, offset)) = cell_at(doc_path, l.start.line)
        && let Some(buffer) = HUB.with_borrow(|hub| hub.docs.iter().find(|d| d.cells.is_some() && d.path == doc_path && d.offset == offset).and_then(|d| d.buffer.upgrade()))
    {
        return format!("cell {n}:{}  {}", l.start.line - offset + 1, line_of(&buffer, l.start.line - offset));
    }
    let name = l.path.strip_prefix(root).unwrap_or(&l.path).display().to_string();
    let open = HUB.with_borrow(|hub| hub.docs.iter().find(|d| d.path == l.path && d.cells.is_none()).and_then(|d| d.buffer.upgrade()));
    let text = match open {
        Some(buffer) => line_of(&buffer, l.start.line),
        None => std::fs::read_to_string(&l.path).ok().and_then(|t| t.lines().nth(l.start.line as usize).map(|s| s.trim().to_string())).unwrap_or_default(),
    };
    format!("{name}:{}  {text}", l.start.line + 1)
}

/// `Ctrl+.`: quick fixes and refactors for the cursor's line (or the selection), from
/// every server on the file (pyright and Ruff for Python); preferred fixes first.
pub fn code_actions_at_cursor(view: &sourceview5::View) {
    let buffer: sourceview5::Buffer = view.buffer().downcast().expect("source buffer");
    flush(&buffer);
    let (mut start, mut end) = buffer.selection_bounds().unwrap_or_else(|| {
        let cursor = buffer.iter_at_mark(&buffer.get_insert());
        (cursor, cursor)
    });
    start.set_line_offset(0);
    if !end.ends_line() {
        end.forward_to_line_end();
    }
    let (first, last) = (start.line() as u32, end.line() as u32);
    type Found = Rc<RefCell<(usize, Vec<(usize, protocol::Action)>)>>;
    let found: Found = Rc::new(RefCell::new((0, Vec::new())));
    let v = view.downgrade();
    let finish = move |found: &Found| {
        let Some(view) = v.upgrade() else { return };
        let mut actions = std::mem::take(&mut found.borrow_mut().1);
        actions.sort_by_key(|(_, a)| !a.preferred);
        if actions.is_empty() {
            show_info(&view, "No quick fixes here.");
            return;
        }
        let rows: Vec<String> = actions.iter().map(|(_, a)| a.title.clone()).collect();
        crate::pick_card::show(&view, "Quick fixes", &rows, move |i| run_action(actions[i].0, &actions[i].1));
    };
    let finish = Rc::new(finish);
    let asked = HUB.with_borrow_mut(|hub| {
        let Some(d) = hub.docs.iter().find(|d| d.buffer.upgrade().as_ref() == Some(&buffer)) else { return 0 };
        let (path, offset) = (d.path.clone(), d.offset);
        let ids: Vec<(usize, Vec<Value>)> = d.servers.iter().map(|&id| {
            // Each server gets back its own diagnostics on these lines.
            let mine = d.diagnostics.get(&id).into_iter().flatten().filter(|x| x.range.start.line <= last && x.range.end.line >= first).map(|x| x.raw.clone()).collect();
            (id, mine)
        }).collect();
        let mut asked = 0;
        for (id, diagnostics) in ids {
            let Some(s) = hub.servers[id].as_mut() else { continue };
            let Some(caps) = s.caps.as_ref().filter(|c| c.code_actions) else { continue };
            let encoding = caps.encoding;
            let mut range = protocol::Range { start: position_of(buffer.upcast_ref(), &start, encoding), end: position_of(buffer.upcast_ref(), &end, encoding) };
            range.start.line += offset;
            range.end.line += offset;
            let Ok(rid) = s.conn.request("textDocument/codeAction", protocol::code_action(&path, range, diagnostics)) else { continue };
            let (found, finish) = (found.clone(), finish.clone());
            s.pending.insert(rid, Box::new(move |m: &Value| {
                let done = {
                    let mut f = found.borrow_mut();
                    f.1.extend(protocol::code_actions(&m["result"]).into_iter().map(|a| (id, a)));
                    f.0 -= 1;
                    f.0 == 0
                };
                if done {
                    finish(&found);
                }
            }));
            asked += 1;
        }
        found.borrow_mut().0 = asked;
        asked
    });
    if asked == 0 {
        show_info(view, "This file's language server offers no quick fixes.");
    }
}

/// A picked code action: its edit, then its command (which may edit through
/// `workspace/applyEdit`).
fn run_action(server: usize, action: &protocol::Action) {
    if let Some(edit) = &action.edit {
        apply_workspace_edit(edit, server_encoding(server));
    }
    if let Some(command) = action.command.clone() {
        HUB.with_borrow_mut(|hub| {
            if let Some(s) = hub.servers.get_mut(server).and_then(Option::as_mut) {
                let _ = s.conn.request("workspace/executeCommand", command);
            }
        });
    }
}

fn hub_offers(server: usize, what: impl Fn(&Capabilities) -> bool) -> bool {
    HUB.with_borrow(|hub| hub.servers.get(server).and_then(|s| s.as_ref()?.caps.as_ref().map(&what))).unwrap_or(false)
}

fn notice(message: String) {
    HUB.with_borrow(|hub| notify(hub, message));
}

/// Applies a workspace edit: open editors (and notebook cells) change in place, other
/// files are rewritten. Returns how many files changed.
fn apply_workspace_edit(edit: &Value, encoding: Encoding) -> usize {
    let mut changed = 0;
    for (path, edits) in protocol::workspace_edit(edit) {
        if edits.is_empty() {
            continue;
        }
        let open: Vec<(sourceview5::Buffer, u32, u32, bool)> = HUB.with_borrow(|hub| hub.docs.iter().filter(|d| d.path == path).filter_map(|d| Some((d.buffer.upgrade()?, d.offset, d.lines, d.cells.is_some()))).collect());
        match open.first() {
            Some((_, _, _, true)) => {
                // Notebook cells: each edit goes to the cell holding its lines.
                for (buffer, offset, lines, _) in &open {
                    apply_edits(buffer, in_cell(&edits, *offset, *lines), encoding);
                }
            }
            Some((buffer, ..)) => apply_edits(buffer, edits, encoding),
            None => {
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                let edits: Vec<_> = edits.into_iter().map(|(r, t)| ((r.start.line, r.start.column), (r.end.line, r.end.column), t)).collect();
                if let Err(e) = std::fs::write(&path, text::apply_edits(&text, &edits, encoding)) {
                    notice(format!("Can't write {}: {e}", path.display()));
                    continue;
                }
            }
        }
        changed += 1;
    }
    changed
}

// ---- inlay hints ----

/// `editor.inlay_hints`: on asks every open file's server, off clears them.
pub fn set_inlay_hints(on: bool) {
    if INLAY_HINTS.replace(on) == on {
        return;
    }
    let buffers: Vec<sourceview5::Buffer> = HUB.with_borrow(|hub| hub.docs.iter().filter_map(|d| d.buffer.upgrade()).collect());
    for buffer in buffers {
        if on {
            schedule_hints(&buffer, Duration::ZERO);
        } else {
            HUB.with_borrow_mut(|hub| {
                if let Some(d) = hub.docs.iter_mut().find(|d| d.buffer.upgrade().as_ref() == Some(&buffer)) {
                    d.hints.remove_all();
                    d.hinted = false;
                    d.hint_texts.clear();
                }
            });
        }
    }
}

/// Asks for hints after `delay`, again from the start if asked meanwhile; for a
/// notebook cell, every cell (one change can retype names in all of them).
fn schedule_hints(buffer: &sourceview5::Buffer, delay: Duration) {
    if !INLAY_HINTS.get() {
        return;
    }
    let buffers: Vec<sourceview5::Buffer> = HUB.with_borrow(|hub| {
        let Some(d) = hub.docs.iter().find(|d| d.buffer.upgrade().as_ref() == Some(buffer)) else { return Vec::new() };
        match &d.cells {
            Some(cells) => hub.docs.iter().filter(|o| o.cells.as_ref().is_some_and(|c| Rc::ptr_eq(c, cells))).filter_map(|o| o.buffer.upgrade()).collect(),
            None => vec![buffer.clone()],
        }
    });
    for buffer in buffers {
        let b = buffer.downgrade();
        let timer = glib::timeout_add_local_once(delay, move || {
            if let Some(buffer) = b.upgrade() {
                HUB.with_borrow_mut(|hub| {
                    if let Some(d) = hub.docs.iter_mut().find(|d| d.buffer.upgrade().as_ref() == Some(&buffer)) {
                        d.hints_timer = None;
                    }
                });
                request_hints(&buffer);
            }
        });
        HUB.with_borrow_mut(|hub| {
            if let Some(d) = hub.docs.iter_mut().find(|d| d.buffer.upgrade().as_ref() == Some(&buffer)) {
                if let Some(old) = d.hints_timer.replace(timer) {
                    old.remove();
                }
            }
        });
    }
}

/// `textDocument/inlayHint` over the buffer's lines (a cell's part of the notebook).
fn request_hints(buffer: &sourceview5::Buffer) {
    let b = buffer.downgrade();
    HUB.with_borrow_mut(|hub| {
        let Some(d) = hub.docs.iter().find(|d| d.buffer.upgrade().as_ref() == Some(buffer)) else { return };
        let (path, offset, lines) = (d.path.clone(), d.offset, buffer.line_count() as u32);
        // Any server on the file that offers them (the main one first).
        let Some(id) = d.servers.iter().copied().find(|&id| hub.servers[id].as_ref().and_then(|s| s.caps.as_ref()).is_some_and(|c| c.inlay_hints)) else { return };
        let Some(s) = hub.servers[id].as_mut() else { return };
        let encoding = s.caps.as_ref().map_or(Encoding::Utf16, |c| c.encoding);
        // The buffer's real end: rust-analyzer refuses a range past the last line.
        let end = position_of(buffer.upcast_ref(), &buffer.end_iter(), encoding);
        let range = protocol::Range { start: Position { line: offset, column: 0 }, end: Position { line: offset + end.line, column: end.column } };
        let Ok(rid) = s.conn.request("textDocument/inlayHint", protocol::inlay_hint(&path, range)) else { return };
        s.pending.insert(
            rid,
            Box::new(move |m: &Value| {
                let Some(buffer) = b.upgrade() else { return };
                // ContentModified: rust-analyzer answers this while it loads the project.
                if m["error"]["code"] == -32801 {
                    schedule_hints(&buffer, Duration::from_secs(1));
                    return;
                }
                let hints = protocol::inlay_hints(&m["result"]).into_iter().filter(|h| h.at.line >= offset && h.at.line < offset + lines).map(|mut h| {
                    h.at.line -= offset;
                    h
                });
                show_hints(&buffer, hints.collect(), encoding);
            }),
        );
    });
}

fn show_hints(buffer: &sourceview5::Buffer, hints: Vec<protocol::InlayHint>, encoding: Encoding) {
    let Some(provider) = HUB.with_borrow_mut(|hub| {
        let d = hub.docs.iter_mut().find(|d| d.buffer.upgrade().as_ref() == Some(buffer))?;
        // A server still indexing answers with none: the next diagnostics ask again.
        d.hinted = !hints.is_empty();
        Some(d.hints.clone())
    }) else {
        return;
    };
    provider.remove_all();
    if !INLAY_HINTS.get() {
        return;
    }
    let mut by_line: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for h in &hints {
        let at = iter_at(buffer.upcast_ref(), h.at, encoding);
        by_line.entry(h.at.line).or_default().push(hint_text(&line_text(buffer.upcast_ref(), at.line()), at.line_offset() as usize, h));
    }
    let shown: Vec<(u32, String)> = by_line.into_iter().map(|(line, texts)| (line, texts.join("  "))).collect();
    for (line, text) in &shown {
        provider.add_annotation(&sourceview5::Annotation::new(Some(text), None::<gtk::gio::Icon>, *line as i32, sourceview5::AnnotationStyle::None));
    }
    HUB.with_borrow_mut(|hub| {
        if let Some(d) = hub.docs.iter_mut().find(|d| d.buffer.upgrade().as_ref() == Some(buffer)) {
            d.hint_texts = shown;
        }
    });
}

/// Test actions: the hints shown in `buffer`, as "line: text" (lines from 1).
pub fn hint_lines(buffer: &sourceview5::Buffer) -> Vec<String> {
    with_doc(buffer, |d| d.hint_texts.iter().map(|(l, t)| format!("{}: {t}", l + 1)).collect()).unwrap_or_default()
}

/// A hint as it reads at the end of its line: `x: int` (the name before it), `-> str`,
/// `a=10` (the argument after a parameter name, cut short; also C's designators,
/// `.width=2.0`).
fn hint_text(line: &str, at: usize, hint: &protocol::InlayHint) -> String {
    let chars: Vec<char> = line.chars().collect();
    let at = at.min(chars.len());
    let designator = hint.label.starts_with(['.', '[']) && hint.label.ends_with('=');
    if hint.parameter || designator {
        let name = hint.label.trim_end_matches([':', '=']).trim();
        let mut depth = 0i32;
        let arg: String = chars[at..]
            .iter()
            .take_while(|&&c| {
                match c {
                    '(' | '[' | '{' => depth += 1,
                    ')' | ']' | '}' if depth == 0 => return false,
                    ')' | ']' | '}' => depth -= 1,
                    ',' if depth == 0 => return false,
                    _ => {}
                }
                true
            })
            .collect();
        let arg = arg.trim();
        let arg = if arg.chars().count() > 16 { format!("{}…", arg.chars().take(15).collect::<String>()) } else { arg.to_string() };
        return format!("{name}={arg}");
    }
    if hint.label.starts_with(':') {
        let start = chars[..at].iter().rposition(|&c| !is_word_char(c)).map_or(0, |i| i + 1);
        let word: String = chars[start..at].iter().collect();
        return format!("{word}{}", hint.label);
    }
    hint.label.clone()
}

#[cfg(test)]
mod hint_tests {
    use super::*;

    fn hint(label: &str, parameter: bool) -> protocol::InlayHint {
        protocol::InlayHint { at: Position { line: 0, column: 0 }, label: label.into(), parameter }
    }

    #[test]
    fn hints_read_with_their_name_or_argument() {
        assert_eq!(hint_text("total = add(1, 2)", 5, &hint(": int", false)), "total: int");
        assert_eq!(hint_text("total = add(1, f(2, 3))", 15, &hint("b=", true)), "b=f(2, 3)");
        assert_eq!(hint_text("add(\"a very long string here\")", 4, &hint("text:", true)), "text=\"a very long st…");
        assert_eq!(hint_text("def f(x)", 8, &hint("-> int", false)), "-> int");
        assert_eq!(hint_text("Rect box = { 2.0, 3.0 };", 13, &hint(".width=", false)), ".width=2.0");
    }
}

// ---- formatting ----

/// `Ctrl+Shift+I`: formats the file with the first server offering it (for Python,
/// Ruff); in a notebook, the current cell (by range).
pub fn format(view: &sourceview5::View) {
    format_with(view, None);
}

/// Saving with `editor.format_on_save`: organizes imports, then formats, then `done`
/// (also when there's nothing to do, or after 3 s if a server doesn't answer).
pub fn tidy(view: &sourceview5::View, done: Box<dyn FnOnce()>) {
    let once = Rc::new(RefCell::new(Some(done)));
    let finish: Then = {
        let once = once.clone();
        Rc::new(move || {
            if let Some(done) = once.borrow_mut().take() {
                done();
            }
        })
    };
    let fallback = finish.clone();
    glib::timeout_add_local_once(Duration::from_secs(3), move || fallback());
    let v = view.downgrade();
    organize_imports(view, Box::new(move || match v.upgrade() {
        Some(view) => format_with(&view, Some(finish)),
        None => finish(),
    }));
}

/// Runs once a step of `tidy` is over.
type Then = Rc<dyn Fn()>;

/// The first server's "organize imports" action (Ruff's), applied; then `then`.
fn organize_imports(view: &sourceview5::View, then: Box<dyn FnOnce()>) {
    let buffer: sourceview5::Buffer = view.buffer().downcast().expect("source buffer");
    flush(&buffer);
    let then = Rc::new(RefCell::new(Some(then)));
    let next = {
        let then = then.clone();
        move || {
            if let Some(f) = then.borrow_mut().take() {
                f();
            }
        }
    };
    let next2 = next.clone();
    let asked = HUB.with_borrow_mut(|hub| {
        let d = hub.docs.iter().find(|d| d.buffer.upgrade().as_ref() == Some(&buffer))?;
        if d.cells.is_some() {
            return None;
        }
        let path = d.path.clone();
        // Linters come after the main server: Ruff sorts imports, pyright doesn't.
        let id = d.servers.iter().copied().rev().find(|&id| hub.servers[id].as_ref().and_then(|s| s.caps.as_ref()).is_some_and(|c| c.code_actions))?;
        let s = hub.servers[id].as_mut()?;
        let encoding = s.caps.as_ref().map_or(Encoding::Utf16, |c| c.encoding);
        let range = protocol::Range { start: Position { line: 0, column: 0 }, end: position_of(buffer.upcast_ref(), &buffer.end_iter(), encoding) };
        let rid = s.conn.request("textDocument/codeAction", protocol::source_action(&path, range, "source.organizeImports")).ok()?;
        println!("lsp: organize imports via {}", s.program);
        s.pending.insert(
            rid,
            Box::new(move |m: &Value| {
                if let Some(action) = protocol::code_actions(&m["result"]).into_iter().find(|a| a.edit.is_some()) {
                    run_action(id, &action);
                }
                next2();
            }),
        );
        Some(())
    });
    if asked.is_none() {
        next();
    }
}

/// `format`, then `then` once the edits are in (or right away if it can't).
fn format_with(view: &sourceview5::View, then: Option<Then>) {
    let quiet = then.is_some();
    let then_now = then.clone();
    let buffer: sourceview5::Buffer = view.buffer().downcast().expect("source buffer");
    flush(&buffer);
    let (tab, spaces) = (view.tab_width(), view.is_insert_spaces_instead_of_tabs());
    let source = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string();
    let b = buffer.downgrade();
    let v = view.downgrade();
    let outcome = HUB.with_borrow_mut(|hub| {
        let d = hub.docs.iter().find(|d| d.buffer.upgrade().as_ref() == Some(&buffer)).ok_or("No language server for this file.")?;
        let (path, offset, cell) = (d.path.clone(), d.offset, d.cells.is_some());
        // In a cell, IPython lines are blank to the servers: range formatting would drop
        // them, so Ruff formats the cell on its own with them masked.
        if cell && tileide_notebook::python_view(&source).lines().zip(source.lines()).any(|(py, src)| py != src && !src.trim().is_empty()) {
            let ruff = d.servers.iter().find_map(|&id| hub.servers[id].as_ref().filter(|s| s.spec.name == "ruff")).ok_or("Cells with % or ! lines need Ruff to format (pacman -S ruff).")?;
            let (masked, taken) = tileide_notebook::mask_ipython(&source).ok_or("A %% cell isn't Python: not formatted.")?;
            let command = vec![ruff.program.clone(), "format".into(), "--stdin-filename".into(), "cell.py".into(), "-".into()];
            return Ok(Some(Piped { command, label: "Ruff", root: ruff.root.clone(), text: masked, taken }));
        }
        if !cell
            && let Some(f) = servers::formatter_for(&path)
            && let Some(program) = servers::locate(f.command[0], &[], &servers::user_dirs(std::env::var_os("HOME").map(PathBuf::from).as_deref()))
        {
            let root = hub.servers[d.servers[0]].as_ref().map_or_else(|| path.parent().unwrap_or(Path::new("/")).to_path_buf(), |s| s.root.clone());
            let command = std::iter::once(program).chain(f.command[1..].iter().map(|a| a.to_string())).collect();
            return Ok(Some(Piped { command, label: f.label, root, text: source.clone(), taken: Vec::new() }));
        }
        let wanted = |c: &Capabilities| if cell { c.range_formatting } else { c.formatting };
        // A linter that formats (Ruff, Biome) before the main server: it's there by choice.
        let id = d.servers.iter().rev().copied().find(|&id| hub.servers[id].as_ref().and_then(|s| s.caps.as_ref()).is_some_and(wanted)).ok_or("No formatter for this file. For Python: install Ruff (pacman -S ruff).")?;
        let s = hub.servers[id].as_mut().ok_or("No formatter for this file.")?;
        let encoding = s.caps.as_ref().map_or(Encoding::Utf16, |c| c.encoding);
        let params = if cell {
            let end = buffer.end_iter();
            let mut range = protocol::Range { start: Position { line: 0, column: 0 }, end: position_of(buffer.upcast_ref(), &end, encoding) };
            range.start.line += offset;
            range.end.line += offset;
            protocol::range_formatting(&path, range, tab, spaces)
        } else {
            protocol::formatting(&path, tab, spaces)
        };
        let method = if cell { "textDocument/rangeFormatting" } else { "textDocument/formatting" };
        let rid = s.conn.request(method, params).map_err(|_| "The formatter isn't answering.")?;
        s.pending.insert(
            rid,
            Box::new(move |m: &Value| {
                let done = || {
                    if let Some(then) = &then {
                        then();
                    }
                };
                let (Some(buffer), Some(view)) = (b.upgrade(), v.upgrade()) else { return done() };
                if let Some(message) = m["error"]["message"].as_str() {
                    show_info(&view, &escape(&format!("Can't format: {message}")));
                    return done();
                }
                // Edits come in the notebook's virtual file: back to this cell's lines.
                let edits = protocol::text_edits(&m["result"])
                    .into_iter()
                    .filter(|(r, _)| r.start.line >= offset)
                    .map(|(mut r, t)| {
                        r.start.line -= offset;
                        r.end.line -= offset;
                        (r, t)
                    })
                    .collect();
                apply_edits(&buffer, edits, encoding);
                view.scroll_mark_onscreen(&buffer.get_insert());
                done();
            }),
        );
        Ok(None)
    });
    if let Ok(Some(piped)) = outcome {
        return format_piped(view, piped, then_now);
    }
    if let Err(message) = outcome {
        // Saving shouldn't nag about a file no formatter knows.
        if !quiet {
            show_info(view, message);
        }
        if let Some(then) = then_now {
            then();
        }
    }
}

/// A cell with IPython lines: `ruff format` on the masked cell (from the project's
/// folder, so its settings apply), then the lines back and the cell replaced.
/// A formatter run as a command: `text` on its stdin (IPython lines masked as `taken`
/// for Ruff on a cell), the result on its stdout.
struct Piped {
    command: Vec<String>,
    label: &'static str,
    root: PathBuf,
    text: String,
    taken: Vec<String>,
}

fn format_piped(view: &sourceview5::View, piped: Piped, then: Option<Then>) {
    let Piped { command, label, root, text, taken } = piped;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(pipe_format(&command, label, &root, &text));
    });
    let v = view.downgrade();
    glib::timeout_add_local(Duration::from_millis(20), move || {
        let formatted = match rx.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
            Ok(formatted) => formatted,
            Err(_) => Err(format!("{label} didn't answer.")),
        };
        if let Some(view) = v.upgrade() {
            let buffer = view.buffer();
            let source = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string();
            let restored = formatted.and_then(|f| if taken.is_empty() { Ok(f) } else { tileide_notebook::unmask_ipython(&f, &taken).ok_or_else(|| format!("{label} moved a % or ! line: not formatted.")) });
            match restored {
                Ok(mut text) => {
                    if !source.ends_with('\n') && text.ends_with('\n') {
                        text.pop();
                    }
                    if text != source {
                        let end = position_of(&buffer, &buffer.end_iter(), Encoding::Utf32);
                        apply_edits(view.buffer().downcast_ref().expect("source buffer"), vec![(protocol::Range { start: Position { line: 0, column: 0 }, end }, text)], Encoding::Utf32);
                    }
                }
                Err(message) if then.is_none() => show_info(&view, &escape(&message)),
                Err(_) => {}
            }
        }
        if let Some(then) = &then {
            then();
        }
        glib::ControlFlow::Break
    });
}

fn pipe_format(command: &[String], label: &str, root: &Path, source: &str) -> Result<String, String> {
    use std::io::Write;
    let mut child = std::process::Command::new(&command[0])
        .args(&command[1..])
        .current_dir(root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Can't run {label}: {e}"))?;
    child.stdin.take().map(|mut i| i.write_all(source.as_bytes()));
    let out = child.wait_with_output().map_err(|e| format!("{label}: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("Can't format: {}", err.lines().find(|l| !l.trim().is_empty()).unwrap_or("it failed")));
    }
    String::from_utf8(out.stdout).map_err(|_| format!("{label} gave back invalid text."))
}

/// Applies edits as one undo step, last first so earlier positions stay valid; the
/// cursor keeps its line and column.
fn apply_edits(buffer: &sourceview5::Buffer, edits: Vec<(protocol::Range, String)>, encoding: Encoding) {
    if edits.is_empty() {
        return;
    }
    let cursor = buffer.iter_at_mark(&buffer.get_insert());
    let (line, column) = (cursor.line(), cursor.line_offset());
    replace_ranges(buffer, edits, encoding);
    let line = line.min(buffer.line_count() - 1);
    let iter = buffer.iter_at_line(line).map(|mut it| {
        let mut end = it;
        if !end.ends_line() {
            end.forward_to_line_end();
        }
        it.set_line_offset(column.min(end.line_offset()));
        it
    });
    if let Some(iter) = iter {
        buffer.place_cursor(&iter);
    }
}

/// Applies `edits` (ranges in the current text) as one undo step.
fn replace_ranges(buffer: &sourceview5::Buffer, mut edits: Vec<(protocol::Range, String)>, encoding: Encoding) {
    if edits.is_empty() {
        return;
    }
    // From the end; edits sharing a start go last-listed first (all ranges refer to
    // the original text, so an insert listed before a delete at the same place stays).
    let mut edits: Vec<(usize, (protocol::Range, String))> = edits.drain(..).enumerate().collect();
    edits.sort_by_key(|(i, e)| std::cmp::Reverse((e.0.start, *i)));
    buffer.begin_user_action();
    for (_, (range, text)) in edits {
        let mut start = iter_at(buffer.upcast_ref(), range.start, encoding);
        let mut end = iter_at(buffer.upcast_ref(), range.end, encoding);
        buffer.delete(&mut start, &mut end);
        buffer.insert(&mut start, &text);
    }
    buffer.end_user_action();
}

/// Where `iter` is drawn, in the view's widget coordinates.
pub fn cursor_rect(view: &sourceview5::View, iter: &gtk::TextIter) -> gdk::Rectangle {
    let r = view.iter_location(iter);
    let (x, y) = view.buffer_to_window_coords(gtk::TextWindowType::Widget, r.x(), r.y());
    gdk::Rectangle::new(x, y, 1, r.height())
}

pub fn css(t: &Theme) -> String {
    let c = theme::css;
    format!(
        "tooltip {{ background-color: {surface}; color: {text}; border: 1px solid {edge}; border-radius: 10px; }}
tooltip label {{ padding: 2px 4px; }}
popover.tileide-info > contents {{ background-color: {surface}; color: {text}; border: 1px solid {edge}; border-radius: 12px; padding: 8px 12px; box-shadow: 0 10px 30px rgba(0, 0, 0, 0.3); }}",
        surface = c(t.surface, 0.98),
        text = c(t.text, 1.0),
        edge = c(t.text, 0.12),
    )
}
