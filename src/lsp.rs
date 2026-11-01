// SPDX-License-Identifier: GPL-3.0-or-later
//! Language servers in the editor (v0.3 phase 6). One server runs per workspace folder
//! and language, started when the first matching file opens; a workspace without a
//! folder starts none. Server threads hand messages to the main loop
//! (`MainContext::invoke`), so nothing polls and the UI never waits on a server.
//!
//! Shown: diagnostics (a squiggle plus a gutter icon, the message on hover: never by
//! colour alone), hover text (mouse tooltip, or `Alt+I` at the cursor), completion
//! (`completion.rs`) and go to definition (`F12`, Ctrl+click).

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
/// Tag and mark category per severity (hints share the info look).
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

struct Doc {
    buffer: glib::WeakRef<sourceview5::Buffer>,
    view: glib::WeakRef<sourceview5::View>,
    path: PathBuf,
    server: usize,
    goto: Goto,
    popup: Rc<Popup>,
    timer: Option<glib::SourceId>,
    diagnostics: Vec<Diagnostic>,
    /// The hover for the word starting at (line, offset), once asked (`None` = nothing to show).
    hover: Option<((i32, i32), Option<String>)>,
    /// Bumped per completion request, so a late reply never replaces a newer one.
    completion_generation: u64,
    handlers: Vec<glib::SignalHandlerId>,
    view_handlers: Vec<(glib::WeakRef<gtk::Widget>, glib::SignalHandlerId)>,
    controllers: Vec<gtk::EventController>,
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
            None => warnings.push(format!("lsp: unknown server \"{name}\" (built in: rust, c, python, typescript, java)")),
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
    detach(buffer);
    let (Some(root), Some((spec, language))) = (root, servers::for_file(path)) else { return };
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let Some(server) = HUB.with_borrow_mut(|hub| start(hub, root, spec)) else { return };

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
    let popup = Popup::new(view);

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

    let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true).to_string();
    HUB.with_borrow_mut(|hub| {
        let first = !hub.docs.iter().any(|d| d.server == server && d.path == path);
        hub.docs.push(Doc {
            buffer: buffer.downgrade(),
            view: view.downgrade(),
            path: path.clone(),
            server,
            goto,
            popup,
            timer: None,
            diagnostics: Vec::new(),
            hover: None,
            completion_generation: 0,
            handlers,
            view_handlers,
            controllers: vec![click.upcast()],
        });
        if first && let Some(s) = hub.servers[server].as_mut() {
            let version = *s.versions.entry(path.clone()).or_insert(1);
            send(s, protocol::did_open(&path, language, version, &text));
        }
    });
}

/// Ends language support for `buffer` (tile closed or replaced, file renamed).
pub fn detach(buffer: &sourceview5::Buffer) {
    let Some(doc) = HUB.with_borrow_mut(|hub| {
        let i = hub.docs.iter().position(|d| d.buffer.upgrade().as_ref() == Some(buffer))?;
        let doc = hub.docs.remove(i);
        let last = !hub.docs.iter().any(|d| d.server == doc.server && d.path == doc.path);
        if last && let Some(s) = hub.servers[doc.server].as_mut() {
            s.versions.remove(&doc.path);
            send(s, protocol::did_close(&doc.path));
        }
        Some(doc)
    }) else {
        return;
    };
    if let Some(timer) = doc.timer {
        timer.remove();
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
        if let Some(s) = hub.servers[d.server].as_mut() {
            send(s, protocol::did_save(&path));
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
            .filter(|(i, s)| s.as_ref().is_some_and(|s| !roots.contains(&s.root) && !docs.iter().any(|d| d.server == *i)))
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
        let hover = protocol::hover_text(result);
        let text = [problems, hover.map(|h| clip(&h))].into_iter().flatten().collect::<Vec<_>>().join("\n\n");
        show_info(&view, if text.is_empty() { "Nothing to show here." } else { &text });
    });
    if !shown {
        let text = messages_at(&buffer, &cursor).unwrap_or_else(|| "No language server for this file.".into());
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
    let Some((goto, encoding)) = with_doc(&buffer, |d| (d.goto.clone(), server_encoding(d.server))) else { return };
    request_at(&buffer, iter, "textDocument/definition", |s| s.caps.as_ref().is_some_and(|c| c.definition), move |result| {
        if let Some(location) = protocol::definition(result) {
            goto(location.path, location.start, encoding);
        }
    });
}

/// Completion at the cursor: `Ctrl+Space` (`manual`), a trigger character, or typing.
pub fn complete(view: &sourceview5::View, manual: bool) {
    let buffer: sourceview5::Buffer = view.buffer().downcast().expect("source buffer");
    let cursor = buffer.iter_at_mark(&buffer.get_insert());
    let anchor = word_start(&cursor);
    let Some((generation, popup, encoding)) = HUB.with_borrow_mut(|hub| {
        let d = hub.docs.iter_mut().find(|d| d.buffer.upgrade().as_ref() == Some(&buffer))?;
        d.completion_generation += 1;
        Some((d.completion_generation, d.popup.clone(), hub.servers[d.server].as_ref()?.caps.as_ref()?.encoding))
    }) else {
        return;
    };
    let b = buffer.downgrade();
    let anchor_mark = buffer.create_mark(None, &anchor, true);
    request_at(&buffer, &cursor, "textDocument/completion", |s| s.caps.as_ref().is_some_and(|c| c.completion), move |result| {
        let Some(buffer) = b.upgrade() else { return };
        let current = with_doc(&buffer, |d| d.completion_generation) == Some(generation);
        let anchor = buffer.iter_at_mark(&anchor_mark);
        buffer.delete_mark(&anchor_mark);
        if !current {
            return;
        }
        let (items, incomplete) = protocol::completions(result);
        if items.is_empty() {
            if manual {
                popup.hide();
            }
            return;
        }
        popup.show(items, incomplete, &anchor, encoding);
    });
}

/// The editor position as LSP's, in `encoding`.
pub fn position_of(buffer: &gtk::TextBuffer, iter: &gtk::TextIter, encoding: Encoding) -> Position {
    let line = iter.line();
    Position { line: line as u32, column: text::to_lsp(&line_text(buffer, line), iter.line_offset() as usize, encoding) }
}

/// LSP's position as an editor iterator, clamped to the buffer.
pub fn iter_at(buffer: &gtk::TextBuffer, p: Position, encoding: Encoding) -> gtk::TextIter {
    let line = (p.line as i32).min(buffer.line_count() - 1).max(0);
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
    let Some(cmd) = servers::command(spec, setting.and_then(|s| s.cmd.as_deref()), servers::on_path) else {
        hub.failed.insert(key);
        notify(hub, format!("No {} language server found. To get one: {}.", spec.label, spec.install));
        return None;
    };
    let id = hub.servers.len();
    let deliver = move |message: Incoming| glib::MainContext::default().invoke(move || receive(id, message));
    let mut conn = match Connection::spawn(&cmd, root, deliver) {
        Ok(conn) => conn,
        Err(e) => {
            hub.failed.insert(key);
            notify(hub, format!("{} language server: {e}. {}.", spec.label, spec.install));
            return None;
        }
    };
    let Ok(request) = conn.request("initialize", protocol::initialize_params(root)) else {
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
}

fn exited(id: usize) {
    let buffers = HUB.with_borrow_mut(|hub| {
        let Some(s) = hub.servers.get_mut(id).and_then(Option::take) else { return Vec::new() };
        hub.failed.insert((s.root.clone(), s.spec.name));
        let message = if s.caps.is_none() {
            format!("{} language server ({}) exited while starting. To get one: {}.", s.spec.label, s.program, s.spec.install)
        } else {
            format!("{} language server ({}) stopped. It restarts when the config changes.", s.spec.label, s.program)
        };
        notify(hub, message);
        hub.docs.iter_mut().filter(|d| d.server == id).filter_map(|d| {
            d.diagnostics.clear();
            d.buffer.upgrade()
        }).collect()
    });
    for buffer in buffers {
        clear_marks(&buffer);
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

/// Sends `method` for the position `iter` if the buffer's server is ready and `offered`
/// says it supports it; `reply` gets the result. Returns whether it was sent.
fn request_at(buffer: &sourceview5::Buffer, iter: &gtk::TextIter, method: &str, offered: impl Fn(&Server) -> bool, reply: impl FnOnce(&Value) + 'static) -> bool {
    flush(buffer);
    HUB.with_borrow_mut(|hub| {
        let Some(d) = hub.docs.iter().find(|d| d.buffer.upgrade().as_ref() == Some(buffer)) else { return false };
        let path = d.path.clone();
        let Some(s) = hub.servers[d.server].as_mut().filter(|s| offered(s)) else { return false };
        let encoding = s.caps.as_ref().map_or(Encoding::Utf16, |c| c.encoding);
        let params = protocol::at(&path, position_of(buffer.upcast_ref(), iter, encoding));
        match s.conn.request(method, params) {
            Ok(rid) => {
                s.pending.insert(rid, Box::new(move |m: &Value| reply(&m["result"])));
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
    let pending = HUB.with_borrow_mut(|hub| hub.docs.iter_mut().find(|d| d.buffer.upgrade().as_ref() == Some(buffer)).and_then(|d| d.timer.take()));
    if let Some(timer) = pending {
        timer.remove();
        sync_text(buffer);
    }
}

fn sync_text(buffer: &sourceview5::Buffer) {
    let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true).to_string();
    HUB.with_borrow_mut(|hub| {
        let Some(d) = hub.docs.iter().find(|d| d.buffer.upgrade().as_ref() == Some(buffer)) else { return };
        let path = d.path.clone();
        let Some(s) = hub.servers[d.server].as_mut() else { return };
        let version = s.versions.entry(path.clone()).or_insert(1);
        *version += 1;
        let version = *version;
        send(s, protocol::did_change(&path, version, &text));
    });
}

/// Completion while typing: trigger characters open it, word characters filter an
/// open list (or open one after two of them), anything else closes it.
fn typed(buffer: &sourceview5::Buffer, text: &str) {
    let Some((view, popup, triggers)) = HUB.with_borrow(|hub| {
        let d = hub.docs.iter().find(|d| d.buffer.upgrade().as_ref() == Some(buffer))?;
        let triggers = hub.servers[d.server].as_ref()?.caps.as_ref()?.triggers.clone();
        Some((d.view.upgrade()?, d.popup.clone(), triggers))
    }) else {
        return;
    };
    let mut chars = text.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        popup.hide();
        return;
    };
    if !view.has_focus() {
        return;
    }
    if triggers.contains(&c) {
        complete(&view, false);
    } else if is_word_char(c) {
        if popup.is_visible() {
            if popup.refilter() && popup.incomplete() {
                complete(&view, false);
            }
        } else {
            let cursor = buffer.iter_at_mark(&buffer.get_insert());
            if cursor.line_offset() - word_start(&cursor).line_offset() >= 2 {
                complete(&view, false);
            }
        }
    } else {
        popup.hide();
    }
}

// ---- diagnostics ----

fn color_tags(buffer: &sourceview5::Buffer, theme: &Theme) {
    let rgba = |(r, g, b): theme::Rgb| gdk::RGBA::new(r as f32, g as f32, b as f32, 1.0);
    let colours = [(theme.error, pango::Underline::Error), (theme.syntax().type_, pango::Underline::Error), (theme.muted, pango::Underline::Single)];
    let table = buffer.tag_table();
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
    for (_, name, _) in KINDS {
        buffer.remove_tag_by_name(name, &start, &end);
        buffer.remove_source_marks(&start, &end, Some(name));
    }
}

fn show_diagnostics(server: usize, path: &Path, list: Vec<Diagnostic>) {
    let encoding = server_encoding(server);
    let buffers: Vec<sourceview5::Buffer> = HUB.with_borrow_mut(|hub| {
        hub.docs
            .iter_mut()
            .filter(|d| d.server == server && d.path == path)
            .filter_map(|d| {
                d.diagnostics = list.clone();
                d.buffer.upgrade()
            })
            .collect()
    });
    for buffer in buffers {
        clear_marks(&buffer);
        let mut worst: BTreeMap<i32, usize> = BTreeMap::new();
        for d in &list {
            let start = iter_at(buffer.upcast_ref(), d.range.start, encoding);
            let mut end = iter_at(buffer.upcast_ref(), d.range.end, encoding);
            let mut start = start;
            // An empty range still gets one visible character.
            if start == end && !end.forward_char() {
                start.backward_char();
            }
            let k = kind(d.severity);
            buffer.apply_tag_by_name(KINDS[k].1, &start, &end);
            let entry = worst.entry(start.line()).or_insert(k);
            *entry = (*entry).min(k);
        }
        for (line, k) in worst {
            if let Some(at) = buffer.iter_at_line(line) {
                buffer.create_source_mark(None, KINDS[k].1, &at);
            }
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
fn line_messages(buffer: &sourceview5::Buffer, line: i32) -> Option<String> {
    let list = with_doc(buffer, |d| d.diagnostics.iter().filter(|x| x.range.start.line as i32 <= line && line <= x.range.end.line as i32).map(describe).collect::<Vec<_>>())?;
    (!list.is_empty()).then(|| list.join("\n"))
}

/// Diagnostics whose range covers `iter`.
fn messages_at(buffer: &sourceview5::Buffer, iter: &gtk::TextIter) -> Option<String> {
    let encoding = with_doc(buffer, |d| server_encoding(d.server))?;
    let here = position_of(buffer.upcast_ref(), iter, encoding);
    let list = with_doc(buffer, |d| d.diagnostics.iter().filter(|x| x.range.start <= here && (here <= x.range.end || x.range.start == x.range.end && here.line == x.range.start.line)).map(describe).collect::<Vec<_>>())?;
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
                    let text = protocol::hover_text(result).map(|t| clip(&t));
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
    let text = [problems, hover].into_iter().flatten().collect::<Vec<_>>().join("\n\n");
    if text.is_empty() {
        return false;
    }
    tooltip.set_text(Some(&text));
    true
}

/// A small card at the cursor (keyboard hover). It doesn't take the keyboard (a grab
/// would hand focus to whatever window is under the mouse when it ends): the next key,
/// a click or leaving the editor closes it, and Esc is used up doing so.
fn show_info(view: &sourceview5::View, text: &str) {
    let buffer = view.buffer();
    let cursor = buffer.iter_at_mark(&buffer.get_insert());
    let label = gtk::Label::new(Some(text));
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
