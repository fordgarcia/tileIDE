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
    /// Completion matches for request `id`, replacing chars `start..end` of its code.
    Completions { id: u64, matches: Vec<String>, start: usize, end: usize },
    /// The result of a silent evaluation `id` (`Kernel::eval`): its text, or the error.
    Eval { id: u64, result: Result<String, String> },
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
        "eval" => {
            let text = v["text"].as_str().unwrap_or("").to_string();
            Event::Eval { id: v["id"].as_u64()?, result: if v["ok"].as_bool() == Some(true) { Ok(text) } else { Err(text) } }
        }
        "complete" => Event::Completions {
            id: v["id"].as_u64()?,
            matches: v["matches"].as_array()?.iter().filter_map(|m| m.as_str().map(String::from)).collect(),
            start: v["start"].as_u64()? as usize,
            end: v["end"].as_u64()? as usize,
        },
        "fatal" => Event::Fatal(v["message"].as_str().unwrap_or("kernel failed").to_string()),
        _ => return None,
    })
}

/// Pythons to try for a notebook in `dir`, best first (`tileide_core::python::candidates`:
/// the nearest virtualenv up to the workspace `folder`, then `python3`, then the distro's).
pub fn pythons(dir: &Path, folder: Option<&Path>) -> Vec<PathBuf> {
    tileide_core::python::candidates(dir, folder)
}

/// Whether `python` can run the bridge (`jupyter_client` and `ipykernel` importable).
pub fn has_jupyter(python: &Path) -> bool {
    tileide_core::python::imports(python, "jupyter_client, ipykernel")
}

/// An installed Jupyter kernel (`jupyter kernelspec list`).
#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    pub name: String,
    pub display: String,
    pub language: String,
}

/// The kernels `python`'s Jupyter knows, by name.
pub fn kernelspecs(python: &Path) -> Vec<Spec> {
    const LIST: &str = "import json\nfrom jupyter_client.kernelspec import KernelSpecManager\nprint(json.dumps({n: [s['spec'].get('display_name', n), s['spec'].get('language', '')] for n, s in KernelSpecManager().get_all_specs().items()}))";
    let Ok(out) = Command::new(python).args(["-c", LIST]).stdin(Stdio::null()).stderr(Stdio::null()).output() else { return Vec::new() };
    parse_specs(&String::from_utf8_lossy(&out.stdout))
}

fn parse_specs(text: &str) -> Vec<Spec> {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text.trim()) else { return Vec::new() };
    map.into_iter()
        .map(|(name, v)| Spec { display: v[0].as_str().unwrap_or(&name).to_string(), language: v[1].as_str().unwrap_or("").to_string(), name })
        .collect()
}

pub const INSTALL_HINT: &str = "No Python here has Jupyter (checked the virtualenvs at and above this notebook's folder, python3, and /usr/bin/python3). Install it with: sudo pacman -Syu python-ipykernel python-jupyter-client (or in a .venv: pip install ipykernel)";

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

    /// Asks the kernel what can go at `cursor` (in characters) of `code`.
    pub fn complete(&mut self, id: u64, code: &str, cursor: usize) -> Result<(), String> {
        self.send(json!({ "op": "complete", "id": id, "code": code, "cursor": cursor }))
    }

    /// Evaluates the Python expression `expr` silently (no count, no output; the
    /// variables are the user's). `_tileide_variables(globals())` lists the variables.
    pub fn eval(&mut self, id: u64, expr: &str) -> Result<(), String> {
        self.send(json!({ "op": "eval", "id": id, "expr": expr }))
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

    #[test]
    fn parses_kernelspec_lists() {
        let specs = parse_specs(r#"{"ir": ["R", "R"], "python3": ["Python 3 (ipykernel)", "python"]}"#);
        assert_eq!(specs[0], Spec { name: "ir".into(), display: "R".into(), language: "R".into() });
        assert_eq!(specs.len(), 2);
        assert!(parse_specs("Traceback …").is_empty());
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
        k.complete(7, "import os\nos.pa", 15).unwrap();
        let events = wait(&|e| matches!(e, Event::Completions { .. }));
        assert!(matches!(events.last(), Some(Event::Completions { id: 7, matches, start: 13, end: 15 }) if matches.iter().any(|m| m == "path")), "{:?}", events.last());
        k.execute(2, "1/0").unwrap();
        let events = wait(&|e| *e == Event::Done { cell: 2 });
        assert!(events.iter().any(|e| matches!(e, Event::Output { output, .. } if output["ename"] == "ZeroDivisionError")));
        // Silent evaluation: the variables, with no new count and no names added.
        k.eval(9, "_tileide_variables(globals())").unwrap();
        let events = wait(&|e| matches!(e, Event::Eval { .. }));
        let Some(Event::Eval { id: 9, result: Ok(json) }) = events.last() else { panic!("{events:?}") };
        let vars: Value = serde_json::from_str(json).unwrap();
        assert_eq!(vars, json!([["x", "int", "", "42", false]]));
        assert!(!events.iter().any(|e| matches!(e, Event::Count { .. } | Event::Output { .. })), "{events:?}");
        k.eval(10, "undefined_name").unwrap();
        let events = wait(&|e| matches!(e, Event::Eval { .. }));
        assert!(matches!(events.last(), Some(Event::Eval { id: 10, result: Err(e) }) if e.contains("NameError")), "{:?}", events.last());
        k.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }
}
