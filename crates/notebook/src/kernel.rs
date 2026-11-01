// SPDX-License-Identifier: GPL-3.0-or-later
//! One running kernel, through `bridge.py` (jupyter_client in the user's Python): JSON
//! lines both ways, a reader thread handing events to `deliver`. TileIDE links no
//! ZeroMQ and installs nothing; a Python without Jupyter gives a clear error.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

use serde_json::{Value, json};

pub const BRIDGE: &str = include_str!("bridge.py");

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Ready { display: String },
    /// "busy" or "idle".
    Status(String),
    Count { cell: u64, count: i64 },
    Output { cell: u64, output: Value },
    Clear { cell: u64 },
    Done { cell: u64 },
    Restarted,
    /// The bridge or kernel failed; the kernel is gone.
    Fatal(String),
    /// The bridge exited.
    Exited,
}

fn parse(line: &str) -> Option<Event> {
    let v: Value = serde_json::from_str(line).ok()?;
    let cell = || v["id"].as_u64();
    Some(match v["ev"].as_str()? {
        "ready" => Event::Ready { display: v["display"].as_str().unwrap_or("kernel").to_string() },
        "status" => Event::Status(v["state"].as_str()?.to_string()),
        "count" => Event::Count { cell: cell()?, count: v["count"].as_i64()? },
        "output" => Event::Output { cell: cell()?, output: v["output"].clone() },
        "clear" => Event::Clear { cell: cell()? },
        "done" => Event::Done { cell: cell()? },
        "restarted" => Event::Restarted,
        "fatal" => Event::Fatal(v["message"].as_str().unwrap_or("kernel failed").to_string()),
        _ => return None,
    })
}

/// Pythons to try, best first: the workspace's virtualenv, then `python3` on PATH.
pub fn pythons(folder: Option<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = folder.into_iter().flat_map(|f| [".venv", "venv", "env"].map(|v| f.join(v).join("bin/python"))).filter(|p| p.is_file()).collect();
    out.push(PathBuf::from("python3"));
    out
}

/// Whether `python` can run the bridge (`jupyter_client` and `ipykernel` importable).
pub fn has_jupyter(python: &Path) -> bool {
    Command::new(python).args(["-c", "import jupyter_client, ipykernel"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

pub const INSTALL_HINT: &str = "Jupyter isn't installed for this Python. Install it with: sudo pacman -S python-ipykernel python-jupyter-client (or in the workspace's .venv: pip install ipykernel)";

pub struct Kernel {
    child: Child,
    stdin: ChildStdin,
}

impl Kernel {
    /// Starts `kernel` (a kernelspec name) with `python` in `cwd`; the bridge script is
    /// written to `script`. Events arrive through `deliver` on a reader thread.
    pub fn start(python: &Path, script: &Path, kernel: &str, cwd: &Path, deliver: impl Fn(Event) + Send + 'static) -> Result<Kernel, String> {
        if let Some(dir) = script.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(script, BRIDGE).map_err(|e| format!("cannot write {}: {e}", script.display()))?;
        let mut cmd = Command::new(python);
        cmd.arg(script).arg(kernel).arg(cwd).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        // The kernel ends with TileIDE, however TileIDE ends.
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        let mut child = cmd.spawn().map_err(|e| format!("cannot start {}: {e}", python.display()))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(event) = parse(&line) {
                    deliver(event);
                }
            }
            deliver(Event::Exited);
        });
        Ok(Kernel { child, stdin })
    }

    fn send(&mut self, message: Value) -> Result<(), String> {
        writeln!(self.stdin, "{message}").and_then(|()| self.stdin.flush()).map_err(|e| format!("kernel bridge: {e}"))
    }

    pub fn execute(&mut self, cell: u64, code: &str) -> Result<(), String> {
        self.send(json!({ "op": "execute", "id": cell, "code": code }))
    }

    pub fn interrupt(&mut self) -> Result<(), String> {
        self.send(json!({ "op": "interrupt" }))
    }

    pub fn restart(&mut self) -> Result<(), String> {
        self.send(json!({ "op": "restart" }))
    }

    /// Asks the bridge to shut the kernel down; kills it if it is still there after 3 s.
    pub fn shutdown(mut self) {
        let _ = self.send(json!({ "op": "shutdown" }));
        let mut child = self.child;
        std::thread::spawn(move || {
            for _ in 0..60 {
                if child.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let _ = child.kill();
            let _ = child.wait();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bridge_events() {
        assert_eq!(parse(r#"{"ev":"count","id":4,"count":7}"#), Some(Event::Count { cell: 4, count: 7 }));
        assert_eq!(parse(r#"{"ev":"status","state":"busy"}"#), Some(Event::Status("busy".into())));
        assert_eq!(parse(r#"{"ev":"nonsense"}"#), None);
        assert_eq!(parse("not json"), None);
    }

    /// End to end with a real kernel, when this machine has one: `TILEIDE_TEST_PYTHON`
    /// (a Python with ipykernel), else `python3` if it has Jupyter; skipped otherwise.
    #[test]
    fn runs_cells_on_a_real_kernel() {
        let python = std::env::var_os("TILEIDE_TEST_PYTHON").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("python3"));
        if !has_jupyter(&python) {
            eprintln!("no Jupyter for {}: skipped", python.display());
            return;
        }
        let dir = std::env::temp_dir().join(format!("tileide-kernel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut k = Kernel::start(&python, &dir.join("bridge.py"), "python3", &dir, move |e| {
            let _ = tx.send(e);
        })
        .unwrap();
        let wait = |want: &dyn Fn(&Event) -> bool| -> Vec<Event> {
            let mut seen = Vec::new();
            loop {
                let e = rx.recv_timeout(std::time::Duration::from_secs(60)).expect("kernel event");
                let stop = want(&e);
                assert!(!matches!(e, Event::Fatal(_)), "{e:?}");
                seen.push(e);
                if stop {
                    return seen;
                }
            }
        };
        wait(&|e| matches!(e, Event::Ready { .. }));
        k.execute(1, "x = 6 * 7\nprint('hello')\nx").unwrap();
        let events = wait(&|e| *e == Event::Done { cell: 1 });
        assert!(events.contains(&Event::Count { cell: 1, count: 1 }));
        assert!(events.iter().any(|e| matches!(e, Event::Output { output, .. } if output["text"] == "hello\n")));
        assert!(events.iter().any(|e| matches!(e, Event::Output { output, .. } if output["data"]["text/plain"] == "42")));
        k.execute(2, "1/0").unwrap();
        let events = wait(&|e| *e == Event::Done { cell: 2 });
        assert!(events.iter().any(|e| matches!(e, Event::Output { output, .. } if output["ename"] == "ZeroDivisionError")));
        k.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }
}
