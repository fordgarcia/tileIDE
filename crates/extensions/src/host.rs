// SPDX-License-Identifier: GPL-3.0-or-later
//! One running extension: a child process speaking JSON-RPC 2.0 over stdin/stdout with
//! `Content-Length` framing (the LSP framing, `tileide_lsp::rpc`). Its stderr goes to a
//! log file. A reader thread hands messages to `deliver`; nothing here blocks the UI.
//!
//! Host → extension: `initialize` {hostVersion, extensionDir, workspaceFolder},
//! `command/run` {command, file, language, text}, `shutdown`, then `exit`.
//! Extension → host (notifications): `window/notice` {message}, `editor/insert` {text}.
//! A request from the extension gets "method not found".

use std::fs::File;
use std::io::{BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tileide_lsp::rpc::{encode, read_message};

#[derive(Debug)]
pub enum Incoming {
    /// A response or a notification.
    Message(Value),
    /// Its stdout closed: it exited or crashed.
    Exited,
}

pub struct Process {
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<ChildStdin>>,
    pub pid: u32,
    pub started: Instant,
    next_id: i64,
}

fn write(stdin: &Mutex<ChildStdin>, message: &Value) -> Result<(), String> {
    let mut stdin = stdin.lock().map_err(|_| "extension stdin poisoned".to_string())?;
    stdin.write_all(&encode(message)).and_then(|()| stdin.flush()).map_err(|e| format!("write failed: {e}"))
}

pub fn request(id: i64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

pub fn notification(method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

/// `run` resolved against the extension's folder: `./x` is inside it, anything else is
/// looked up on PATH.
pub fn command_line(run: &[String], dir: &Path) -> Vec<String> {
    run.iter()
        .enumerate()
        .map(|(i, part)| match part.strip_prefix("./") {
            Some(rest) if i == 0 || dir.join(rest).exists() => dir.join(rest).to_string_lossy().into_owned(),
            _ => part.clone(),
        })
        .collect()
}

impl Process {
    /// Starts the extension in `dir` (its own folder); stderr is appended to `log`.
    pub fn spawn(run: &[String], dir: &Path, log: &Path, deliver: impl Fn(Incoming) + Send + 'static) -> Result<Process, String> {
        let cmd = command_line(run, dir);
        let (program, args) = cmd.split_first().ok_or("empty \"run\"")?;
        if let Some(parent) = log.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let stderr = File::create(log).map(Stdio::from).unwrap_or_else(|_| Stdio::null());
        let mut child = Command::new(program)
            .args(args)
            .current_dir(dir)
            .env("TILEIDE_EXTENSION_DIR", dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()
            .map_err(|e| format!("cannot start {program}: {e}"))?;
        let pid = child.id();
        let stdin = Arc::new(Mutex::new(child.stdin.take().ok_or("no stdin")?));
        let mut stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        let replies = stdin.clone();
        std::thread::spawn(move || {
            while let Ok(Some(message)) = read_message(&mut stdout) {
                if message.get("method").is_some()
                    && let Some(id) = message.get("id")
                {
                    let refuse = json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "method not found" } });
                    let _ = write(&replies, &refuse);
                    continue;
                }
                deliver(Incoming::Message(message));
            }
            deliver(Incoming::Exited);
        });
        Ok(Process { child: Arc::new(Mutex::new(child)), stdin, pid, started: Instant::now(), next_id: 0 })
    }

    /// Sends a request; returns its id (the reply arrives through `deliver`).
    pub fn request(&mut self, method: &str, params: Value) -> Result<i64, String> {
        self.next_id += 1;
        write(&self.stdin, &request(self.next_id, method, params))?;
        Ok(self.next_id)
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        write(&self.stdin, &notification(method, params))
    }

    /// Freezes (`SIGSTOP`) or thaws (`SIGCONT`) the process: a suspended extension uses
    /// no CPU and answers nothing until resumed.
    pub fn set_suspended(&self, suspended: bool) -> Result<(), String> {
        let signal = if suspended { libc::SIGSTOP } else { libc::SIGCONT };
        // SAFETY: plain kill(2) on our own child's pid.
        if unsafe { libc::kill(self.pid as libc::pid_t, signal) } == 0 { Ok(()) } else { Err(std::io::Error::last_os_error().to_string()) }
    }

    /// `shutdown` + `exit`, then a kill if it hasn't gone within `grace`. Runs on its
    /// own thread, so quitting never waits on an extension.
    pub fn stop(mut self, grace: Duration) {
        let _ = self.set_suspended(false);
        let _ = self.request("shutdown", Value::Null);
        let _ = self.notify("exit", Value::Null);
        let child = self.child.clone();
        std::thread::spawn(move || {
            let deadline = Instant::now() + grace;
            while Instant::now() < deadline {
                if child.lock().map(|mut c| c.try_wait().ok().flatten().is_some()).unwrap_or(true) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if let Ok(mut c) = child.lock() {
                let _ = c.kill();
                let _ = c.wait();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_programs_inside_the_folder() {
        let dir = Path::new("/ext/a");
        assert_eq!(command_line(&["./bin".into(), "-v".into()], dir), ["/ext/a/bin", "-v"]);
        // An argument is only rewritten when the file exists.
        assert_eq!(command_line(&["python3".into(), "./main.py".into()], dir), ["python3", "./main.py"]);
    }
}

/// The shipped sample (`examples/extensions/word-count`) end to end; skipped without python3.
#[cfg(test)]
mod sample {
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn word_count_runs_a_command() {
        if Command::new("python3").arg("-c").arg("pass").status().is_err() {
            eprintln!("python3 missing: skipped");
            return;
        }
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/word-count");
        let installed = crate::read_installed(&dir).unwrap();
        let log = std::env::temp_dir().join(format!("tileide-ext-sample-{}.log", std::process::id()));
        let (tx, rx) = mpsc::channel();
        let mut p = Process::spawn(&installed.manifest.run, &dir, &log, move |m| {
            let _ = tx.send(m);
        })
        .unwrap();
        let next = || match rx.recv_timeout(Duration::from_secs(5)).expect("a reply") {
            Incoming::Message(m) => m,
            Incoming::Exited => panic!("exited early; see {}", log.display()),
        };
        let id = p.request("initialize", json!({ "hostVersion": "test" })).unwrap();
        assert_eq!(next()["id"], id);
        p.request("command/run", json!({ "command": "word-count.count", "file": "/x/a.txt", "text": "one two\nthree\n" })).unwrap();
        let notice = next();
        assert_eq!(notice["method"], "window/notice");
        assert_eq!(notice["params"]["message"], "a.txt: 3 words, 2 lines, 14 characters");
        assert!(next()["id"].is_number());
        let sample = crate::meter::sample(p.pid).expect("running");
        assert!(sample.rss_kb > 0);
        p.set_suspended(true).unwrap();
        p.set_suspended(false).unwrap();
        p.stop(Duration::from_secs(2));
        assert!(matches!(rx.recv_timeout(Duration::from_secs(5)), Ok(Incoming::Message(_)) | Ok(Incoming::Exited)));
        let _ = std::fs::remove_file(log);
    }
}
