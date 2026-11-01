// SPDX-License-Identifier: GPL-3.0-or-later
//! Extensions in the app (v0.3 phase 8). Each extension is its own process
//! (`tileide_extensions::host`), started lazily by its activation events and shared by
//! all workspaces; per-workspace modes decide where it may start and offer commands.
//! Reader threads hand messages to the main loop (`MainContext::invoke`), like the LSP
//! hub, so the UI never waits on an extension.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk4::glib;
use tileide_config::ExtensionPick;
use tileide_extensions::host::{Incoming, Process};
use tileide_extensions::meter::{self, Sample};
use tileide_extensions::{Event, Installed, Mode, install, should_start};
use tileide_lsp::{Value, json};

const STOP_GRACE: Duration = Duration::from_secs(2);

/// What the focused editor offers a command.
#[derive(Debug, Default, Clone)]
pub struct Context {
    pub file: Option<PathBuf>,
    pub language: Option<String>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    NotStarted,
    Starting,
    Running,
    Suspended,
    Exited(String),
}

/// One row of the Extensions section.
#[derive(Debug, Clone)]
pub struct Row {
    pub installed: Installed,
    pub pick: ExtensionPick,
    pub status: Status,
    pub startup: Option<Duration>,
    pub rss_kb: Option<u64>,
    pub cpu: Option<f64>,
}

pub struct Hooks {
    /// A message for the user (the banner).
    pub notice: Rc<dyn Fn(String)>,
    /// `editor/insert`: text at the focused editor's cursor.
    pub insert: Rc<dyn Fn(String)>,
    /// Something shown in the Extensions section changed.
    pub changed: Rc<dyn Fn()>,
}

struct Running {
    process: Process,
    /// Tells this run's messages from a previous run's late ones.
    generation: u64,
    init: i64,
    /// Requests waiting for `initialize` to return.
    queue: Vec<(String, Value)>,
    startup: Option<Duration>,
    last: Option<Sample>,
    rss_kb: Option<u64>,
    cpu: Option<f64>,
    suspended: bool,
}

#[derive(Default)]
struct Hub {
    root: PathBuf,
    installed: Vec<Installed>,
    picks: BTreeMap<String, ExtensionPick>,
    running: HashMap<String, Running>,
    exited: HashMap<String, String>,
    /// Stopped on purpose: their exit is not reported.
    stopping: HashSet<String>,
    /// Last measured startup per id, kept after it stops.
    startup: HashMap<String, Duration>,
    workspace: Option<u8>,
    folder: Option<PathBuf>,
    generation: u64,
    hooks: Option<Rc<Hooks>>,
}

thread_local! {
    static HUB: RefCell<Hub> = RefCell::new(Hub::default());
}

/// `$XDG_DATA_HOME/tileide/extensions` (installed data, not config).
pub fn default_root() -> PathBuf {
    glib::user_data_dir().join("tileide").join("extensions")
}

fn log_path(id: &str) -> PathBuf {
    glib::user_runtime_dir().join("tileide-extensions").join(format!("{id}.log"))
}

/// Finds the installed extensions; returns problems with broken ones.
pub fn init(root: PathBuf, picks: BTreeMap<String, ExtensionPick>, hooks: Hooks) -> Vec<String> {
    HUB.with_borrow_mut(|hub| {
        hub.root = root;
        hub.picks = picks;
        hub.hooks = Some(Rc::new(hooks));
    });
    rescan()
}

pub fn rescan() -> Vec<String> {
    let root = HUB.with_borrow(|hub| hub.root.clone());
    let (installed, problems) = tileide_extensions::discover(&root);
    let gone: Vec<String> = HUB.with_borrow_mut(|hub| {
        hub.installed = installed;
        hub.running.keys().filter(|id| !hub.installed.iter().any(|i| &i.manifest.id == *id)).cloned().collect()
    });
    gone.iter().for_each(|id| stop(id));
    problems
}

pub fn root() -> PathBuf {
    HUB.with_borrow(|hub| hub.root.clone())
}

pub fn picks() -> BTreeMap<String, ExtensionPick> {
    HUB.with_borrow(|hub| hub.picks.clone())
}

fn pick(hub: &Hub, id: &str) -> ExtensionPick {
    hub.picks.get(id).cloned().unwrap_or_default()
}

/// The active workspace changed (or its folder did).
pub fn set_workspace(number: Option<u8>, folder: Option<PathBuf>) {
    HUB.with_borrow_mut(|hub| {
        hub.workspace = number;
        hub.folder = folder;
    });
    event(Event::WorkspaceEntered);
}

/// Starts every extension that asked for `e` and may run in the active workspace.
pub fn event(e: Event) {
    let due: Vec<String> = HUB.with_borrow(|hub| {
        hub.installed
            .iter()
            .filter(|i| !hub.running.contains_key(&i.manifest.id))
            .filter(|i| {
                let p = pick(hub, &i.manifest.id);
                should_start(&i.manifest, p.enabled, p.mode(hub.workspace), e)
            })
            .map(|i| i.manifest.id.clone())
            .collect()
    });
    due.iter().for_each(|id| start(id));
}

fn start(id: &str) {
    let Some((installed, folder, generation)) = HUB.with_borrow_mut(|hub| {
        let installed = hub.installed.iter().find(|i| i.manifest.id == id)?.clone();
        hub.generation += 1;
        Some((installed, hub.folder.clone(), hub.generation))
    }) else {
        return;
    };
    let key = id.to_string();
    let deliver = move |message: Incoming| {
        let key = key.clone();
        glib::MainContext::default().invoke(move || receive(&key, generation, message));
    };
    match Process::spawn(&installed.manifest.run, &installed.dir, &log_path(id), deliver) {
        Ok(mut process) => {
            let params = json!({
                "hostVersion": env!("CARGO_PKG_VERSION"),
                "extensionDir": installed.dir,
                "workspaceFolder": folder,
            });
            let init = process.request("initialize", params).unwrap_or(-1);
            HUB.with_borrow_mut(|hub| {
                hub.exited.remove(id);
                hub.stopping.remove(id);
                hub.running.insert(id.into(), Running { process, generation, init, queue: Vec::new(), startup: None, last: None, rss_kb: None, cpu: None, suspended: false });
            });
        }
        Err(e) => {
            HUB.with_borrow_mut(|hub| hub.exited.insert(id.into(), e.clone()));
            notify(format!("extension {}: {e}", installed.manifest.name));
        }
    }
    changed();
}

fn hooks() -> Option<Rc<Hooks>> {
    HUB.with_borrow(|hub| hub.hooks.clone())
}

fn notify(message: String) {
    if let Some(h) = hooks() {
        // Outside any hub borrow: the banner may call back in.
        glib::idle_add_local_once(move || (h.notice)(message));
    }
}

fn changed() {
    if let Some(h) = hooks() {
        glib::idle_add_local_once(move || (h.changed)());
    }
}

fn name_of(hub: &Hub, id: &str) -> String {
    hub.installed.iter().find(|i| i.manifest.id == id).map_or_else(|| id.to_string(), |i| i.manifest.name.clone())
}

fn receive(id: &str, generation: u64, incoming: Incoming) {
    let current = HUB.with_borrow(|hub| hub.running.get(id).is_some_and(|r| r.generation == generation));
    if !current {
        return;
    }
    let message = match incoming {
        Incoming::Message(m) => m,
        Incoming::Exited => {
            let report = HUB.with_borrow_mut(|hub| {
                hub.running.remove(id);
                let deliberate = hub.stopping.remove(id);
                (!deliberate).then(|| {
                    let reason = format!("exited; log: {}", log_path(id).display());
                    hub.exited.insert(id.into(), reason.clone());
                    format!("extension {}: {reason}", name_of(hub, id))
                })
            });
            if let Some(r) = report {
                notify(r);
            }
            changed();
            return;
        }
    };
    let name = HUB.with_borrow(|hub| name_of(hub, id));
    match message.get("method").and_then(Value::as_str) {
        Some("window/notice") => {
            if let Some(text) = message["params"]["message"].as_str() {
                notify(format!("{name}: {text}"));
            }
        }
        Some("editor/insert") => {
            if let (Some(text), Some(h)) = (message["params"]["text"].as_str(), hooks()) {
                let text = text.to_string();
                glib::idle_add_local_once(move || (h.insert)(text));
            }
        }
        Some(_) => {}
        None => {
            if let Some(error) = message.get("error") {
                notify(format!("{name}: {}", error["message"].as_str().unwrap_or("request failed")));
            }
            let ready = HUB.with_borrow_mut(|hub| {
                let running = hub.running.get_mut(id)?;
                if message["id"].as_i64() != Some(running.init) || running.startup.is_some() {
                    return None;
                }
                let took = running.process.started.elapsed();
                running.startup = Some(took);
                hub.startup.insert(id.into(), took);
                Some(std::mem::take(&mut hub.running.get_mut(id)?.queue))
            });
            if let Some(queue) = ready {
                for (method, params) in queue {
                    send(id, &method, params);
                }
                changed();
            }
        }
    }
}

fn send(id: &str, method: &str, params: Value) {
    HUB.with_borrow_mut(|hub| {
        let Some(running) = hub.running.get_mut(id) else { return };
        if running.startup.is_none() {
            running.queue.push((method.into(), params));
        } else if let Err(e) = running.process.request(method, params) {
            eprintln!("extension {id}: {e}");
        }
    });
}

/// Runs command `command` (starting its extension if needed).
pub fn run_command(command: &str, context: Context) {
    let found = HUB.with_borrow(|hub| {
        let i = hub.installed.iter().find(|i| i.manifest.commands.iter().any(|c| c.id == command))?;
        let p = pick(hub, &i.manifest.id);
        Some((i.manifest.id.clone(), i.manifest.name.clone(), p.enabled, p.mode(hub.workspace), hub.running.get(&i.manifest.id).is_some_and(|r| r.suspended)))
    });
    let Some((id, name, enabled, mode, suspended)) = found else {
        notify(format!("no installed extension offers the command \"{command}\""));
        return;
    };
    if !enabled || mode == Mode::Off {
        notify(format!("{name} is {} (Settings → Extensions, Alt+Shift+X)", if enabled { "off in this workspace" } else { "disabled" }));
        return;
    }
    if suspended {
        notify(format!("{name} is suspended; resume it in Settings → Extensions"));
        return;
    }
    event(Event::Command(command));
    let params = json!({ "command": command, "file": context.file, "language": context.language, "text": context.text });
    send(&id, "command/run", params);
}

pub fn set_enabled(id: &str, enabled: bool) {
    HUB.with_borrow_mut(|hub| hub.picks.entry(id.into()).or_default().enabled = enabled);
    if !enabled {
        stop(id);
    }
    changed();
}

pub fn set_mode(id: &str, mode: Mode) {
    let workspace = HUB.with_borrow_mut(|hub| {
        let n = hub.workspace?;
        hub.picks.entry(id.into()).or_default().workspaces.insert(n, mode);
        Some(n)
    });
    if workspace.is_some() && mode == Mode::On {
        event(Event::WorkspaceEntered);
    }
    changed();
}

pub fn set_suspended(id: &str, suspended: bool) {
    let result = HUB.with_borrow_mut(|hub| {
        let running = hub.running.get_mut(id)?;
        let r = running.process.set_suspended(suspended);
        if r.is_ok() {
            running.suspended = suspended;
            running.last = None;
        }
        Some(r)
    });
    if let Some(Err(e)) = result {
        notify(format!("extension {id}: {e}"));
    }
    changed();
}

pub fn stop(id: &str) {
    let running = HUB.with_borrow_mut(|hub| {
        let r = hub.running.remove(id)?;
        hub.stopping.insert(id.into());
        Some(r)
    });
    if let Some(r) = running {
        r.process.stop(STOP_GRACE);
        changed();
    }
}

pub fn stop_all() {
    let ids: Vec<String> = HUB.with_borrow(|hub| hub.running.keys().cloned().collect());
    ids.iter().for_each(|id| stop(id));
}

/// Stops and deletes an extension's folder.
pub fn remove(id: &str) -> Result<(), String> {
    stop(id);
    let installed = HUB.with_borrow(|hub| hub.installed.iter().find(|i| i.manifest.id == id).cloned()).ok_or("not installed")?;
    install::remove(&installed)?;
    HUB.with_borrow_mut(|hub| {
        hub.picks.remove(id);
        hub.exited.remove(id);
    });
    rescan();
    changed();
    Ok(())
}

/// Installs off the main thread (`git clone` can take a while), then rescans.
pub fn install(source: Source, replace: bool, done: impl FnOnce(Result<String, String>) + 'static) {
    let root = root();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = match &source {
            Source::Folder(path) => install::from_folder(path, &root, replace),
            Source::Git(url) => install::from_git(url, &root, replace),
        };
        let _ = tx.send(result.map(|i| i.manifest.name));
    });
    let mut done = Some(done);
    glib::timeout_add_local(Duration::from_millis(50), move || match rx.try_recv() {
        Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        result => {
            let result = result.unwrap_or_else(|_| Err("install thread failed".into()));
            if result.is_ok() {
                for p in rescan() {
                    notify(p);
                }
                changed();
            }
            if let Some(done) = done.take() {
                done(result);
            }
            glib::ControlFlow::Break
        }
    });
}

pub enum Source {
    Folder(PathBuf),
    Git(String),
}

/// Refreshes the weight meter of every running extension.
pub fn sample() {
    HUB.with_borrow_mut(|hub| {
        for r in hub.running.values_mut() {
            let Some(now) = meter::sample(r.process.pid) else { continue };
            r.cpu = r.last.as_ref().map(|before| meter::cpu_percent(before, &now));
            r.rss_kb = Some(now.rss_kb);
            r.last = Some(now);
        }
    });
}

pub fn rows() -> Vec<Row> {
    HUB.with_borrow(|hub| {
        hub.installed
            .iter()
            .map(|i| {
                let id = &i.manifest.id;
                let running = hub.running.get(id);
                let status = match (running, hub.exited.get(id)) {
                    (Some(r), _) if r.suspended => Status::Suspended,
                    (Some(r), _) if r.startup.is_none() => Status::Starting,
                    (Some(_), _) => Status::Running,
                    (None, Some(reason)) => Status::Exited(reason.clone()),
                    (None, None) => Status::NotStarted,
                };
                Row {
                    installed: i.clone(),
                    pick: pick(hub, id),
                    status,
                    startup: hub.startup.get(id).copied(),
                    rss_kb: running.and_then(|r| r.rss_kb),
                    cpu: running.and_then(|r| r.cpu),
                }
            })
            .collect()
    })
}

pub fn workspace() -> Option<u8> {
    HUB.with_borrow(|hub| hub.workspace)
}

/// The extensions folder, created on first use (for "Open folder").
pub fn ensure_root() -> PathBuf {
    let root = root();
    let _ = std::fs::create_dir_all(&root);
    root
}

