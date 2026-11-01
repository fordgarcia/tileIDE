// SPDX-License-Identifier: GPL-3.0-or-later
//! Debugging (v0.4 phase 4) without a display: which debug adapter serves a file
//! (`plan`), and a DAP session over stdio with the LSP framing (`Session`). Built on
//! the spike 7 client (gdb's own DAP, 10/10 runs). TileIDE installs nothing: a missing
//! adapter says what to install.

use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tileide_lsp::rpc::{encode, read_message};

/// How to debug one file.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Shown in the debug tile ("gdb", "debugpy").
    pub adapter: String,
    /// The adapter process.
    pub cmd: Vec<String>,
    /// Built before launching (compile with debug info), run in `cwd`.
    pub build: Vec<Vec<String>>,
    /// The `launch` request's arguments.
    pub launch: Value,
    pub cwd: PathBuf,
}

/// The debug adapter for `file`: gdb (≥ 14, `gdb -i dap`) for C, C++ and single-file
/// Rust; debugpy for Python. `found(program)` is whether it is on PATH, `python_has`
/// whether `python3` can import a module.
pub fn plan(file: &Path, out_dir: &Path, found: impl Fn(&str) -> bool, python_has: impl Fn(&str) -> bool) -> Result<Plan, String> {
    let ext = file.extension().map_or_else(String::new, |e| e.to_string_lossy().to_lowercase());
    let cwd = file.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let stem = file.file_stem().map_or_else(|| "a".into(), |s| s.to_string_lossy().into_owned());
    let out = out_dir.join(format!("{stem}-debug"));
    let path = |p: &Path| p.to_string_lossy().into_owned();
    let native = |compiler: &str, args: &[&str]| -> Result<Plan, String> {
        if !found("gdb") {
            return Err("gdb is not installed. Install it with: sudo pacman -S gdb".into());
        }
        if !found(compiler) {
            return Err(format!("{compiler} is not installed. Install it with: sudo pacman -S {}", tileide_core::run::package_for(compiler)));
        }
        let mut build = vec![compiler.to_string()];
        build.extend(args.iter().map(|a| a.to_string()));
        build.extend([path(file), "-o".into(), path(&out)]);
        Ok(Plan {
            adapter: "gdb".into(),
            cmd: vec!["gdb".into(), "-q".into(), "-nx".into(), "-i".into(), "dap".into()],
            build: vec![build],
            launch: json!({ "program": path(&out), "cwd": path(&cwd), "stopAtBeginningOfMainSubprogram": false }),
            cwd: cwd.clone(),
        })
    };
    match ext.as_str() {
        "c" => native("cc", &["-g", "-O0", "-Wall"]),
        "cpp" | "cc" | "cxx" => native("c++", &["-g", "-O0", "-Wall"]),
        "rs" => native("rustc", &["-g", "-C", "opt-level=0", "--edition", "2024"]),
        "py" => {
            if !python_has("debugpy") {
                return Err("debugpy is not installed for python3. Install it with: sudo pacman -S python-debugpy (or pip install debugpy in your .venv)".into());
            }
            Ok(Plan {
                adapter: "debugpy".into(),
                cmd: vec!["python3".into(), "-m".into(), "debugpy.adapter".into()],
                build: Vec::new(),
                launch: json!({ "program": path(file), "cwd": path(&cwd), "console": "internalConsole", "justMyCode": true, "redirectOutput": true }),
                cwd,
            })
        }
        "" => Err("This file has no extension, so TileIDE can't pick a debugger.".into()),
        other => Err(format!("No debugger for .{other} files yet (C, C++, Rust with gdb; Python with debugpy).")),
    }
}

/// A message from the adapter.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// A response (`request_seq`, `success`, `body` or `message`) or an event.
    Message(Value),
    /// The adapter's stdout closed.
    Exited,
}

pub struct Session {
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<ChildStdin>>,
    seq: i64,
}

fn write(stdin: &Mutex<ChildStdin>, message: &Value) -> Result<(), String> {
    let mut stdin = stdin.lock().map_err(|_| "adapter stdin poisoned".to_string())?;
    stdin.write_all(&encode(message)).and_then(|()| stdin.flush()).map_err(|e| format!("write failed: {e}"))
}

impl Session {
    /// Starts the adapter; messages arrive through `deliver` on a reader thread.
    /// Reverse requests the client doesn't support (`runInTerminal`) are refused here.
    pub fn start(cmd: &[String], cwd: &Path, deliver: impl Fn(Incoming) + Send + 'static) -> Result<Session, String> {
        let (program, args) = cmd.split_first().ok_or("empty adapter command")?;
        let mut command = Command::new(program);
        command.args(args).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        // The adapter (and with it the debuggee) ends with TileIDE.
        unsafe {
            use std::os::unix::process::CommandExt;
            command.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        let mut child = command.spawn().map_err(|e| format!("cannot start {program}: {e}"))?;
        let stdin = Arc::new(Mutex::new(child.stdin.take().ok_or("no stdin")?));
        let mut stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        let replies = stdin.clone();
        std::thread::spawn(move || {
            while let Ok(Some(message)) = read_message(&mut stdout) {
                if message["type"] == "request" {
                    let refuse = json!({ "seq": 0, "type": "response", "request_seq": message["seq"], "command": message["command"], "success": false, "message": "not supported by TileIDE" });
                    let _ = write(&replies, &refuse);
                    continue;
                }
                deliver(Incoming::Message(message));
            }
            deliver(Incoming::Exited);
        });
        Ok(Session { child: Arc::new(Mutex::new(child)), stdin, seq: 0 })
    }

    /// Sends a request; returns its `seq` (the response arrives through `deliver`).
    pub fn request(&mut self, command: &str, arguments: Value) -> Result<i64, String> {
        self.seq += 1;
        write(&self.stdin, &json!({ "seq": self.seq, "type": "request", "command": command, "arguments": arguments }))?;
        Ok(self.seq)
    }

    /// `disconnect` (ending the debuggee), then a kill if the adapter lingers.
    pub fn stop(mut self) {
        let _ = self.request("disconnect", json!({ "terminateDebuggee": true }));
        let child = self.child.clone();
        std::thread::spawn(move || {
            for _ in 0..40 {
                if child.lock().map(|mut c| c.try_wait().ok().flatten().is_some()).unwrap_or(true) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            if let Ok(mut c) = child.lock() {
                let _ = c.kill();
                let _ = c.wait();
            }
        });
    }
}

/// `initialize` arguments TileIDE sends to every adapter.
pub fn initialize_args() -> Value {
    json!({ "clientID": "tileide", "clientName": "TileIDE", "adapterID": "tileide", "linesStartAt1": true, "columnsStartAt1": true, "pathFormat": "path", "supportsRunInTerminalRequest": false, "supportsVariableType": true })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn picks_adapters_and_explains_missing_ones() {
        let all = |_: &str| true;
        let p = plan(Path::new("/w/m.c"), Path::new("/o"), all, all).unwrap();
        assert_eq!(p.adapter, "gdb");
        assert_eq!(p.build[0][..4], ["cc", "-g", "-O0", "-Wall"]);
        assert_eq!(p.launch["program"], "/o/m-debug");
        assert_eq!(plan(Path::new("/w/a.py"), Path::new("/o"), all, all).unwrap().adapter, "debugpy");
        assert!(plan(Path::new("/w/a.py"), Path::new("/o"), all, |_| false).unwrap_err().contains("debugpy"));
        assert!(plan(Path::new("/w/m.c"), Path::new("/o"), |p| p != "gdb", all).unwrap_err().contains("pacman -S gdb"));
        assert!(plan(Path::new("/w/a.js"), Path::new("/o"), all, all).unwrap_err().contains(".js"));
    }

    /// gdb end to end on a small C file (the spike 7 flow); skipped without gdb or cc.
    #[test]
    fn gdb_stops_at_a_breakpoint() {
        let on = tileide_core::run::on_path;
        if !on("gdb") || !on("cc") {
            eprintln!("gdb or cc missing: skipped");
            return;
        }
        let dir = std::env::temp_dir().join(format!("tileide-dap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("t.c");
        std::fs::write(&src, "#include <stdio.h>\nint main(void) {\n  int a = 41;\n  a += 1;\n  printf(\"%d\\n\", a);\n  return 0;\n}\n").unwrap();
        let p = plan(&src, &dir, on, |_| false).unwrap();
        let status = Command::new(&p.build[0][0]).args(&p.build[0][1..]).status().unwrap();
        assert!(status.success());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut s = Session::start(&p.cmd, &p.cwd, move |m| {
            let _ = tx.send(m);
        })
        .unwrap();
        let next = |pred: &dyn Fn(&Value) -> bool| loop {
            match rx.recv_timeout(Duration::from_secs(20)).expect("adapter message") {
                Incoming::Message(m) if pred(&m) => return m,
                Incoming::Message(_) => {}
                Incoming::Exited => panic!("adapter exited"),
            }
        };
        let init = s.request("initialize", initialize_args()).unwrap();
        next(&|m| m["request_seq"] == init);
        let launch = s.request("launch", p.launch.clone()).unwrap();
        next(&|m| m["event"] == "initialized");
        let bp = s.request("setBreakpoints", json!({ "source": { "path": src }, "breakpoints": [{ "line": 4 }] })).unwrap();
        next(&|m| m["request_seq"] == bp);
        let done = s.request("configurationDone", json!({})).unwrap();
        next(&|m| m["request_seq"] == done);
        let _ = launch;
        let stopped = next(&|m| m["event"] == "stopped");
        let stack = s.request("stackTrace", json!({ "threadId": stopped["body"]["threadId"] })).unwrap();
        let stack = next(&|m| m["request_seq"] == stack);
        assert_eq!(stack["body"]["stackFrames"][0]["line"], 4);
        s.stop();
        let _ = std::fs::remove_dir_all(dir);
    }
}
