// SPDX-License-Identifier: GPL-3.0-or-later
//! The audio visualizer's source (v0.4 phase 3). `pactl subscribe` tells when playback
//! streams come and go (no polling); while one plays, `parec` captures the default
//! output's monitor (mono, 22 kHz float) and a reader thread feeds the analyzer. No
//! audio library is linked, and nothing records while nothing plays. The bars are
//! shown only while there is sound (`visualizer::Gate`).
//!
//! The capture stream is an input stream: bars or panels that show "recording" for any
//! input may light up while music plays. The Settings switch turns it all off.

use std::cell::{Cell, RefCell};
use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gtk4::glib;
use tileide_core::visualizer::{Analyzer, Gate, RATE};
use serde_json::Value;

const FRAME: Duration = Duration::from_millis(33);
/// Capture keeps running this long after the last stream stops (track changes).
const LINGER: Duration = Duration::from_secs(3);

pub struct Visualizer {
    me: RefCell<std::rc::Weak<Visualizer>>,
    analyzer: Arc<Mutex<Analyzer>>,
    gate: RefCell<Gate>,
    watcher: RefCell<Option<Child>>,
    capture: RefCell<Option<Child>>,
    linger: RefCell<Option<glib::SourceId>>,
    tick: RefCell<Option<glib::SourceId>>,
    /// Bar heights and how shown they are (0..1, fades in and out).
    bars: RefCell<Vec<f64>>,
    presence: Cell<f64>,
    enabled: Cell<bool>,
    on_frame: Rc<dyn Fn()>,
}

impl Visualizer {
    /// `on_frame` runs every frame while the bars move or fade.
    pub fn new(on_frame: Rc<dyn Fn()>) -> Rc<Visualizer> {
        let v = Rc::new(Visualizer {
            me: RefCell::new(std::rc::Weak::new()),
            analyzer: Arc::new(Mutex::new(Analyzer::default())),
            gate: RefCell::new(Gate::default()),
            watcher: RefCell::new(None),
            capture: RefCell::new(None),
            linger: RefCell::new(None),
            tick: RefCell::new(None),
            bars: RefCell::new(Vec::new()),
            presence: Cell::new(0.0),
            enabled: Cell::new(false),
            on_frame,
        });
        *v.me.borrow_mut() = Rc::downgrade(&v);
        v
    }

    /// The bars to draw and their opacity, or `None` while hidden.
    pub fn shown(&self) -> Option<(Vec<f64>, f64)> {
        let k = self.presence.get();
        (k > 0.0).then(|| (self.bars.borrow().clone(), k))
    }

    pub fn set_enabled(&self, enabled: bool) {
        if enabled == self.enabled.get() {
            return;
        }
        self.enabled.set(enabled);
        if enabled {
            self.watch();
        } else {
            for child in [self.watcher.take(), self.capture.take()].into_iter().flatten() {
                stop(child);
            }
            self.presence.set(0.0);
            self.gate.borrow_mut().reset();
            (self.on_frame)();
        }
    }

    pub fn shutdown(&self) {
        self.set_enabled(false);
    }

    /// Starts `pactl subscribe`; every sink-input event re-checks what plays.
    fn watch(&self) {
        let child = with_parent(Command::new("pactl").arg("subscribe")).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                eprintln!("visualizer: cannot run pactl ({e}); no audio visualizer");
                return;
            }
        };
        let Some(stdout) = child.stdout.take() else { return };
        *self.watcher.borrow_mut() = Some(child);
        let me = self.me.borrow().clone();
        // The thread only sends a marker; the main loop owns the state.
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let _ = tx.send(());
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line.contains("sink-input") && tx.send(()).is_err() {
                    break;
                }
            }
        });
        glib::timeout_add_local(Duration::from_millis(250), move || {
            let Some(v) = me.upgrade() else { return glib::ControlFlow::Break };
            if !v.enabled.get() {
                return glib::ControlFlow::Break;
            }
            if rx.try_iter().count() > 0 {
                v.check();
            }
            glib::ControlFlow::Continue
        });
    }

    /// Asks (off the main thread) whether any playback stream is running.
    fn check(&self) {
        let me = self.me.borrow().clone();
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        std::thread::spawn(move || {
            let _ = tx.send(playing());
        });
        glib::timeout_add_local(Duration::from_millis(30), move || match rx.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            result => {
                if let (Some(v), Ok(playing)) = (me.upgrade(), result) {
                    v.set_playing(playing);
                }
                glib::ControlFlow::Break
            }
        });
    }

    fn set_playing(&self, playing: bool) {
        if !self.enabled.get() {
            return;
        }
        if let Some(id) = self.linger.take() {
            id.remove();
        }
        if playing {
            if self.capture.borrow().is_none() {
                self.start_capture();
            }
            return;
        }
        let me = self.me.borrow().clone();
        let id = glib::timeout_add_local_once(LINGER, move || {
            if let Some(v) = me.upgrade() {
                v.linger.take();
                if let Some(child) = v.capture.take() {
                    stop(child);
                }
            }
        });
        *self.linger.borrow_mut() = Some(id);
    }

    fn start_capture(&self) {
        let device = std::env::var("TILEIDE_AUDIO_DEVICE").unwrap_or_else(|_| "@DEFAULT_MONITOR@".into());
        let child = with_parent(&mut Command::new("parec"))
            .args([format!("--device={device}"), format!("--rate={}", RATE as u32), "--channels=1".into(), "--format=float32le".into(), "--latency-msec=20".into(), "--raw".into()])
            .args(["--client-name=TileIDE", "--stream-name=Audio visualizer"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                eprintln!("visualizer: cannot run parec ({e})");
                return;
            }
        };
        let Some(mut stdout) = child.stdout.take() else { return };
        *self.capture.borrow_mut() = Some(child);
        let analyzer = self.analyzer.clone();
        std::thread::spawn(move || {
            let mut bytes = [0u8; 2048];
            let mut samples = Vec::with_capacity(512);
            while let Ok(n) = stdout.read(&mut bytes) {
                if n == 0 {
                    break;
                }
                samples.clear();
                samples.extend(bytes[..n - n % 4].chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])));
                if let Ok(mut a) = analyzer.lock() {
                    a.feed(&samples);
                }
            }
        });
        self.start_tick();
    }

    /// Animates while capturing or while the bars fade out.
    fn start_tick(&self) {
        if self.tick.borrow().is_some() {
            return;
        }
        let me = self.me.borrow().clone();
        let id = glib::timeout_add_local(FRAME, move || {
            let Some(v) = me.upgrade() else { return glib::ControlFlow::Break };
            let capturing = v.capture.borrow().is_some();
            let (bars, rms) = match v.analyzer.lock() {
                Ok(mut a) => {
                    if capturing { a.update() } else { a.decay() }
                    (a.bars(), a.rms())
                }
                Err(_) => return glib::ControlFlow::Break,
            };
            let visible = capturing && v.gate.borrow_mut().update(rms, glib::monotonic_time());
            // Same feel as the rest of the motion: ~200 ms in, ~150 ms out.
            let k = v.presence.get();
            let k = if visible { (k + 0.17).min(1.0) } else { (k - 0.22).max(0.0) };
            v.presence.set(k);
            *v.bars.borrow_mut() = bars;
            (v.on_frame)();
            if !capturing && k == 0.0 {
                v.tick.take();
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
        *self.tick.borrow_mut() = Some(id);
    }
}

/// The helper ends when TileIDE does, however it ends (a crash or a kill skips `shutdown`).
fn with_parent(cmd: &mut Command) -> &mut Command {
    use std::os::unix::process::CommandExt;
    // SAFETY: prctl is async-signal-safe; nothing else runs between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        })
    }
}

fn stop(mut child: Child) {
    let _ = child.kill();
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

/// Whether any playback stream is running (not corked) right now.
fn playing() -> bool {
    let Ok(out) = Command::new("pactl").args(["-f", "json", "list", "sink-inputs"]).stderr(Stdio::null()).output() else { return false };
    let Ok(Value::Array(inputs)) = serde_json::from_slice::<Value>(&out.stdout) else { return false };
    inputs.iter().any(|i| i["corked"] == Value::Bool(false) && i["properties"]["application.name"].as_str() != Some("TileIDE"))
}
