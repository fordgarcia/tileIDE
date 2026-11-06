// SPDX-License-Identifier: GPL-3.0-or-later
//! Java debugging (v0.4 phase 5) through `jdb`, which every JDK ships: a small DAP
//! adapter (`tileide dap-jdb`) that turns DAP requests into jdb commands and jdb's text
//! back into DAP. No Java debug adapter (or language server) has to be installed.
//!
//! jdb prints a prompt when it waits for a command: `> ` with no current thread,
//! `main[1] ` while a thread is suspended. Commands are sent one at a time, each
//! answered by the lines up to the next prompt. While the program runs, its output is
//! forwarded line by line and jdb's own event lines ("Breakpoint hit: …") become
//! `stopped` events.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tileide_lsp::rpc::{encode, read_message};

/// The `package` a Java source declares (`package a.b;`), if any.
pub fn package_of(source: &str) -> Option<String> {
    let mut in_comment = false;
    for line in source.lines() {
        let mut line = line.trim();
        if in_comment {
            let Some(end) = line.find("*/") else { continue };
            line = line[end + 2..].trim();
            in_comment = false;
        }
        if line.starts_with("/*") {
            in_comment = !line.contains("*/");
            continue;
        }
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let rest = line.strip_prefix("package")?.trim_start();
        return rest.split(';').next().map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
    }
    None
}

/// The source root of a file in `dir` declaring `package`: `dir` without the package's
/// folders when it ends with them (`src/com/x` for `com.x` → `src`), else `dir`.
pub fn source_root(dir: &Path, package: Option<&str>) -> PathBuf {
    let Some(package) = package else { return dir.to_path_buf() };
    let parts: Vec<&str> = package.split('.').collect();
    let mut root = dir.to_path_buf();
    for part in parts.iter().rev() {
        if !root.ends_with(part) {
            return dir.to_path_buf();
        }
        root.pop();
    }
    root
}

enum Msg {
    Dap(Value),
    DapClosed,
    /// One complete line of jdb's output (prompts removed).
    Line(String),
    /// jdb waits for a command.
    Prompt,
    JdbClosed,
}

/// Splits jdb's stdout into lines and prompts. A `name[n] ` prompt only counts for a
/// thread name jdb has reported, so program output like `a[0] = 1` stays output.
fn read_jdb(mut out: impl Read, tx: Sender<Msg>, threads: Arc<Mutex<HashSet<String>>>) {
    let mut buf = String::new();
    let mut bytes = [0u8; 4096];
    let mut pending: Vec<u8> = Vec::new();
    let prompt_len = |s: &str| -> Option<usize> {
        if s.starts_with("> ") {
            return Some(2);
        }
        let open = s.find('[')?;
        let close = open + s[open..].find("] ")?;
        let (name, n) = (&s[..open], &s[open + 1..close]);
        let known = threads.lock().is_ok_and(|t| t.contains(name));
        (known && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then_some(close + 2)
    };
    loop {
        let n = match out.read(&mut bytes) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        pending.extend_from_slice(&bytes[..n]);
        // Keep an incomplete UTF-8 sequence for the next read.
        let valid = match std::str::from_utf8(&pending) {
            Ok(s) => s.len(),
            Err(e) => e.valid_up_to(),
        };
        buf.push_str(std::str::from_utf8(&pending[..valid]).unwrap_or_default());
        pending.drain(..valid);
        loop {
            if let Some(len) = prompt_len(&buf) {
                buf.drain(..len);
                let _ = tx.send(Msg::Prompt);
                continue;
            }
            let Some(end) = buf.find('\n') else { break };
            let line: String = buf.drain(..=end).collect();
            let line = line.trim_end_matches(['\n', '\r']).to_string();
            if let Some(t) = line.find("\"thread=") {
                let name = &line[t + 8..];
                if let Some(q) = name.find('"')
                    && let Ok(mut threads) = threads.lock()
                {
                    threads.insert(name[..q].to_string());
                }
            }
            let _ = tx.send(Msg::Line(line));
        }
    }
    if !buf.is_empty() {
        let _ = tx.send(Msg::Line(std::mem::take(&mut buf)));
    }
    let _ = tx.send(Msg::JdbClosed);
}

/// A frame from `where`: `  [1] com.x.Main.add (Main.java:3)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub index: i64,
    /// `com.x.Main.add`.
    pub name: String,
    /// `com.x.Main`.
    pub class: String,
    pub file: Option<String>,
    pub line: i64,
}

pub fn parse_frame(line: &str) -> Option<Frame> {
    let line = line.trim();
    let rest = line.strip_prefix('[')?;
    let close = rest.find(']')?;
    let index = rest[..close].parse().ok()?;
    let rest = rest[close + 1..].trim();
    let (name, place) = rest.split_once(" (")?;
    let place = place.trim_end_matches(')');
    let class = name.rsplit_once('.').map_or(name, |(c, _)| c).to_string();
    let (file, line) = match place.rsplit_once(':') {
        Some((file, n)) => (Some(file.to_string()), n.replace(',', "").parse().unwrap_or(0)),
        None => (None, 0),
    };
    Some(Frame { index, name: name.to_string(), class, file, line })
}

/// `name = value` lines of `locals`, both sections (arguments, then locals).
pub fn parse_locals(lines: &[String]) -> Vec<(String, String)> {
    lines.iter().filter_map(|l| l.split_once(" = ")).filter(|(n, _)| !n.contains(' ') && !n.is_empty()).map(|(n, v)| (n.to_string(), v.to_string())).collect()
}

/// The children `dump` prints: object fields (`name: value`) or array elements
/// (`1, 2, 3`, named `[0]`, `[1]`, …).
pub fn parse_dump(lines: &[String]) -> Vec<(String, String)> {
    let body: Vec<&str> = lines.iter().map(|l| l.trim()).skip_while(|l| !l.ends_with('{')).skip(1).take_while(|l| *l != "}").collect();
    let fields: Vec<(String, String)> = body.iter().filter_map(|l| l.split_once(": ")).map(|(n, v)| (n.to_string(), v.to_string())).collect();
    if !fields.is_empty() && fields.len() == body.len() {
        return fields;
    }
    body.join(" ").split(", ").filter(|v| !v.trim().is_empty()).enumerate().map(|(i, v)| (format!("[{i}]"), v.trim().to_string())).collect()
}

/// What a variables reference expands: a frame's locals, or an expression's fields.
#[derive(Clone)]
enum Reference {
    Locals(i64),
    Expr(i64, String),
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Configuring,
    Running,
    Paused,
}

struct Bridge {
    out: Box<dyn Write>,
    rx: Receiver<Msg>,
    /// DAP requests that arrived while waiting for jdb.
    queued: VecDeque<Value>,
    jdb: Option<(Child, ChildStdin)>,
    seq: i64,
    state: State,
    /// The frame jdb's `up`/`down` point at (1 = top).
    frame: i64,
    thread: String,
    source_root: PathBuf,
    /// Breakpoints per source file, as jdb names them (`com.x.Main:12`).
    breakpoints: HashMap<String, Vec<String>>,
    refs: BTreeMap<i64, Reference>,
    /// Why the program last stopped, held until jdb's prompt.
    stop: Option<(String, String)>,
    /// The last resume was a step (a step into JDK code goes on).
    stepping: bool,
    /// Skips the source line jdb echoes after a stop.
    skip_echo: bool,
    failed: bool,
}

impl Bridge {
    fn send(&mut self, mut message: Value) {
        self.seq += 1;
        message["seq"] = json!(self.seq);
        let _ = self.out.write_all(&encode(&message));
        let _ = self.out.flush();
    }

    fn event(&mut self, event: &str, body: Value) {
        self.send(json!({ "type": "event", "event": event, "body": body }));
    }

    fn reply(&mut self, request: &Value, result: Result<Value, String>) {
        let mut r = json!({ "type": "response", "request_seq": request["seq"], "command": request["command"], "success": result.is_ok() });
        match result {
            Ok(body) => r["body"] = body,
            Err(message) => r["message"] = json!(message),
        }
        self.send(r);
    }

    fn output(&mut self, text: &str) {
        self.event("output", json!({ "category": "stdout", "output": format!("{text}\n") }));
    }

    /// Sends one command and returns jdb's answer (the lines up to its next prompt).
    fn command(&mut self, cmd: &str) -> Vec<String> {
        let Some((_, stdin)) = self.jdb.as_mut() else { return Vec::new() };
        if writeln!(stdin, "{cmd}").and_then(|()| stdin.flush()).is_err() {
            return Vec::new();
        }
        let mut lines = Vec::new();
        while let Ok(msg) = self.rx.recv() {
            match msg {
                Msg::Line(l) => lines.push(l),
                Msg::Prompt => break,
                Msg::Dap(v) => self.queued.push_back(v),
                Msg::DapClosed => self.queued.push_back(json!({ "command": "disconnect" })),
                Msg::JdbClosed => {
                    self.ended(&lines);
                    break;
                }
            }
        }
        lines
    }

    /// A command that resumes the program; its events arrive later.
    fn resume(&mut self, cmd: &str, step: bool) {
        self.state = State::Running;
        self.stepping = step;
        self.refs.clear();
        if let Some((_, stdin)) = self.jdb.as_mut() {
            let _ = writeln!(stdin, "{cmd}").and_then(|()| stdin.flush());
        }
    }

    /// Points jdb at frame `index` (1 = top).
    fn goto_frame(&mut self, index: i64) {
        if index > self.frame {
            self.command(&format!("up {}", index - self.frame));
        } else if index < self.frame {
            self.command(&format!("down {}", self.frame - index));
        }
        self.frame = index;
    }

    fn source_of(&self, frame: &Frame) -> Option<PathBuf> {
        let file = frame.file.as_ref()?;
        let package = frame.class.rsplit_once('.').map(|(p, _)| p.replace('.', "/"));
        let path = match package {
            Some(p) => self.source_root.join(p).join(file),
            None => self.source_root.join(file),
        };
        path.is_file().then_some(path)
    }

    fn frames(&mut self) -> Vec<Frame> {
        self.command("where").iter().filter_map(|l| parse_frame(l)).collect()
    }

    fn ended(&mut self, lines: &[String]) {
        for l in lines {
            self.output(l);
        }
        if self.jdb.take().is_some() {
            self.event("exited", json!({ "exitCode": i64::from(self.failed) }));
            self.event("terminated", json!({}));
        }
    }

    /// A line jdb printed while the program ran.
    fn running_line(&mut self, line: String) {
        let line = line.strip_prefix("VM Started: ").map(str::to_string).unwrap_or(line);
        if self.skip_echo {
            self.skip_echo = false;
            if line.split_whitespace().next().is_some_and(|n| n.replace(',', "").parse::<u64>().is_ok()) {
                return;
            }
        }
        let event = [("Breakpoint hit:", "breakpoint"), ("Step completed:", "step"), ("Exception occurred:", "exception")].into_iter().find(|(p, _)| line.starts_with(p));
        if let Some((_, reason)) = event {
            if reason == "exception" {
                self.failed = true;
            }
            self.stop = Some((reason.to_string(), line.clone()));
            self.skip_echo = true;
            return;
        }
        let noise = ["Set uncaught ", "Set deferred uncaught ", "Set deferred breakpoint ", "Set breakpoint ", "run ", "Removed: "];
        if line.is_empty() || noise.iter().any(|n| line.starts_with(n)) {
            return;
        }
        if line == "The application exited" {
            self.command("exit");
            self.ended(&[]);
            return;
        }
        self.output(&line);
    }

    /// jdb prompted while the program ran: a stop, or just the prompt after a resume.
    fn running_prompt(&mut self) {
        let Some((reason, line)) = self.stop.take() else { return };
        self.state = State::Paused;
        self.frame = 1;
        if let Some(t) = line.find("\"thread=") {
            let name = &line[t + 8..];
            self.thread = name[..name.find('"').unwrap_or(name.len())].to_string();
        }
        // A step that left the user's code (into the JDK, or out of main) goes on.
        if self.stepping && reason == "step" {
            let frames = self.frames();
            let top_is_user = frames.first().is_some_and(|f| self.source_of(f).is_some());
            if !top_is_user {
                let user_below = frames.iter().any(|f| self.source_of(f).is_some());
                self.resume(if user_below { "step up" } else { "cont" }, user_below);
                return;
            }
        }
        let text = line.split_once(": ").map_or(line.as_str(), |(_, t)| t).to_string();
        self.event("stopped", json!({ "reason": reason, "threadId": 1, "allThreadsStopped": true, "description": text, "text": text }));
    }

    fn breakpoints(&mut self, args: &Value) -> Result<Value, String> {
        let path = PathBuf::from(args["source"]["path"].as_str().ok_or("no source path")?);
        let source = std::fs::read_to_string(&path).unwrap_or_default();
        let stem = path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        let class = package_of(&source).map_or_else(|| stem.clone(), |p| format!("{p}.{stem}"));
        let key = path.to_string_lossy().into_owned();
        for old in self.breakpoints.remove(&key).unwrap_or_default() {
            self.command(&format!("clear {old}"));
        }
        let mut set = Vec::new();
        let mut result = Vec::new();
        for bp in args["breakpoints"].as_array().cloned().unwrap_or_default() {
            let line = bp["line"].as_i64().unwrap_or(0);
            let spot = format!("{class}:{line}");
            let answer = self.command(&format!("stop at {spot}")).join("\n");
            let verified = !answer.contains("Unable to set");
            if verified {
                set.push(spot);
            }
            result.push(json!({ "verified": verified, "line": line, "message": (!verified).then_some(answer) }));
        }
        self.breakpoints.insert(key, set);
        Ok(json!({ "breakpoints": result }))
    }

    fn variables(&mut self, reference: i64) -> Vec<Value> {
        let Some(r) = self.refs.get(&reference).cloned() else { return Vec::new() };
        let (frame, prefix, pairs) = match r {
            Reference::Locals(frame) => {
                self.goto_frame(frame);
                let lines = self.command("locals");
                (frame, None, parse_locals(&lines))
            }
            Reference::Expr(frame, expr) => {
                self.goto_frame(frame);
                let lines = self.command(&format!("dump {expr}"));
                (frame, Some(expr), parse_dump(&lines))
            }
        };
        pairs
            .into_iter()
            .map(|(name, value)| {
                let expr = match &prefix {
                    None => Some(name.clone()),
                    Some(p) if name.starts_with('[') => Some(format!("{p}{name}")),
                    // Inherited fields print qualified (`java.util.AbstractList.modCount`).
                    Some(_) if name.contains('.') => None,
                    Some(p) => Some(format!("{p}.{name}")),
                };
                let child = match expr.filter(|_| value.starts_with("instance of ")) {
                    Some(expr) => {
                        let id = self.refs.keys().next_back().map_or(1000, |k| k + 1);
                        self.refs.insert(id, Reference::Expr(frame, expr));
                        id
                    }
                    None => 0,
                };
                json!({ "name": name, "value": value, "variablesReference": child })
            })
            .collect()
    }

    fn launch(&mut self, args: &Value, tx: Sender<Msg>) -> Result<Value, String> {
        let class = args["mainClass"].as_str().ok_or("no mainClass")?;
        let classpath = args["classPath"].as_str().unwrap_or(".");
        self.source_root = PathBuf::from(args["sourceRoot"].as_str().unwrap_or("."));
        let cwd = args["cwd"].as_str().map_or_else(|| self.source_root.clone(), PathBuf::from);
        let mut cmd = Command::new("jdb");
        cmd.args(["-classpath", classpath, "-sourcepath"]).arg(&self.source_root).arg(class).current_dir(&cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        // A launch config's arguments (after the class, for `main`) and environment.
        cmd.args(args["args"].as_array().into_iter().flatten().filter_map(Value::as_str));
        cmd.envs(args["env"].as_object().into_iter().flatten().filter_map(|(k, v)| Some((k, v.as_str()?))));
        // jdb (and the program it started) end with the bridge.
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        let mut child = cmd.spawn().map_err(|e| format!("cannot start jdb: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let threads = Arc::new(Mutex::new(HashSet::from(["main".to_string()])));
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let t = tx.clone();
        std::thread::spawn(move || read_jdb(stdout, t, threads));
        if let Some(stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let _ = tx.send(Msg::Line(line));
                }
            });
        }
        self.jdb = Some((child, stdin));
        // "Initializing jdb ..." up to the first prompt.
        let mut lines = Vec::new();
        while let Ok(msg) = self.rx.recv() {
            match msg {
                Msg::Prompt => return Ok(json!({})),
                Msg::Line(l) => lines.push(l),
                Msg::Dap(v) => self.queued.push_back(v),
                Msg::DapClosed | Msg::JdbClosed => break,
            }
        }
        self.jdb = None;
        Err(format!("jdb didn't start: {}", lines.join(" ")))
    }

    fn handle(&mut self, request: Value, tx: &Sender<Msg>) -> bool {
        let args = &request["arguments"];
        let result = match request["command"].as_str().unwrap_or("") {
            "initialize" => Ok(json!({ "supportsConfigurationDoneRequest": true })),
            "launch" => {
                let r = self.launch(args, tx.clone());
                let ok = r.is_ok();
                self.reply(&request, r);
                if ok {
                    self.event("initialized", json!({}));
                }
                return true;
            }
            "setBreakpoints" => self.breakpoints(args),
            "setExceptionBreakpoints" => Ok(json!({})),
            "configurationDone" => {
                self.reply(&request, Ok(json!({})));
                self.resume("run", false);
                return true;
            }
            "threads" => Ok(json!({ "threads": [{ "id": 1, "name": self.thread }] })),
            "stackTrace" if self.state == State::Paused => {
                let frames = self.frames();
                let frames: Vec<Value> = frames
                    .iter()
                    .map(|f| {
                        let source = self.source_of(f).map(|p| json!({ "name": f.file, "path": p }));
                        json!({ "id": f.index, "name": f.name, "line": f.line, "column": 1, "source": source })
                    })
                    .collect();
                Ok(json!({ "stackFrames": frames, "totalFrames": frames.len() }))
            }
            "scopes" => {
                let frame = args["frameId"].as_i64().unwrap_or(1);
                let id = self.refs.keys().next_back().map_or(1000, |k| k + 1);
                self.refs.insert(id, Reference::Locals(frame));
                Ok(json!({ "scopes": [{ "name": "Locals", "variablesReference": id, "expensive": false }] }))
            }
            "variables" if self.state == State::Paused => {
                let vars = self.variables(args["variablesReference"].as_i64().unwrap_or(0));
                Ok(json!({ "variables": vars }))
            }
            "continue" if self.state == State::Paused => {
                self.reply(&request, Ok(json!({ "allThreadsContinued": true })));
                self.resume("cont", false);
                return true;
            }
            cmd @ ("next" | "stepIn" | "stepOut") if self.state == State::Paused => {
                self.reply(&request, Ok(json!({})));
                self.resume(
                    match cmd {
                        "next" => "next",
                        "stepIn" => "step",
                        _ => "step up",
                    },
                    true,
                );
                return true;
            }
            "disconnect" => {
                if let Some((mut child, mut stdin)) = self.jdb.take() {
                    let _ = writeln!(stdin, "exit").and_then(|()| stdin.flush());
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    let _ = child.kill();
                    let _ = child.wait();
                }
                self.reply(&request, Ok(json!({})));
                return false;
            }
            "stackTrace" | "variables" | "continue" | "next" | "stepIn" | "stepOut" => Err("the program is not paused".into()),
            other => Err(format!("{other} is not supported by the jdb bridge")),
        };
        self.reply(&request, result);
        true
    }
}

/// Serves DAP on `input`/`output` until `disconnect` or end of input (`tileide dap-jdb`).
pub fn serve(input: impl Read + Send + 'static, output: impl Write + 'static) {
    let (tx, rx) = channel();
    let t = tx.clone();
    std::thread::spawn(move || {
        let mut input = BufReader::new(input);
        while let Ok(Some(message)) = read_message(&mut input) {
            if t.send(Msg::Dap(message)).is_err() {
                return;
            }
        }
        let _ = t.send(Msg::DapClosed);
    });
    let mut b = Bridge {
        out: Box::new(output),
        rx,
        queued: VecDeque::new(),
        jdb: None,
        seq: 0,
        state: State::Configuring,
        frame: 1,
        thread: "main".into(),
        source_root: PathBuf::from("."),
        breakpoints: HashMap::new(),
        refs: BTreeMap::new(),
        stop: None,
        stepping: false,
        skip_echo: false,
        failed: false,
    };
    loop {
        if let Some(request) = b.queued.pop_front() {
            if !b.handle(request, &tx) {
                return;
            }
            continue;
        }
        let Ok(msg) = b.rx.recv() else { return };
        match msg {
            Msg::Dap(request) => {
                if !b.handle(request, &tx) {
                    return;
                }
            }
            Msg::DapClosed => {
                b.handle(json!({ "command": "disconnect" }), &tx);
                return;
            }
            Msg::Line(line) if b.state == State::Running => b.running_line(line),
            Msg::Line(line) => b.output(&line),
            Msg::Prompt if b.state == State::Running => b.running_prompt(),
            Msg::Prompt => {}
            Msg::JdbClosed => b.ended(&[]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_packages_and_source_roots() {
        assert_eq!(package_of("/* header\n package no; */\n// c\npackage com.x.app;\nclass A {}").as_deref(), Some("com.x.app"));
        assert_eq!(package_of("import java.util.*;\nclass A {}"), None);
        assert_eq!(source_root(Path::new("/p/src/com/x"), Some("com.x")), PathBuf::from("/p/src"));
        assert_eq!(source_root(Path::new("/p/other"), Some("com.x")), PathBuf::from("/p/other"));
        assert_eq!(source_root(Path::new("/p"), None), PathBuf::from("/p"));
    }

    #[test]
    fn parses_frames_locals_and_dumps() {
        let f = parse_frame("  [2] com.x.Main.main (Main.java:1,207)").unwrap();
        assert_eq!((f.index, f.class.as_str(), f.file.as_deref(), f.line), (2, "com.x.Main", Some("Main.java"), 1207));
        assert_eq!(parse_frame("  [3] java.lang.Thread.run (native method)").unwrap().file, None);
        let lines = |s: &str| s.lines().map(str::to_string).collect::<Vec<_>>();
        let locals = parse_locals(&lines("Method arguments:\nargs = instance of java.lang.String[0] (id=449)\nLocal variables:\na = 41\nname = \"t = 1\""));
        assert_eq!(locals[1], ("a".into(), "41".into()));
        assert_eq!(locals[2].1, "\"t = 1\"");
        let obj = parse_dump(&lines(" p = {\n    count: 3\n    xs: instance of java.util.ArrayList(id=463)\n}"));
        assert_eq!(obj, vec![("count".into(), "3".into()), ("xs".into(), "instance of java.util.ArrayList(id=463)".into())]);
        assert_eq!(parse_dump(&lines(" arr = {\n1, 2, 3\n}"))[2], ("[2]".into(), "3".into()));
    }

    /// End to end with the JDK's jdb on a small program; skipped without javac/jdb.
    #[test]
    fn stops_steps_and_reads_variables() {
        let on = tileide_core::run::on_path;
        if !on("javac") || !on("jdb") {
            eprintln!("javac or jdb missing: skipped");
            return;
        }
        let dir = std::env::temp_dir().join(format!("tileide-jdb-{}", std::process::id()));
        let src = dir.join("src/demo");
        std::fs::create_dir_all(&src).unwrap();
        let file = src.join("Main.java");
        std::fs::write(&file, "package demo;\npublic class Main {\n    static int add(int x, int y) {\n        int s = x + y;\n        return s;\n    }\n    public static void main(String[] args) {\n        int[] arr = {1, 2, 3};\n        int a = 41;\n        a = add(a, 1);\n        System.out.println(\"a[0] = \" + a);\n    }\n}\n").unwrap();
        let classes = dir.join("classes");
        assert!(Command::new("javac").args(["-g", "-d"]).arg(&classes).arg("-sourcepath").arg(dir.join("src")).arg(&file).status().unwrap().success());

        let (to_bridge, from_test) = std::os::unix::net::UnixStream::pair().unwrap();
        std::thread::spawn(move || serve(from_test.try_clone().unwrap(), from_test));
        let mut writer = to_bridge.try_clone().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut r = BufReader::new(to_bridge);
            while let Ok(Some(m)) = read_message(&mut r) {
                let _ = tx.send(m);
            }
        });
        let mut seq = 0;
        let mut request = |command: &str, arguments: Value| {
            seq += 1;
            writer.write_all(&encode(&json!({ "seq": seq, "type": "request", "command": command, "arguments": arguments }))).unwrap();
            seq
        };
        let mut output = String::new();
        let mut next = |pred: &dyn Fn(&Value) -> bool| loop {
            let m = rx.recv_timeout(std::time::Duration::from_secs(30)).expect("bridge message");
            if m["event"] == "output" {
                output += m["body"]["output"].as_str().unwrap_or("");
            }
            if pred(&m) {
                return (m, output.clone());
            }
        };
        request("initialize", json!({}));
        request("launch", json!({ "mainClass": "demo.Main", "classPath": classes, "sourceRoot": dir.join("src"), "cwd": dir }));
        next(&|m| m["event"] == "initialized");
        let bp = request("setBreakpoints", json!({ "source": { "path": file }, "breakpoints": [{ "line": 4 }, { "line": 99 }] }));
        let (bp, _) = next(&|m| m["request_seq"] == bp);
        assert_eq!(bp["body"]["breakpoints"][0]["verified"], true);
        request("configurationDone", json!({}));
        let (stopped, _) = next(&|m| m["event"] == "stopped");
        assert_eq!(stopped["body"]["reason"], "breakpoint");
        let st = request("stackTrace", json!({ "threadId": 1 }));
        let (st, _) = next(&|m| m["request_seq"] == st);
        let frames = &st["body"]["stackFrames"];
        assert_eq!((frames[0]["line"].as_i64(), frames[0]["name"].as_str()), (Some(4), Some("demo.Main.add")));
        assert_eq!(frames[0]["source"]["path"].as_str(), Some(file.to_str().unwrap()));
        // The caller's locals: frame 2, then its array's elements.
        let sc = request("scopes", json!({ "frameId": 2 }));
        let (sc, _) = next(&|m| m["request_seq"] == sc);
        let vars = request("variables", json!({ "variablesReference": sc["body"]["scopes"][0]["variablesReference"] }));
        let (vars, _) = next(&|m| m["request_seq"] == vars);
        let vars = vars["body"]["variables"].as_array().unwrap().clone();
        let a = vars.iter().find(|v| v["name"] == "a").unwrap();
        assert_eq!(a["value"], "41");
        let arr = vars.iter().find(|v| v["name"] == "arr").unwrap();
        let items = request("variables", json!({ "variablesReference": arr["variablesReference"] }));
        let (items, _) = next(&|m| m["request_seq"] == items);
        assert_eq!(items["body"]["variables"][2]["value"], "3");
        // Step out of add, back in main.
        request("stepOut", json!({ "threadId": 1 }));
        next(&|m| m["event"] == "stopped");
        let st = request("stackTrace", json!({ "threadId": 1 }));
        let (st, _) = next(&|m| m["request_seq"] == st);
        assert_eq!(st["body"]["stackFrames"][0]["name"], "demo.Main.main");
        request("continue", json!({ "threadId": 1 }));
        let (_, output) = next(&|m| m["event"] == "terminated");
        assert!(output.contains("a[0] = 42"), "program output kept: {output:?}");
        request("disconnect", json!({}));
        let _ = std::fs::remove_dir_all(dir);
    }
}
