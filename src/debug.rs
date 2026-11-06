// SPDX-License-Identifier: GPL-3.0-or-later
//! Debugging in the editor (v0.4 phase 4). `F9` toggles a breakpoint (a dot in the
//! gutter), `F5` builds the focused file with debug info and starts its debugger (or
//! continues), `F10`/`F11`/`Shift+F11` step over/into/out, `Shift+F5` stops. A debug
//! tile next to the code shows where the program is (call stack), its variables
//! (click to expand) and its output; the paused line is highlighted in the editor.
//! Adapter messages reach the main loop through `MainContext::invoke`, like the LSP hub.
//! A notebook cell is debugged in its running kernel (`debug_cells`): debugpy listens
//! there, and each cell's breakpoints go to the file the kernel compiles it as.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::{gdk, glib};
use gtk4 as gtk;
use sourceview5::prelude::*;
use tileide_core::theme::{self, Theme};
use tileide_dap::{Incoming, Plan, Session};
use tileide_lsp::{Value, json};

const BREAKPOINT: &str = "tileide-breakpoint";
const PAUSED_TAG: &str = "tileide-paused-line";

pub fn css(t: &Theme) -> String {
    let c = theme::css;
    format!(
        ".debug {{ padding: 6px 4px 8px 4px; }}
.debug .bar {{ padding: 4px 10px 6px 14px; }}
.debug .title {{ font-weight: 600; font-size: 13px; }}
.debug .state {{ font-size: 12px; border-radius: 10px; padding: 2px 10px; background-color: {chip}; color: {muted}; }}
.debug .state.paused {{ background-color: {accent_bg}; color: {accent}; }}
.debug .state.failed {{ background-color: {error_bg}; color: {error}; }}
.debug button.icon {{ min-width: 26px; min-height: 26px; padding: 0; border: none; box-shadow: none; background: transparent; color: {muted}; border-radius: 13px; }}
.debug button.icon:hover {{ background-color: {hover}; color: {text}; }}
.debug .section {{ font-weight: 600; color: {muted}; font-size: 11px; letter-spacing: 1px; margin: 10px 14px 4px 14px; }}
.debug list, .debug scrolledwindow, .debug textview, .debug textview text {{ background: transparent; }}
.debug row {{ border-radius: 8px; padding: 3px 10px; color: {text}; font-size: 13px; }}
.debug row:selected {{ background-color: {select}; color: {text}; }}
.debug .var-value {{ font-family: monospace; color: {muted}; }}
.debug .hint {{ color: {muted}; font-size: 12px; margin: 2px 14px; }}",
        text = c(t.text, 1.0),
        muted = c(t.muted, 1.0),
        chip = c(t.text, 0.07),
        hover = c(t.text, 0.06),
        select = c(t.accent, 0.16),
        accent = c(t.accent, 1.0),
        accent_bg = c(t.accent, 0.16),
        error = c(t.error, 1.0),
        error_bg = c(t.error, 0.16),
    )
}

/// Opens a file at a 1-based line in the active workspace; returns its editor.
pub type ShowLine = Rc<dyn Fn(&Path, u32) -> Option<(sourceview5::View, sourceview5::Buffer)>>;

pub struct Hooks {
    pub show_line: ShowLine,
    /// Shows the debug tile in the active workspace (if it isn't there yet).
    pub show_panel: Rc<dyn Fn(&gtk::Widget)>,
}

type Reply = Box<dyn FnOnce(&Value)>;

struct Running {
    session: Session,
    pending: HashMap<i64, Reply>,
    thread: Option<i64>,
    generation: u64,
    /// Attached to a notebook kernel's debugpy at this address.
    kernel: Option<String>,
    /// Runs once `configurationDone` is answered (the cell to debug).
    on_configured: Option<Box<dyn FnOnce()>>,
}

/// A notebook cell, under the file its kernel runs it as.
struct CellFile {
    /// The cell's breakpoint key (`cell_key`).
    key: PathBuf,
    label: String,
    view: glib::WeakRef<sourceview5::View>,
    buffer: glib::WeakRef<sourceview5::Buffer>,
}

#[derive(Default)]
struct Hub {
    /// Breakpoint marks per file (the marks move with edits; deleted ones are skipped).
    marks: BTreeMap<PathBuf, Vec<sourceview5::Mark>>,
    /// Breakpoint lines of files not open in any editor.
    saved: BTreeMap<PathBuf, BTreeSet<u32>>,
    running: Option<Running>,
    generation: u64,
    panel: Option<Rc<Panel>>,
    hooks: Option<Rc<Hooks>>,
    /// The buffer whose line is highlighted as paused.
    paused: Option<sourceview5::Buffer>,
    accent: Option<gdk::RGBA>,
    /// By the kernel's file for the cell.
    cells: HashMap<PathBuf, CellFile>,
}

thread_local! {
    static HUB: RefCell<Hub> = RefCell::new(Hub::default());
}

struct Panel {
    root: gtk::Box,
    title: gtk::Label,
    state: gtk::Label,
    frames: gtk::ListBox,
    /// One frame per `frames` row: (id, path, line).
    frame_rows: RefCell<Vec<(i64, Option<PathBuf>, u32)>>,
    vars: gtk::ListBox,
    /// One entry per `vars` row: (variablesReference, depth, expanded).
    var_rows: RefCell<Vec<(i64, usize, bool)>>,
    output: gtk::TextView,
}

pub fn init(hooks: Hooks) {
    HUB.with_borrow_mut(|h| h.hooks = Some(Rc::new(hooks)));
}

pub fn set_theme(t: &Theme) {
    let rgba = gdk::RGBA::new(t.accent.0 as f32, t.accent.1 as f32, t.accent.2 as f32, 0.18);
    HUB.with_borrow_mut(|h| h.accent = Some(rgba));
}

fn hooks() -> Option<Rc<Hooks>> {
    HUB.with_borrow(|h| h.hooks.clone())
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// An editor opened `path`: its gutter shows breakpoints, and saved ones come back.
pub fn attach(buffer: &sourceview5::Buffer, view: &sourceview5::View, path: &Path) {
    let attrs = sourceview5::MarkAttributes::new();
    attrs.set_icon_name("media-record-symbolic");
    view.set_mark_attributes(BREAKPOINT, &attrs, 40);
    view.set_show_line_marks(true);
    let path = canonical(path);
    let lines = HUB.with_borrow_mut(|h| h.saved.remove(&path)).unwrap_or_default();
    let marks: Vec<sourceview5::Mark> = lines.into_iter().filter_map(|line| buffer.iter_at_line(line as i32 - 1).map(|it| buffer.create_source_mark(None, BREAKPOINT, &it))).collect();
    HUB.with_borrow_mut(|h| h.marks.entry(path).or_default().extend(marks));
}

/// The 1-based breakpoint lines of every file.
fn breakpoints() -> BTreeMap<PathBuf, BTreeSet<u32>> {
    HUB.with_borrow_mut(|h| {
        let mut all = h.saved.clone();
        for (path, marks) in &mut h.marks {
            marks.retain(|m| !m.is_deleted());
            let lines: BTreeSet<u32> = marks
                .iter()
                .filter_map(|m| {
                    let buffer = m.buffer()?;
                    Some(buffer.iter_at_mark(m).line() as u32 + 1)
                })
                .collect();
            all.entry(path.clone()).or_default().extend(lines);
        }
        all
    })
}

/// `F9`: a breakpoint on the cursor's line, or off if there is one.
pub fn toggle_breakpoint(view: &sourceview5::View, path: &Path) {
    let Some(buffer) = view.buffer().downcast::<sourceview5::Buffer>().ok() else { return };
    let line = buffer.iter_at_mark(&buffer.get_insert()).line();
    let path = canonical(path);
    if buffer.source_marks_at_line(line, Some(BREAKPOINT)).is_empty() {
        if let Some(it) = buffer.iter_at_line(line) {
            let mark = buffer.create_source_mark(None, BREAKPOINT, &it);
            HUB.with_borrow_mut(|h| h.marks.entry(path.clone()).or_default().push(mark));
        }
    } else if let (Some(start), Some(mut end)) = (buffer.iter_at_line(line), buffer.iter_at_line(line)) {
        end.forward_to_line_end();
        buffer.remove_source_marks(&start, &end, Some(BREAKPOINT));
    }
    send_breakpoints(&path);
}

fn send_breakpoints(path: &Path) {
    let lines: Vec<Value> = breakpoints().get(path).into_iter().flatten().map(|l| json!({ "line": l })).collect();
    // A cell's breakpoints go to its kernel file (none known: it isn't being debugged).
    let target = if is_cell_key(path) { HUB.with_borrow(|h| h.cells.iter().find(|(_, c)| c.key == path).map(|(file, _)| file.clone())) } else { Some(path.to_path_buf()) };
    if let Some(target) = target {
        request("setBreakpoints", json!({ "source": { "path": target }, "breakpoints": lines }), |_| {});
    }
}

/// What a notebook cell's breakpoints are kept under (F9 in a cell).
pub fn cell_key(notebook: &Path, cell: u64) -> PathBuf {
    let mut key = canonical(notebook).into_os_string();
    key.push(format!("#cell-{cell}"));
    PathBuf::from(key)
}

fn is_cell_key(path: &Path) -> bool {
    path.to_string_lossy().contains(".ipynb#cell-")
}

/// One cell for `debug_cells`.
pub struct Cell {
    /// The file the kernel runs this cell's current code as.
    pub file: PathBuf,
    pub key: PathBuf,
    pub label: String,
    pub view: sourceview5::View,
    pub buffer: sourceview5::Buffer,
}

/// Debugs in a notebook's kernel, whose debugpy listens at `address`: attaches (once
/// per kernel), sets the cells' breakpoints, then `run`s the cell.
pub fn debug_cells(address: &str, title: &str, cells: Vec<Cell>, run: Box<dyn FnOnce()>) {
    HUB.with_borrow_mut(|h| {
        // A cell's file changes with its code: the old one is forgotten.
        h.cells.retain(|_, c| !cells.iter().any(|n| n.key == c.key));
        for c in &cells {
            h.cells.insert(c.file.clone(), CellFile { key: c.key.clone(), label: c.label.clone(), view: c.view.downgrade(), buffer: c.buffer.downgrade() });
        }
    });
    let attached = HUB.with_borrow(|h| h.running.as_ref().map(|r| r.kernel.as_deref() == Some(address)));
    if attached == Some(true) {
        for c in &cells {
            send_breakpoints(&c.key);
        }
        // Answered after the breakpoints (requests are handled in order).
        request("threads", json!({}), move |_| run());
        return;
    }
    if attached == Some(false) {
        stop();
    }
    let shown = panel();
    if let Some(h) = hooks() {
        (h.show_panel)(shown.root.upcast_ref());
    }
    shown.clear();
    shown.title.set_text(&format!("Debug · {title}"));
    let generation = HUB.with_borrow_mut(|h| {
        h.generation += 1;
        h.generation
    });
    let deliver = move |m: Incoming| {
        glib::MainContext::default().invoke(move || receive(generation, m));
    };
    let session = match Session::connect(address, deliver) {
        Ok(s) => s,
        Err(e) => {
            shown.set_state("Can't debug", Some("failed"));
            shown.append(&format!("{e}\n"));
            return;
        }
    };
    HUB.with_borrow_mut(|h| h.running = Some(Running { session, pending: HashMap::new(), thread: None, generation, kernel: Some(address.to_string()), on_configured: Some(run) }));
    shown.set_state("Attaching to the kernel…", None);
    request("initialize", tileide_dap::initialize_args(), move |reply| {
        if reply["success"] == false {
            fail(&format!("the debugger refused to start: {}", reply["message"].as_str().unwrap_or("?")));
            return;
        }
        request("attach", json!({ "justMyCode": true }), |reply| {
            if reply["success"] == false {
                fail(&format!("attach failed: {}", reply["message"].as_str().unwrap_or("?")));
            }
        });
    });
}

/// Sends a request to the running adapter; `reply` gets the response.
fn request(command: &str, arguments: Value, reply: impl FnOnce(&Value) + 'static) {
    HUB.with_borrow_mut(|h| {
        let Some(r) = h.running.as_mut() else { return };
        match r.session.request(command, arguments) {
            Ok(seq) => {
                r.pending.insert(seq, Box::new(reply));
            }
            Err(e) => eprintln!("debug: {command}: {e}"),
        }
    });
}

pub fn is_running() -> bool {
    HUB.with_borrow(|h| h.running.is_some())
}

pub fn is_paused() -> bool {
    HUB.with_borrow(|h| h.running.as_ref().is_some_and(|r| r.thread.is_some()))
}

fn panel() -> Rc<Panel> {
    if let Some(p) = HUB.with_borrow(|h| h.panel.clone()) {
        return p;
    }
    let p = Rc::new(Panel::new());
    HUB.with_borrow_mut(|h| h.panel = Some(p.clone()));
    p
}

/// Why a debug session didn't start.
enum Failure {
    /// No debugger or toolchain for the file (also an alert).
    Missing(String),
    Build(String),
}

/// `F5`: continues a paused program, else builds and debugs `file` (in the workspace
/// `folder`, where its Cargo project and virtualenv are looked for), or the picked
/// launch config's program (`picked`, Alt+L; or why it can't be used).
pub fn start_or_continue(file: Option<PathBuf>, folder: Option<PathBuf>, picked: Option<Result<tileide_core::launch::Resolved, String>>) {
    if is_paused() {
        step("continue");
        return;
    }
    if is_running() {
        return;
    }
    let shown = panel();
    if let Some(h) = hooks() {
        (h.show_panel)(shown.root.upcast_ref());
    }
    shown.clear();
    let config = match picked.transpose() {
        Ok(config) => config,
        Err(e) => {
            shown.set_state("Can't debug", Some("failed"));
            shown.append(&format!("{e}\n"));
            return;
        }
    };
    // The launch config's program stands in for the focused file.
    let Some(file) = config.as_ref().map(|l| l.program.clone()).or(file) else {
        shown.set_state("Nothing to debug", Some("failed"));
        shown.append("Focus a C, C++, Rust, Python, Java, JavaScript or TypeScript file and press F5.\n");
        return;
    };
    let name = config.as_ref().map_or_else(|| file.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()), |l| l.name.clone());
    shown.title.set_text(&format!("Debug · {name}"));
    shown.set_state("Building…", None);
    // Planning (it asks Python about debugpy) and building run off the main thread.
    let out_dir = glib::user_runtime_dir().join("tileide-run");
    let (tx, rx) = std::sync::mpsc::channel::<Result<(Plan, String), Failure>>();
    std::thread::spawn(move || {
        let _ = std::fs::create_dir_all(&out_dir);
        let dir = file.parent().unwrap_or(std::path::Path::new("/")).to_path_buf();
        let host = tileide_dap::Host {
            found: &tileide_core::run::on_path,
            pythons: tileide_core::python::candidates(&dir, folder.as_deref()),
            python_has: &tileide_core::python::imports,
            bridge: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("tileide")),
            folder,
            node: node(&dir),
            js_debug: tileide_dap::js_debug(std::env::var_os("HOME").map(PathBuf::from).as_deref()),
        };
        let plan = match &config {
            Some(config) => tileide_dap::plan_launch(config, &out_dir, &host),
            None => tileide_dap::plan(&file, &out_dir, &host),
        };
        let result = plan.map_err(Failure::Missing).and_then(|plan| {
            let mut log = String::new();
            for step in &plan.build {
                log += &format!("$ {}\n", step.join(" "));
                let out = std::process::Command::new(&step[0]).args(&step[1..]).current_dir(&plan.cwd).output().map_err(|e| Failure::Build(format!("cannot run {}: {e}", step[0])))?;
                log += &tileide_core::run::strip_ansi(&String::from_utf8_lossy(&out.stderr));
                if !out.status.success() {
                    return Err(Failure::Build(log + "\nThe build failed; fix the errors above and press F5 again.\n"));
                }
            }
            Ok((plan, log))
        });
        let _ = tx.send(result);
    });
    glib::timeout_add_local(std::time::Duration::from_millis(40), move || match rx.try_recv() {
        Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(_) => glib::ControlFlow::Break,
        Ok(Err(failure)) => {
            let p = panel();
            p.set_state("Can't debug", Some("failed"));
            match failure {
                Failure::Missing(message) => {
                    p.append(&format!("{message}\n"));
                    crate::alerts::missing(message);
                }
                Failure::Build(log) => p.append(&log),
            }
            glib::ControlFlow::Break
        }
        Ok(Ok((plan, log))) => {
            let p = panel();
            p.append(&log);
            launch(plan);
            glib::ControlFlow::Break
        }
    });
}

/// A `node` that runs in `dir`: PATH's first (a mise shim fails outside a project
/// that pins one), else one from the user's tool folders.
fn node(dir: &Path) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
    let user = tileide_lsp::servers::user_dirs(std::env::var_os("HOME").map(PathBuf::from).as_deref());
    path.iter().chain(&user).map(|d| d.join("node")).filter(|n| n.is_file()).find(|n| {
        std::process::Command::new(n).arg("--version").current_dir(dir).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success())
    })
}

fn launch(plan: Plan) {
    let generation = HUB.with_borrow_mut(|h| {
        h.generation += 1;
        h.generation
    });
    let deliver = move |m: Incoming| {
        glib::MainContext::default().invoke(move || receive(generation, m));
    };
    let started = if plan.listens { Session::serve(&plan.cmd, &plan.cwd, deliver) } else { Session::start(&plan.cmd, &plan.cwd, deliver) };
    let session = match started {
        Ok(s) => s,
        Err(e) => {
            let p = panel();
            p.set_state("Can't debug", Some("failed"));
            p.append(&format!("{e}\n"));
            return;
        }
    };
    HUB.with_borrow_mut(|h| h.running = Some(Running { session, pending: HashMap::new(), thread: None, generation, kernel: None, on_configured: None }));
    panel().set_state(&format!("Starting {}…", plan.adapter), None);
    let launch_args = plan.launch.clone();
    request("initialize", tileide_dap::initialize_args(), move |reply| {
        if reply["success"] == false {
            fail(&format!("the debugger refused to start: {}", reply["message"].as_str().unwrap_or("?")));
            return;
        }
        // gdb answers `launch` only after `configurationDone` (sent on `initialized`).
        request("launch", launch_args, |reply| {
            if reply["success"] == false {
                fail(&format!("launch failed: {}", reply["message"].as_str().unwrap_or("?")));
            }
        });
    });
}

fn fail(message: &str) {
    let p = panel();
    p.set_state("Failed", Some("failed"));
    p.append(&format!("{message}\n"));
    stop();
}

/// `Shift+F5`: ends the program and the debugger (a kernel just runs on).
pub fn stop() {
    clear_paused();
    if let Some(r) = HUB.with_borrow_mut(|h| h.running.take()) {
        let state = if r.kernel.is_some() { "Detached" } else { "Stopped" };
        r.session.stop();
        // Its later events are ignored (no session): the panel says it here.
        let p = panel();
        p.frames.remove_all();
        p.vars.remove_all();
        p.frame_rows.borrow_mut().clear();
        p.var_rows.borrow_mut().clear();
        if ["Running", "Paused", "Starting", "Attaching"].iter().any(|s| p.state.text().starts_with(s)) {
            p.set_state(state, None);
        }
    }
}

/// A notebook cell finished: an attached kernel session waits for the next one.
pub fn kernel_idle() {
    if HUB.with_borrow(|h| h.running.as_ref().is_some_and(|r| r.kernel.is_some() && r.thread.is_none())) {
        panel().set_state("Attached · F5 in a cell debugs it", None);
    }
}

/// `continue`, `next` (over), `stepIn`, `stepOut` on the paused thread.
pub fn step(command: &str) {
    let Some(thread) = HUB.with_borrow(|h| h.running.as_ref().and_then(|r| r.thread)) else { return };
    request(command, json!({ "threadId": thread }), |_| {});
    resumed();
}

fn resumed() {
    HUB.with_borrow_mut(|h| {
        if let Some(r) = h.running.as_mut() {
            r.thread = None;
        }
    });
    clear_paused();
    let p = panel();
    p.set_state("Running", None);
    p.frames.remove_all();
    p.vars.remove_all();
    p.frame_rows.borrow_mut().clear();
    p.var_rows.borrow_mut().clear();
}

fn receive(generation: u64, incoming: Incoming) {
    if HUB.with_borrow(|h| h.running.as_ref().map(|r| r.generation)) != Some(generation) {
        return;
    }
    let message = match incoming {
        Incoming::Message(m) => m,
        Incoming::Exited => {
            HUB.with_borrow_mut(|h| h.running.take());
            clear_paused();
            let p = panel();
            if p.state.text().starts_with("Running") || p.state.text().starts_with("Paused") || p.state.text().starts_with("Starting") {
                p.set_state("Debugger ended", None);
            }
            return;
        }
    };
    if message["type"] == "response" {
        let reply = HUB.with_borrow_mut(|h| h.running.as_mut().and_then(|r| message["request_seq"].as_i64().and_then(|s| r.pending.remove(&s))));
        if let Some(reply) = reply {
            reply(&message);
        }
        return;
    }
    let body = &message["body"];
    match message["event"].as_str() {
        Some("initialized") => {
            for path in breakpoints().keys() {
                send_breakpoints(path);
            }
            request("setExceptionBreakpoints", json!({ "filters": [] }), |_| {});
            request("configurationDone", json!({}), |_| {
                if let Some(run) = HUB.with_borrow_mut(|h| h.running.as_mut().and_then(|r| r.on_configured.take())) {
                    run();
                }
            });
            panel().set_state("Running", None);
        }
        Some("output") if body["category"] != "telemetry" => panel().append(body["output"].as_str().unwrap_or("")),
        Some("stopped") => {
            let thread = body["threadId"].as_i64();
            HUB.with_borrow_mut(|h| {
                if let Some(r) = h.running.as_mut() {
                    r.thread = thread;
                }
            });
            let reason = body["reason"].as_str().unwrap_or("paused").to_string();
            request("stackTrace", json!({ "threadId": thread, "levels": 30 }), move |reply| {
                let frames = reply["body"]["stackFrames"].as_array().cloned().unwrap_or_default();
                let p = panel();
                p.show_frames(&frames);
                let line = frames.first().and_then(|f| f["line"].as_u64()).unwrap_or(0);
                p.set_state(&format!("Paused · {reason} · line {line}"), Some("paused"));
                select_frame(0);
            });
        }
        Some("continued") => resumed(),
        Some("exited") => {
            let code = body["exitCode"].as_i64().unwrap_or(0);
            panel().set_state(&format!("Exited {code}"), (code != 0).then_some("failed"));
        }
        Some("terminated") => {
            let p = panel();
            if !p.state.text().starts_with("Exited") {
                p.set_state("Ended", None);
            }
            stop();
        }
        _ => {}
    }
}

/// Shows frame `index` in the editor and loads its variables.
fn select_frame(index: usize) {
    let p = panel();
    let Some((id, path, line)) = p.frame_rows.borrow().get(index).cloned() else { return };
    if let Some(row) = p.frames.row_at_index(index as i32) {
        p.frames.select_row(Some(&row));
    }
    let cell = path.as_ref().and_then(|p| HUB.with_borrow(|h| h.cells.get(p).and_then(|c| Some((c.view.upgrade()?, c.buffer.upgrade()?)))));
    if let Some((view, buffer)) = cell {
        view.grab_focus();
        mark_paused(&view, &buffer, line);
    } else if let (Some(path), Some(h)) = (path, hooks())
        && let Some((view, buffer)) = (h.show_line)(&path, line)
    {
        mark_paused(&view, &buffer, line);
    }
    request("scopes", json!({ "frameId": id }), |reply| {
        let p = panel();
        p.vars.remove_all();
        p.var_rows.borrow_mut().clear();
        let scopes = reply["body"]["scopes"].as_array().cloned().unwrap_or_default();
        for (i, scope) in scopes.iter().enumerate() {
            let reference = scope["variablesReference"].as_i64().unwrap_or(0);
            let row = p.var_row(scope["name"].as_str().unwrap_or("Scope"), "", reference > 0, 0, true);
            p.vars.append(&row);
            p.var_rows.borrow_mut().push((reference, 0, false));
            // The first scope (locals) opens by itself.
            if i == 0 && reference > 0 {
                expand(0);
            }
        }
    });
}

/// Loads the children of variable row `index` under it, or folds them away.
fn expand(index: usize) {
    let p = panel();
    let Some((reference, depth, expanded)) = p.var_rows.borrow().get(index).copied() else { return };
    if reference <= 0 {
        return;
    }
    if expanded {
        let mut rows = p.var_rows.borrow_mut();
        let end = rows[index + 1..].iter().position(|r| r.1 <= depth).map_or(rows.len(), |n| index + 1 + n);
        for _ in index + 1..end {
            if let Some(row) = p.vars.row_at_index(index as i32 + 1) {
                p.vars.remove(&row);
            }
        }
        rows.drain(index + 1..end);
        rows[index].2 = false;
        return;
    }
    p.var_rows.borrow_mut()[index].2 = true;
    let kernel = HUB.with_borrow(|h| h.running.as_ref().is_some_and(|r| r.kernel.is_some()));
    request("variables", json!({ "variablesReference": reference }), move |reply| {
        let p = panel();
        let mut vars = reply["body"]["variables"].as_array().cloned().unwrap_or_default();
        // A cell's top level is the kernel's namespace: IPython's own names are noise.
        if kernel && depth == 0 {
            vars.retain(|v| v["name"].as_str().is_some_and(|n| !n.starts_with('_') && !["In", "Out", "exit", "quit", "get_ipython"].contains(&n)));
        }
        for (n, v) in vars.iter().enumerate() {
            let child = v["variablesReference"].as_i64().unwrap_or(0);
            let row = p.var_row(v["name"].as_str().unwrap_or("?"), v["value"].as_str().unwrap_or(""), child > 0, depth + 1, false);
            p.vars.insert(&row, (index + 1 + n) as i32);
            p.var_rows.borrow_mut().insert(index + 1 + n, (child, depth + 1, false));
        }
    });
}

fn mark_paused(view: &sourceview5::View, buffer: &sourceview5::Buffer, line: u32) {
    clear_paused();
    let tag = buffer.tag_table().lookup(PAUSED_TAG).unwrap_or_else(|| {
        let tag = gtk::TextTag::new(Some(PAUSED_TAG));
        if let Some(accent) = HUB.with_borrow(|h| h.accent) {
            tag.set_paragraph_background_rgba(Some(&accent));
        }
        buffer.tag_table().add(&tag);
        tag
    });
    if let Some(start) = buffer.iter_at_line(line as i32 - 1) {
        let mut end = start;
        end.forward_line();
        buffer.apply_tag(&tag, &start, &end);
        buffer.place_cursor(&start);
        let mark = buffer.get_insert();
        let view = view.clone();
        glib::idle_add_local_once(move || view.scroll_to_mark(&mark, 0.2, false, 0.0, 0.0));
    }
    HUB.with_borrow_mut(|h| h.paused = Some(buffer.clone()));
}

fn clear_paused() {
    if let Some(buffer) = HUB.with_borrow_mut(|h| h.paused.take()) {
        buffer.remove_tag_by_name(PAUSED_TAG, &buffer.start_iter(), &buffer.end_iter());
    }
}

/// Test actions: the panel's state, frames and variable rows.
pub fn describe() -> String {
    let p = panel();
    let rows = |list: &gtk::ListBox| {
        let mut out = Vec::new();
        let mut row = list.first_child();
        while let Some(r) = row {
            let mut texts = Vec::new();
            let mut child = r.first_child();
            // A row is a label, or a box of labels (variables).
            while let Some(c) = child {
                if let Some(l) = c.downcast_ref::<gtk::Label>() {
                    texts.push(l.text().to_string());
                }
                let mut inner = c.first_child();
                while let Some(i) = inner {
                    if let Some(l) = i.downcast_ref::<gtk::Label>() {
                        texts.push(l.text().trim().to_string());
                    }
                    inner = i.next_sibling();
                }
                child = c.next_sibling();
            }
            out.push(texts.into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" "));
            row = r.next_sibling();
        }
        out.join(" | ")
    };
    format!("{} · {} · frames: {} · vars: {}", p.title.text(), p.state.text(), rows(&p.frames), rows(&p.vars))
}

fn icon(name: &str, tip: &str, action: fn()) -> gtk::Button {
    let b = gtk::Button::from_icon_name(name);
    b.add_css_class("icon");
    b.set_tooltip_text(Some(tip));
    b.connect_clicked(move |_| action());
    b
}

fn section(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("section");
    l.set_xalign(0.0);
    l
}

impl Panel {
    fn new() -> Panel {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("debug");
        root.set_focusable(true);
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        bar.add_css_class("bar");
        let title = gtk::Label::new(Some("Debug"));
        title.add_css_class("title");
        title.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        let state = gtk::Label::new(None);
        state.add_css_class("state");
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        bar.append(&title);
        bar.append(&spacer);
        for b in [
            icon("media-playback-start-symbolic", "Continue (F5)", || step("continue")),
            icon("go-next-symbolic", "Step over (F10)", || step("next")),
            icon("go-down-symbolic", "Step into (F11)", || step("stepIn")),
            icon("go-up-symbolic", "Step out (Shift+F11)", || step("stepOut")),
            icon("media-playback-stop-symbolic", "Stop (Shift+F5)", stop),
        ] {
            bar.append(&b);
        }
        root.append(&bar);
        let state_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        state_row.set_margin_start(14);
        state.set_halign(gtk::Align::Start);
        state_row.append(&state);
        root.append(&state_row);

        root.append(&section("CALL STACK"));
        let frames = gtk::ListBox::new();
        frames.set_selection_mode(gtk::SelectionMode::Single);
        let frames_scroll = gtk::ScrolledWindow::new();
        frames_scroll.set_child(Some(&frames));
        frames_scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        frames_scroll.set_propagate_natural_height(true);
        frames_scroll.set_max_content_height(140);
        frames_scroll.set_margin_start(6);
        frames_scroll.set_margin_end(6);
        root.append(&frames_scroll);

        root.append(&section("VARIABLES"));
        let vars = gtk::ListBox::new();
        vars.set_selection_mode(gtk::SelectionMode::None);
        let vars_scroll = gtk::ScrolledWindow::new();
        vars_scroll.set_child(Some(&vars));
        vars_scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        vars_scroll.set_vexpand(true);
        vars_scroll.set_margin_start(6);
        vars_scroll.set_margin_end(6);
        root.append(&vars_scroll);

        root.append(&section("OUTPUT"));
        let output = gtk::TextView::new();
        output.set_editable(false);
        output.set_cursor_visible(false);
        output.set_monospace(true);
        output.set_wrap_mode(gtk::WrapMode::WordChar);
        output.set_left_margin(14);
        output.set_right_margin(14);
        output.set_top_margin(4);
        output.set_bottom_margin(8);
        let output_scroll = gtk::ScrolledWindow::new();
        output_scroll.set_child(Some(&output));
        output_scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        output_scroll.set_vexpand(true);
        root.append(&output_scroll);
        let hint = gtk::Label::new(Some("F9 breakpoint · F5 start/continue · F10 over · F11 into · Shift+F11 out · Shift+F5 stop"));
        hint.add_css_class("hint");
        hint.set_wrap(true);
        hint.set_xalign(0.0);
        root.append(&hint);

        frames.connect_row_activated(|_, row| select_frame(row.index() as usize));
        vars.connect_row_activated(|_, row| expand(row.index() as usize));
        Panel { root, title, state, frames, frame_rows: RefCell::new(Vec::new()), vars, var_rows: RefCell::new(Vec::new()), output }
    }

    fn clear(&self) {
        self.output.buffer().set_text("");
        self.frames.remove_all();
        self.vars.remove_all();
        self.frame_rows.borrow_mut().clear();
        self.var_rows.borrow_mut().clear();
    }

    fn set_state(&self, text: &str, class: Option<&str>) {
        self.state.set_text(text);
        for c in ["paused", "failed"] {
            self.state.remove_css_class(c);
        }
        if let Some(c) = class {
            self.state.add_css_class(c);
        }
    }

    fn append(&self, text: &str) {
        let buffer = self.output.buffer();
        let mut end = buffer.end_iter();
        buffer.insert(&mut end, &tileide_core::run::strip_ansi(text));
        let mark = buffer.create_mark(None, &buffer.end_iter(), false);
        self.output.scroll_mark_onscreen(&mark);
        buffer.delete_mark(&mark);
    }

    fn show_frames(&self, frames: &[Value]) {
        self.frames.remove_all();
        let mut rows = Vec::new();
        for f in frames {
            let path = f["source"]["path"].as_str().map(PathBuf::from);
            let line = f["line"].as_u64().unwrap_or(0) as u32;
            let cell = path.as_ref().and_then(|p| HUB.with_borrow(|h| h.cells.get(p).map(|c| c.label.clone())));
            let file = cell.or_else(|| path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "?".into());
            let label = gtk::Label::new(Some(&format!("{}  ·  {file}:{line}", f["name"].as_str().unwrap_or("?"))));
            label.set_xalign(0.0);
            label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            self.frames.append(&label);
            rows.push((f["id"].as_i64().unwrap_or(0), path, line));
        }
        *self.frame_rows.borrow_mut() = rows;
    }

    fn var_row(&self, name: &str, value: &str, expandable: bool, depth: usize, heading: bool) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_margin_start(depth as i32 * 14);
        let arrow = gtk::Label::new(Some(if expandable { "›" } else { " " }));
        arrow.set_width_chars(1);
        let name = gtk::Label::new(Some(name));
        if heading {
            name.add_css_class("title");
        }
        name.set_xalign(0.0);
        // One line: js-debug gives a function's whole source as its value.
        let one_line = value.contains('\n').then(|| value.lines().map(str::trim).collect::<Vec<_>>().join(" "));
        let value = gtk::Label::new(Some(one_line.as_deref().unwrap_or(value)));
        value.add_css_class("var-value");
        value.set_xalign(0.0);
        value.set_hexpand(true);
        value.set_ellipsize(gtk::pango::EllipsizeMode::End);
        value.set_selectable(true);
        row.append(&arrow);
        row.append(&name);
        row.append(&value);
        row
    }
}

/// Ends any session (TileIDE is quitting).
pub fn shutdown() {
    stop();
}
