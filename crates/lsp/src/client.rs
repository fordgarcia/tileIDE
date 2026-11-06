// SPDX-License-Identifier: GPL-3.0-or-later
//! One running server. A reader thread answers the server's own requests and hands
//! everything else to `deliver`, which the app routes to its main loop; writes go
//! straight to stdin. Nothing here blocks on the server.

use std::io::{BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::protocol::{self, answer_server_request};
use crate::rpc::{encode, read_message};

#[derive(Debug)]
pub enum Incoming {
    /// A response or a notification.
    Message(Value),
    /// The server's stdout closed: it exited or crashed.
    Exited,
}

pub struct Connection {
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<ChildStdin>>,
    /// What `workspace/configuration` is answered with.
    settings: Arc<Mutex<Value>>,
    next_id: i64,
}

fn write(stdin: &Mutex<ChildStdin>, message: &Value) -> Result<(), String> {
    let mut stdin = stdin.lock().map_err(|_| "server stdin poisoned".to_string())?;
    stdin.write_all(&encode(message)).and_then(|()| stdin.flush()).map_err(|e| format!("write failed: {e}"))
}

impl Connection {
    /// Starts `cmd` (program and arguments) in `dir`. `settings` answers the server's
    /// `workspace/configuration` requests.
    pub fn spawn(cmd: &[String], dir: &std::path::Path, settings: Value, deliver: impl Fn(Incoming) + Send + 'static) -> Result<Connection, String> {
        let (program, args) = cmd.split_first().ok_or("empty command")?;
        let mut command = Command::new(program);
        // A server found by full path runs with its folder first on PATH: an npm-installed
        // one (`#!/usr/bin/env node`) then gets the node installed beside it, even when a
        // version manager's shim has no default outside a project.
        if let Some(dir) = std::path::Path::new(program).parent().filter(|d| d.is_absolute()) {
            let rest = std::env::var_os("PATH").unwrap_or_default();
            if let Ok(path) = std::env::join_paths(std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(&rest))) {
                command.env("PATH", path);
            }
        }
        let mut child = command
            .args(args)
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start {program}: {e}"))?;
        let stdin = Arc::new(Mutex::new(child.stdin.take().ok_or("no stdin")?));
        let mut stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        let replies = stdin.clone();
        let settings = Arc::new(Mutex::new(settings));
        let current = settings.clone();
        std::thread::spawn(move || {
            while let Ok(Some(message)) = read_message(&mut stdout) {
                let answer = current.lock().ok().and_then(|s| answer_server_request(&message, &s));
                match answer {
                    Some(answer) => {
                        let _ = write(&replies, &answer);
                        // Acknowledged here; the app applies the edit, refreshes hints, notes capabilities.
                        if ["workspace/applyEdit", "workspace/inlayHint/refresh", "workspace/diagnostic/refresh", "client/registerCapability", "client/unregisterCapability"].contains(&message["method"].as_str().unwrap_or("")) {
                            deliver(Incoming::Message(message));
                        }
                    }
                    None => deliver(Incoming::Message(message)),
                }
            }
            deliver(Incoming::Exited);
        });
        Ok(Connection { child: Arc::new(Mutex::new(child)), stdin, settings, next_id: 0 })
    }

    /// Sends a request; returns its id (the reply arrives through `deliver`).
    pub fn request(&mut self, method: &str, params: Value) -> Result<i64, String> {
        self.next_id += 1;
        write(&self.stdin, &protocol::request(self.next_id, method, params))?;
        Ok(self.next_id)
    }

    pub fn send(&self, message: &Value) -> Result<(), String> {
        write(&self.stdin, message)
    }

    /// New answers for `workspace/configuration`; the server is told to ask again.
    pub fn set_settings(&self, settings: Value) -> Result<(), String> {
        *self.settings.lock().map_err(|_| "settings poisoned".to_string())? = settings.clone();
        self.send(&protocol::notification("workspace/didChangeConfiguration", serde_json::json!({ "settings": settings })))
    }

    /// `shutdown` + `exit`, then a kill if it hasn't gone within `grace`. Runs on its
    /// own thread, so closing a workspace or quitting never waits on a server.
    pub fn shutdown(mut self, grace: Duration) {
        let _ = self.request("shutdown", Value::Null);
        let _ = self.send(&protocol::notification("exit", Value::Null));
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
