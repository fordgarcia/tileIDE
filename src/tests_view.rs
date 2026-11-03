// SPDX-License-Identifier: GPL-3.0-or-later
//! The Tests tile (v0.5 phase 9): pytest's tests by file with their last result,
//! updated live while a run goes. A test's name opens it; a failure's line opens where
//! it failed; ▶ runs one test or file. `Alt+Shift+R` runs the test at the cursor (or
//! the file, or everything) and `F8` goes to the next failure.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;
use tileide_core::theme::{self, Theme};

use crate::pytest::{Event, Outcome, Report, Run, Test};

pub fn css(t: &Theme) -> String {
    let c = theme::css;
    let green = t.terminal_palette()[2];
    format!(
        ".tests .file {{ font-weight: bold; color: {text}; padding: 8px 14px 2px 14px; }}
.tests .row {{ padding: 1px 14px; }}
.tests .mark {{ font-family: monospace; min-width: 16px; }}
.tests .mark.passed {{ color: {green}; }}
.tests .mark.failed, .tests .mark.error {{ color: {error}; }}
.tests .mark.skipped, .tests .mark.idle, .tests .seconds, .tests .why.skipped {{ color: {muted}; }}
.tests .mark.running {{ color: {accent}; }}
.tests .why {{ color: {error}; font-family: monospace; font-size: 12px; padding: 0 14px 4px 40px; }}
.tests .summary {{ color: {muted}; padding: 4px 14px; }}
.tests .log {{ color: {muted}; font-family: monospace; font-size: 12px; padding: 4px 14px; }}
.tests link {{ color: {text}; }}
.tests .why link {{ color: {error}; }}
.tests button.run {{ min-width: 22px; min-height: 22px; padding: 0; border: none; box-shadow: none; background: transparent; color: {muted}; border-radius: 11px; }}
.tests button.run:hover {{ background-color: {hover}; color: {text}; }}",
        text = c(t.text, 1.0),
        muted = c(t.muted, 1.0),
        accent = c(t.accent, 1.0),
        error = c(t.error, 1.0),
        green = c(green, 1.0),
        hover = c(t.text, 0.06),
    )
}

/// Opens a file (relative paths are against the run folder) at a 1-based line.
pub type OpenAt = Rc<dyn Fn(PathBuf, u32)>;

pub struct TestsView {
    root: gtk::Box,
    title: gtk::Label,
    summary: gtk::Label,
    list: gtk::Box,
    /// pytest's own output when it didn't get to run tests (collection errors).
    log: gtk::Label,
    install: gtk::Button,
    dir: RefCell<PathBuf>,
    folder: RefCell<Option<PathBuf>>,
    python: RefCell<Option<PathBuf>>,
    tests: RefCell<Vec<Test>>,
    results: RefCell<HashMap<String, Report>>,
    running: RefCell<HashSet<String>>,
    /// Running everything (the collected list replaces the old one).
    all: Cell<bool>,
    run: RefCell<Option<Run>>,
    events: RefCell<Option<std::sync::mpsc::Receiver<Event>>>,
    output: RefCell<Vec<String>>,
    started: Cell<Option<Instant>>,
    dirty: Cell<bool>,
    /// The failure `F8` showed last.
    last_failure: Cell<Option<usize>>,
    open: OpenAt,
    me: RefCell<Weak<TestsView>>,
}

fn pill(text: &str) -> gtk::Button {
    let b = gtk::Button::with_label(text);
    b.add_css_class("pill");
    b.set_valign(gtk::Align::Center);
    b
}

fn link_label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(None);
    l.set_markup(&format!("<a href=\"go\">{}</a>", glib::markup_escape_text(text)));
    l.set_xalign(0.0);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

impl TestsView {
    pub fn new(open: OpenAt) -> Rc<TestsView> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("notebook");
        root.add_css_class("tests");
        root.set_widget_name("Tests");
        root.set_focusable(true);
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.add_css_class("bar");
        let title = gtk::Label::new(Some("Tests"));
        title.add_css_class("kernel");
        title.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        let (all, failed, stop) = (pill("Run all"), pill("Run failed"), pill("Stop"));
        let install = pill("Install pytest");
        install.set_visible(false);
        bar.append(&title);
        bar.append(&spacer);
        for b in [&install, &all, &failed, &stop] {
            bar.append(b);
        }
        root.append(&bar);
        let summary = gtk::Label::new(Some("Not run yet."));
        summary.add_css_class("summary");
        summary.set_xalign(0.0);
        summary.set_wrap(true);
        root.append(&summary);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let log = gtk::Label::new(None);
        log.add_css_class("log");
        log.set_xalign(0.0);
        log.set_selectable(true);
        log.set_wrap(true);
        log.set_visible(false);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&list);
        content.append(&log);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_vexpand(true);
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroll.set_child(Some(&content));
        root.append(&scroll);
        let view = Rc::new(TestsView {
            root,
            title,
            summary,
            list,
            log,
            install,
            dir: RefCell::new(PathBuf::from(".")),
            folder: RefCell::new(None),
            python: RefCell::new(None),
            tests: RefCell::new(Vec::new()),
            results: RefCell::new(HashMap::new()),
            running: RefCell::new(HashSet::new()),
            all: Cell::new(false),
            run: RefCell::new(None),
            events: RefCell::new(None),
            output: RefCell::new(Vec::new()),
            started: Cell::new(None),
            dirty: Cell::new(false),
            last_failure: Cell::new(None),
            open,
            me: RefCell::new(Weak::new()),
        });
        *view.me.borrow_mut() = Rc::downgrade(&view);
        let on = |f: fn(&TestsView)| {
            let me = Rc::downgrade(&view);
            move |_: &gtk::Button| {
                if let Some(v) = me.upgrade() {
                    f(&v);
                }
            }
        };
        all.connect_clicked(on(|v| v.rerun(Vec::new())));
        failed.connect_clicked(on(TestsView::run_failed));
        stop.connect_clicked(on(TestsView::stop));
        view.install.connect_clicked(on(TestsView::install_pytest));
        view
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// Runs `ids` (none: every test) in `dir` with the Python for it (chosen, else the
    /// first with pytest). `folder` is the workspace folder (for finding the Python).
    pub fn start(&self, dir: &Path, folder: Option<&Path>, ids: Vec<String>) {
        if *self.dir.borrow() != dir {
            // Another project: its own list.
            self.tests.borrow_mut().clear();
            self.results.borrow_mut().clear();
            *self.python.borrow_mut() = None;
        }
        *self.dir.borrow_mut() = dir.to_path_buf();
        *self.folder.borrow_mut() = folder.map(Path::to_path_buf);
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let shown = home.as_ref().and_then(|h| dir.strip_prefix(h).ok()).map_or_else(|| dir.display().to_string(), |r| format!("~/{}", r.display()));
        self.title.set_text(&format!("Tests · {shown}"));
        let known = self.python.borrow().clone();
        if let Some(python) = known {
            self.launch(&python, ids);
            return;
        }
        self.summary.set_text("Looking for pytest…");
        let candidates = tileide_core::python::candidates(dir, folder);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let found = candidates.iter().find(|p| tileide_core::python::imports(p, "pytest")).cloned();
            let _ = tx.send((found, candidates.into_iter().next()));
        });
        let me = self.me.borrow().clone();
        glib::timeout_add_local(Duration::from_millis(40), move || match rx.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Ok((found, first)) => {
                if let Some(v) = me.upgrade() {
                    match found {
                        Some(python) => {
                            *v.python.borrow_mut() = Some(python.clone());
                            v.launch(&python, ids.clone());
                        }
                        None => v.no_pytest(first),
                    }
                }
                glib::ControlFlow::Break
            }
            Err(_) => glib::ControlFlow::Break,
        });
    }

    fn no_pytest(&self, python: Option<PathBuf>) {
        let venv = python.as_deref().and_then(tileide_core::python::venv_of);
        self.install.set_visible(venv.is_some());
        *self.python.borrow_mut() = None;
        self.summary.set_text(&match (python, venv) {
            (Some(p), Some(_)) => format!("pytest isn't installed for {}. Install pytest puts it there.", p.display()),
            _ => "No Python here has pytest. Make a .venv for the project with Alt+P, then Install pytest.".into(),
        });
    }

    /// Installs pytest into the project's virtualenv, in a terminal tile.
    fn install_pytest(&self) {
        let candidates = tileide_core::python::candidates(&self.dir.borrow(), self.folder.borrow().as_deref());
        let Some(python) = candidates.into_iter().find(|p| tileide_core::python::venv_of(p).is_some()) else { return };
        let quoted = format!("'{}'", python.display().to_string().replace('\'', r"'\''"));
        let command = if tileide_core::python::which("uv").is_some() { format!("uv pip install --python {quoted} pytest") } else { format!("{quoted} -m pip install pytest") };
        crate::interpreter::terminal(&self.dir.borrow(), &command);
        self.summary.set_text("Installing pytest in a terminal tile; run the tests again once it's done.");
        self.install.set_visible(false);
    }

    fn launch(&self, python: &Path, ids: Vec<String>) {
        self.stop();
        self.install.set_visible(false);
        self.all.set(ids.is_empty());
        {
            let tests = self.tests.borrow();
            let mut running = self.running.borrow_mut();
            running.clear();
            // A file or class id stands for the tests inside it.
            let covered = |t: &Test| ids.is_empty() || ids.iter().any(|id| t.id == *id || t.id.starts_with(&format!("{id}::")));
            running.extend(tests.iter().filter(|t| covered(t)).map(|t| t.id.clone()));
        }
        self.output.borrow_mut().clear();
        self.started.set(Some(Instant::now()));
        self.last_failure.set(None);
        self.summary.set_text(if ids.is_empty() { "Running all tests…".into() } else { format!("Running {}…", ids.join(", ")) }.as_str());
        let (tx, rx) = std::sync::mpsc::channel();
        let deliver = move |e: Event| {
            let _ = tx.send(e);
        };
        match Run::start(python, &self.dir.borrow(), &ids, false, deliver) {
            Ok(run) => {
                println!("tests: running {} in {}", if ids.is_empty() { "all".into() } else { ids.join(" ") }, self.dir.borrow().display());
                *self.run.borrow_mut() = Some(run);
                *self.events.borrow_mut() = Some(rx);
                self.render();
                self.poll();
            }
            Err(e) => self.summary.set_text(&e),
        }
    }

    fn poll(&self) {
        let me = self.me.borrow().clone();
        glib::timeout_add_local(Duration::from_millis(50), move || {
            let Some(v) = me.upgrade() else { return glib::ControlFlow::Break };
            let events: Vec<Event> = match v.events.borrow().as_ref() {
                Some(rx) => rx.try_iter().collect(),
                None => return glib::ControlFlow::Break,
            };
            let mut alive = true;
            for e in events {
                alive &= e != Event::Exited;
                v.apply(e);
            }
            if v.dirty.take() {
                v.render();
            }
            if alive { glib::ControlFlow::Continue } else { glib::ControlFlow::Break }
        });
    }

    fn apply(&self, event: Event) {
        match event {
            Event::Collected(found) => {
                let mut tests = self.tests.borrow_mut();
                if self.all.get() {
                    let ids: HashSet<&String> = found.iter().map(|t| &t.id).collect();
                    self.results.borrow_mut().retain(|id, _| ids.contains(id));
                    *tests = found.clone();
                } else {
                    for t in &found {
                        if !tests.iter().any(|o| o.id == t.id) {
                            tests.push(t.clone());
                        }
                    }
                }
                self.running.borrow_mut().extend(found.into_iter().map(|t| t.id));
                self.dirty.set(true);
            }
            Event::Result(report) => {
                self.running.borrow_mut().remove(&report.id);
                self.results.borrow_mut().insert(report.id.clone(), report);
                self.dirty.set(true);
            }
            Event::Output(line) => self.output.borrow_mut().push(line),
            Event::Finished(status) => self.finished(status),
            Event::Exited => {
                self.running.borrow_mut().clear();
                self.events.take();
                if let Some(run) = self.run.take() {
                    run.finish();
                }
                self.dirty.set(true);
            }
        }
    }

    fn finished(&self, status: i64) {
        let took = self.started.get().map_or(0.0, |s| s.elapsed().as_secs_f64());
        let results = self.results.borrow();
        let count = |o: Outcome| results.values().filter(|r| r.outcome == o).count();
        let mut parts = Vec::new();
        for (n, word) in [(count(Outcome::Passed), "passed"), (count(Outcome::Failed), "failed"), (count(Outcome::Error), "error"), (count(Outcome::Skipped), "skipped")] {
            if n > 0 {
                parts.push(format!("{n} {word}{}", if word == "error" && n > 1 { "s" } else { "" }));
            }
        }
        let text = match status {
            5 => "No tests found (pytest looks for test_*.py and *_test.py).".to_string(),
            2..=4 => "pytest stopped before running the tests:".to_string(),
            _ => format!("{} · {took:.1} s{}", parts.join(", "), if count(Outcome::Failed) + count(Outcome::Error) > 0 { " · F8: next failure" } else { "" }),
        };
        self.summary.set_text(&text);
        println!("tests: {text}");
        // Collection errors and the like: what pytest said.
        let log: Vec<String> = self.output.borrow().iter().filter(|l| !l.trim().is_empty()).cloned().collect();
        let show = matches!(status, 2..=4);
        self.log.set_text(&log.iter().rev().take(40).rev().cloned().collect::<Vec<_>>().join("\n"));
        self.log.set_visible(show && !log.is_empty());
    }

    pub fn stop(&self) {
        if let Some(mut run) = self.run.take() {
            run.stop();
            self.summary.set_text("Stopped.");
        }
        self.events.take();
        self.running.borrow_mut().clear();
        self.dirty.set(true);
    }

    fn rerun(&self, ids: Vec<String>) {
        let (dir, folder) = (self.dir.borrow().clone(), self.folder.borrow().clone());
        self.start(&dir, folder.as_deref(), ids);
    }

    fn run_failed(&self) {
        let failed = self.failures().into_iter().map(|t| t.id).collect::<Vec<_>>();
        if failed.is_empty() {
            self.summary.set_text("No failed tests to run.");
            return;
        }
        self.rerun(failed);
    }

    /// The failed tests (and errors), in list order.
    fn failures(&self) -> Vec<Test> {
        let results = self.results.borrow();
        self.tests.borrow().iter().filter(|t| results.get(&t.id).is_some_and(|r| matches!(r.outcome, Outcome::Failed | Outcome::Error))).cloned().collect()
    }

    /// `F8`: the next failure's place (cycling); false if there is none.
    pub fn next_failure(&self) -> bool {
        let failures = self.failures();
        if failures.is_empty() {
            return false;
        }
        let next = self.last_failure.get().map_or(0, |i| (i + 1) % failures.len());
        self.last_failure.set(Some(next));
        let t = &failures[next];
        let at = self.results.borrow().get(&t.id).and_then(|r| r.at.clone());
        let (file, line) = at.unwrap_or((t.file.clone(), t.line));
        println!("tests: failure {} of {}: {} at {file}:{line}", next + 1, failures.len(), t.id);
        (self.open)(self.dir.borrow().join(file), line);
        true
    }

    fn render(&self) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let tests = self.tests.borrow();
        let results = self.results.borrow();
        let running = self.running.borrow();
        let mut file: Option<&str> = None;
        for t in tests.iter() {
            if file != Some(t.file.as_str()) {
                file = Some(&t.file);
                let head = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                let name = gtk::Label::new(Some(&t.file));
                name.add_css_class("file");
                name.set_xalign(0.0);
                name.set_hexpand(true);
                head.append(&name);
                head.append(&self.run_button(&t.file, "Run this file"));
                self.list.append(&head);
            }
            let result = results.get(&t.id);
            let (mark, class) = if running.contains(&t.id) {
                ("⟳", "running")
            } else {
                match result.map(|r| r.outcome) {
                    Some(Outcome::Passed) => ("✓", "passed"),
                    Some(Outcome::Failed) => ("✗", "failed"),
                    Some(Outcome::Error) => ("!", "error"),
                    Some(Outcome::Skipped) => ("–", "skipped"),
                    None => ("○", "idle"),
                }
            };
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.add_css_class("row");
            let m = gtk::Label::new(Some(mark));
            m.add_css_class("mark");
            m.add_css_class(class);
            let short = t.id.split_once("::").map_or(t.id.as_str(), |(_, rest)| rest);
            let name = link_label(short, &[]);
            name.set_hexpand(true);
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let (open, place) = (self.open.clone(), (self.dir.borrow().join(&t.file), t.line));
            name.connect_activate_link(move |_, _| {
                open(place.0.clone(), place.1);
                glib::Propagation::Stop
            });
            row.append(&m);
            row.append(&name);
            if let Some(r) = result.filter(|r| r.seconds >= 0.01) {
                let s = gtk::Label::new(Some(&format!("{:.2} s", r.seconds)));
                s.add_css_class("seconds");
                row.append(&s);
            }
            row.append(&self.run_button(&t.id, "Run this test"));
            self.list.append(&row);
            // Why it failed (or was skipped), one line, opening where it failed.
            if let Some(r) = result.filter(|r| r.outcome != Outcome::Passed && !r.message.is_empty()) {
                let first = r.message.lines().next().unwrap_or("");
                let place = r.at.as_ref().map(|(f, l)| format!("{f}:{l}: "));
                let why = match (&r.at, r.outcome) {
                    (Some((f, l)), Outcome::Failed | Outcome::Error) => {
                        let label = link_label(&format!("{}{first}", place.unwrap_or_default()), &["why"]);
                        let (open, path, line) = (self.open.clone(), self.dir.borrow().join(f), *l);
                        label.connect_activate_link(move |_, _| {
                            open(path.clone(), line);
                            glib::Propagation::Stop
                        });
                        label.set_tooltip_text(Some(&r.text));
                        label
                    }
                    _ => {
                        let l = gtk::Label::new(Some(first));
                        l.add_css_class("why");
                        l.set_xalign(0.0);
                        if r.outcome == Outcome::Skipped {
                            l.add_css_class("skipped");
                        }
                        l
                    }
                };
                why.set_wrap(true);
                self.list.append(&why);
            }
        }
    }

    fn run_button(&self, id: &str, tip: &str) -> gtk::Button {
        let b = gtk::Button::from_icon_name("media-playback-start-symbolic");
        b.add_css_class("run");
        b.set_tooltip_text(Some(tip));
        let (me, id) = (self.me.borrow().clone(), id.to_string());
        b.connect_clicked(move |_| {
            if let Some(v) = me.upgrade() {
                v.rerun(vec![id.clone()]);
            }
        });
        b
    }

    /// Test action `tests:all|failed|stop`: the bar's buttons.
    pub fn test_action(&self, verb: &str) {
        match verb {
            "all" => self.rerun(Vec::new()),
            "failed" => self.run_failed(),
            "stop" => self.stop(),
            other => eprintln!("test action: tests:{other}?"),
        }
    }

    /// Test action: the rows as text.
    pub fn describe(&self) -> String {
        let results = self.results.borrow();
        let rows: Vec<String> = self
            .tests
            .borrow()
            .iter()
            .map(|t| format!("{} {}", results.get(&t.id).map_or("○".into(), |r| format!("{:?}", r.outcome)), t.id))
            .collect();
        format!("{} | {}", self.summary.text(), rows.join(" | "))
    }
}
