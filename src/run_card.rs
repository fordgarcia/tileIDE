// SPDX-License-Identifier: GPL-3.0-or-later
//! `Alt+R` (v0.4): runs the focused file and shows its output on a large card over a
//! dimmed window, in the Settings card's style. The steps (compile, then run) run in
//! their own process group off the main thread; output streams in as it comes.
//! `Ctrl+C` stops the run, `Alt+R` runs again, `Esc` closes (and stops it).
//! Programs get no stdin: interactive ones belong in a terminal tile. stdout and stderr
//! share one pipe, so lines show in the order they were written.

use std::cell::{Cell, RefCell};
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4 as gtk;
use tileide_core::run::{Plan, strip_ansi};
use tileide_core::theme::{self, Theme};

/// Output kept on the card; older text is dropped from the top.
const MAX_CHARS: i32 = 1_000_000;

pub fn css(t: &Theme) -> String {
    let c = theme::css;
    format!(
        ".run {{ background-color: rgba(0, 0, 0, 0.5); }}
.run .card {{ background-color: {surface}; color: {text}; border-radius: 22px; border: 1px solid {edge}; padding: 10px; box-shadow: 0 18px 48px rgba(0, 0, 0, 0.4); }}
.run .header {{ padding: 6px 8px 4px 12px; }}
.run .name {{ font-size: 15px; font-weight: 600; }}
.run .status {{ font-size: 12px; border-radius: 10px; padding: 2px 10px; background-color: {chip}; color: {muted}; }}
.run .status.ok {{ background-color: {ok_bg}; color: {accent}; }}
.run .status.failed {{ background-color: {err_bg}; color: {error}; }}
.run .key {{ font-size: 11px; border-radius: 6px; padding: 2px 7px; background-color: {chip}; color: {text}; }}
.run .keyname {{ font-size: 12px; color: {muted}; margin-right: 8px; }}
.run .divider {{ background-color: {edge}; min-height: 1px; margin: 8px 4px 0 4px; }}
.run scrolledwindow, .run textview, .run textview text {{ background: transparent; }}
.run textview text {{ color: {text}; caret-color: transparent; }}
.run .command {{ font-size: 12px; color: {muted}; padding: 6px 12px 2px 12px; }}",
        surface = c(t.surface, 0.98),
        text = c(t.text, 1.0),
        muted = c(t.muted, 1.0),
        edge = c(t.text, 0.10),
        chip = c(t.text, 0.07),
        accent = c(t.accent, 1.0),
        ok_bg = c(t.accent, 0.16),
        error = c(t.error, 1.0),
        err_bg = c(t.error, 0.16),
    )
}

enum Out {
    Step(String),
    Text(String),
    /// Exit code (`None` = killed by a signal), whether it was stopped, how long it took.
    Done(Option<i32>, bool, Duration),
}

pub struct RunCard {
    root: gtk::Box,
    card: gtk::Box,
    name: gtk::Label,
    status: gtk::Label,
    output: gtk::TextView,
    scroll: gtk::ScrolledWindow,
    command: gtk::Label,
    note_tag: gtk::TextTag,
    /// This run's process group (0 when none) and stop flag; fresh per run, so a
    /// stopped run's worker never sees the next run's flag.
    group: RefCell<Arc<AtomicI32>>,
    stopped: RefCell<Arc<AtomicBool>>,
    /// Bumped per run, so a late message from an older run is dropped.
    generation: Cell<u64>,
    running: Cell<bool>,
    /// The last plan, for "run again".
    last: RefCell<Option<Plan>>,
    on_close: Rc<dyn Fn()>,
}

fn keycap(key: &str, what: &str) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    let k = gtk::Label::new(Some(key));
    k.add_css_class("key");
    let w = gtk::Label::new(Some(what));
    w.add_css_class("keyname");
    b.append(&k);
    b.append(&w);
    b
}

impl RunCard {
    pub fn new(overlay: &gtk::Overlay, on_close: Rc<dyn Fn()>) -> Rc<RunCard> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("run");
        root.set_visible(false);
        let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        card.add_css_class("card");
        card.set_halign(gtk::Align::Center);
        card.set_valign(gtk::Align::Center);
        card.set_vexpand(true);
        card.set_overflow(gtk::Overflow::Hidden);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        header.add_css_class("header");
        let name = gtk::Label::new(None);
        name.add_css_class("name");
        name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        name.set_max_width_chars(40);
        let status = gtk::Label::new(None);
        status.add_css_class("status");
        status.set_valign(gtk::Align::Center);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        header.append(&name);
        header.append(&status);
        header.append(&spacer);
        // The way out, always in view.
        for (key, what) in [("Ctrl+C", "stop"), ("Alt+R", "run again"), ("Esc", "close")] {
            header.append(&keycap(key, what));
        }
        card.append(&header);
        let divider = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        divider.add_css_class("divider");
        card.append(&divider);

        let output = gtk::TextView::new();
        output.set_editable(false);
        output.set_cursor_visible(false);
        output.set_monospace(true);
        output.set_wrap_mode(gtk::WrapMode::WordChar);
        output.set_left_margin(24);
        output.set_right_margin(24);
        output.set_top_margin(18);
        output.set_bottom_margin(18);
        output.set_pixels_below_lines(2);
        output.add_css_class("output");
        let buffer = output.buffer();
        let note_tag = buffer.create_tag(Some("note"), &[]).expect("new tag");
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroll.set_vexpand(true);
        scroll.set_child(Some(&output));
        card.append(&scroll);
        let command = gtk::Label::new(None);
        command.add_css_class("command");
        command.set_xalign(0.0);
        command.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        // Its natural width would otherwise widen the card to the whole window.
        command.set_max_width_chars(40);
        card.append(&command);
        root.append(&card);
        overlay.add_overlay(&root);

        let view = Rc::new(RunCard {
            root,
            card,
            name,
            status,
            output,
            scroll,
            command,
            note_tag,
            group: RefCell::new(Arc::new(AtomicI32::new(0))),
            stopped: RefCell::new(Arc::new(AtomicBool::new(false))),
            generation: Cell::new(0),
            running: Cell::new(false),
            last: RefCell::new(None),
            on_close,
        });

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let v = Rc::downgrade(&view);
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(v) = v.upgrade() else { return glib::Propagation::Proceed };
            let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
            match key {
                gdk::Key::Escape => v.close(),
                gdk::Key::c | gdk::Key::C if ctrl && v.running.get() => v.stop(),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        view.root.add_controller(keys);
        let click = gtk::GestureClick::new();
        let v = Rc::downgrade(&view);
        click.connect_released(move |gesture, _, x, y| {
            let Some(v) = v.upgrade() else { return };
            if v.root.pick(x, y, gtk::PickFlags::DEFAULT).is_some_and(|w| w == v.root) {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                v.close();
            }
        });
        view.root.add_controller(click);
        view
    }

    pub fn is_open(&self) -> bool {
        crate::motion::shown(&self.root)
    }

    pub fn set_theme(&self, t: &Theme) {
        self.note_tag.set_foreground_rgba(Some(&rgba(t.muted, 1.0)));
    }

    fn show(&self) {
        let (w, h) = (self.root.parent().map_or(1100, |p| p.width()), self.root.parent().map_or(700, |p| p.height()));
        self.card.set_size_request((w - 80).clamp(320, 1000), (h - 80).clamp(260, 700));
        if !self.is_open() {
            crate::motion::fade_in(&self.root);
        }
        self.output.grab_focus();
    }

    /// Opens the card with a message instead of a run (nothing to run, missing tool).
    pub fn show_message(&self, title: &str, message: &str) {
        self.stop();
        self.generation.set(self.generation.get() + 1);
        self.name.set_text(title);
        self.set_status("Can't run", Some("failed"));
        self.command.set_text("");
        let buffer = self.output.buffer();
        buffer.set_text("");
        let mut end = buffer.end_iter();
        buffer.insert(&mut end, message);
        self.show();
    }

    /// Runs `plan`, replacing whatever ran before.
    pub fn run(self: &Rc<Self>, plan: Plan) {
        self.stop();
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.output.buffer().set_text("");
        self.name.set_text(&plan.label);
        self.set_status("Running…", None);
        self.command.set_text(&format!("{}  ·  {}", plan.steps.iter().map(|s| shell_line(s)).collect::<Vec<_>>().join("  &&  "), plan.cwd.display()));
        *self.last.borrow_mut() = Some(plan.clone());
        self.running.set(true);
        let (group, stopped) = (Arc::new(AtomicI32::new(0)), Arc::new(AtomicBool::new(false)));
        *self.group.borrow_mut() = group.clone();
        *self.stopped.borrow_mut() = stopped.clone();
        self.show();

        let weak = Rc::downgrade(self);
        let (tx, rx) = std::sync::mpsc::channel::<Out>();
        std::thread::spawn(move || execute(&plan, &group, &stopped, &tx));
        // Drained on the main loop; a burst of output lands as one insert.
        glib::timeout_add_local(Duration::from_millis(30), move || {
            let Some(v) = weak.upgrade() else { return glib::ControlFlow::Break };
            if v.generation.get() != generation {
                return glib::ControlFlow::Break;
            }
            loop {
                match rx.try_recv() {
                    Ok(Out::Step(line)) if v.last.borrow().as_ref().is_some_and(|p| p.steps.len() > 1) => v.append(&format!("$ {line}\n"), Some(&v.note_tag)),
                    Ok(Out::Step(_)) => {}
                    Ok(Out::Text(text)) => v.append(&text, None),
                    Ok(Out::Done(code, stopped, took)) => {
                        v.finish(code, stopped, took);
                        return glib::ControlFlow::Break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => return glib::ControlFlow::Break,
                }
            }
        });
    }

    /// Runs the last plan again (`Alt+R` with no focused file).
    pub fn rerun(self: &Rc<Self>) -> bool {
        let last = self.last.borrow().clone();
        match last {
            Some(plan) => {
                self.run(plan);
                true
            }
            None => false,
        }
    }

    fn finish(&self, code: Option<i32>, stopped: bool, took: Duration) {
        self.running.set(false);
        let secs = took.as_secs_f64();
        let time = if secs < 10.0 { format!("{secs:.2} s") } else { format!("{secs:.0} s") };
        match (stopped, code) {
            (true, _) => self.set_status(&format!("Stopped · {time}"), Some("failed")),
            (false, Some(0)) => self.set_status(&format!("Exit 0 · {time}"), Some("ok")),
            (false, Some(n)) => self.set_status(&format!("Exit {n} · {time}"), Some("failed")),
            (false, None) => self.set_status(&format!("Killed · {time}"), Some("failed")),
        }
        if self.output.buffer().char_count() == 0 {
            self.append("(no output)", Some(&self.note_tag));
        }
        // A Python script that stopped on a missing module: say how to install it.
        let python = self.last.borrow().as_ref().and_then(|p| p.steps.last()).and_then(|s| s.first()).filter(|p| p.contains("python")).map(PathBuf::from);
        if code != Some(0)
            && let Some(python) = python
        {
            let buffer = self.output.buffer();
            let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
            if let Some(module) = tileide_core::python::missing_module(&text) {
                let hint = tileide_core::python::install_hint(&module, &python);
                self.append(&format!("\n{hint}\n"), Some(&self.note_tag));
                crate::alerts::missing(hint);
            }
        }
    }

    fn set_status(&self, text: &str, class: Option<&str>) {
        self.status.set_text(text);
        for c in ["ok", "failed"] {
            self.status.remove_css_class(c);
        }
        if let Some(c) = class {
            self.status.add_css_class(c);
        }
    }

    fn append(&self, text: &str, tag: Option<&gtk::TextTag>) {
        let buffer = self.output.buffer();
        // Follow the end only if the reader is already there.
        let adj = self.scroll.vadjustment();
        let at_end = adj.value() + adj.page_size() >= adj.upper() - 4.0;
        let mut end = buffer.end_iter();
        match tag {
            Some(tag) => buffer.insert_with_tags(&mut end, text, &[tag]),
            None => buffer.insert(&mut end, text),
        }
        let excess = buffer.char_count() - MAX_CHARS;
        if excess > 0 {
            let (mut start, mut cut) = (buffer.start_iter(), buffer.iter_at_offset(excess));
            buffer.delete(&mut start, &mut cut);
        }
        if at_end {
            let mark = buffer.create_mark(None, &buffer.end_iter(), false);
            self.output.scroll_mark_onscreen(&mark);
            buffer.delete_mark(&mark);
        }
    }

    /// Ends the running steps (the whole process group).
    pub fn stop(&self) {
        let pgid = self.group.borrow().load(Ordering::SeqCst);
        if self.running.get() {
            self.stopped.borrow().store(true, Ordering::SeqCst);
        }
        if self.running.get() && pgid > 0 {
            // SAFETY: kill(2) on the group our own child leads.
            unsafe {
                libc::kill(-pgid, libc::SIGTERM);
            }
        }
    }

    pub fn close(&self) {
        if !self.is_open() {
            return;
        }
        self.stop();
        crate::motion::fade_out(&self.root);
        (self.on_close)();
    }
}

fn rgba(c: tileide_core::theme::Rgb, a: f64) -> gdk::RGBA {
    gdk::RGBA::new(c.0 as f32, c.1 as f32, c.2 as f32, a as f32)
}

fn shell_line(step: &[String]) -> String {
    step.iter().map(|p| if p.contains([' ', '\'', '"']) { format!("'{}'", p.replace('\'', "'\\''")) } else { p.clone() }).collect::<Vec<_>>().join(" ")
}

/// A close-on-exec pipe: (read end, write end).
fn pipe() -> std::io::Result<(std::os::fd::OwnedFd, std::os::fd::OwnedFd)> {
    use std::os::fd::FromRawFd;
    let mut fds = [0; 2];
    // SAFETY: pipe2 fills both fds on success; each is owned exactly once below.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { (std::os::fd::OwnedFd::from_raw_fd(fds[0]), std::os::fd::OwnedFd::from_raw_fd(fds[1])) })
}

/// Runs the steps in order on this (worker) thread; stops at the first failure.
fn execute(plan: &Plan, group: &AtomicI32, stopped: &AtomicBool, tx: &std::sync::mpsc::Sender<Out>) {
    let started = Instant::now();
    let mut code = Some(0);
    for step in &plan.steps {
        let _ = tx.send(Out::Step(shell_line(step)));
        let Some((program, args)) = step.split_first() else { continue };
        let (reader, writer) = match pipe() {
            Ok(p) => p,
            Err(e) => {
                let _ = tx.send(Out::Text(format!("cannot create a pipe: {e}\n")));
                code = Some(127);
                break;
            }
        };
        let child = writer.try_clone().and_then(|for_stderr| {
            Command::new(program)
                .args(args)
                .current_dir(&plan.cwd)
                .env("NO_COLOR", "1")
                .env("CARGO_TERM_COLOR", "never")
                .env("PYTHONUNBUFFERED", "1")
                .envs(plan.env.iter().map(|(k, v)| (k, v)))
                .stdin(Stdio::null())
                .stdout(Stdio::from(writer))
                .stderr(Stdio::from(for_stderr))
                .process_group(0)
                .spawn()
        });
        // The Command (and its copies of the write end) is gone, so the read ends at exit.
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(Out::Text(format!("cannot start {program}: {e}\n")));
                code = Some(127);
                break;
            }
        };
        group.store(child.id() as i32, Ordering::SeqCst);
        let mut pipe = std::fs::File::from(reader);
        let mut chunk = [0u8; 8192];
        // Bytes of a UTF-8 character split across reads wait for the rest.
        let mut pending = Vec::new();
        while let Ok(n) = pipe.read(&mut chunk) {
            if n == 0 {
                break;
            }
            pending.extend_from_slice(&chunk[..n]);
            let valid = match std::str::from_utf8(&pending) {
                Ok(_) => pending.len(),
                Err(e) if e.error_len().is_none() => e.valid_up_to(),
                Err(_) => pending.len(),
            };
            let text = String::from_utf8_lossy(&pending[..valid]).into_owned();
            pending.drain(..valid);
            if tx.send(Out::Text(strip_ansi(&text))).is_err() {
                break;
            }
        }
        code = child.wait().ok().and_then(|s| s.code());
        group.store(0, Ordering::SeqCst);
        if code != Some(0) || stopped.load(Ordering::SeqCst) {
            break;
        }
    }
    let _ = tx.send(Out::Done(code, stopped.load(Ordering::SeqCst), started.elapsed()));
}
