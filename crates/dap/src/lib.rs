// SPDX-License-Identifier: GPL-3.0-or-later
//! Debugging (v0.4 phase 4) without a display: which debug adapter serves a file
//! (`plan`), and a DAP session over stdio, or TCP to a debugger already listening (a
//! notebook's kernel), with the LSP framing (`Session`). Built on
//! the spike 7 client (gdb's own DAP, 10/10 runs). TileIDE installs nothing: a missing
//! adapter says what to install.

use std::io::{BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tileide_lsp::rpc::{encode, read_message};

pub mod jdb;

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

/// What `plan` may ask about this machine (closures, so tests can fake it).
pub struct Host<'a> {
    /// Whether a program is on PATH.
    pub found: &'a dyn Fn(&str) -> bool,
    /// Pythons to try, best first (`tileide_core::python::candidates`).
    pub pythons: Vec<PathBuf>,
    /// Whether a Python can import a module.
    pub python_has: &'a dyn Fn(&Path, &str) -> bool,
    /// This program: it serves Java as `tileide dap-jdb` (the bridge in `jdb`).
    pub bridge: PathBuf,
    /// The workspace folder (a Cargo project is looked for up to it).
    pub folder: Option<PathBuf>,
}

/// The debug adapter for `file`: gdb (≥ 14, `gdb -i dap`) for C, C++ and Rust (a
/// single file, or the Cargo project around it); debugpy for Python (in the nearest
/// virtualenv that has it); TileIDE's own jdb bridge for Java.
pub fn plan(file: &Path, out_dir: &Path, host: &Host) -> Result<Plan, String> {
    let found = host.found;
    let ext = file.extension().map_or_else(String::new, |e| e.to_string_lossy().to_lowercase());
    let cwd = file.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let stem = file.file_stem().map_or_else(|| "a".into(), |s| s.to_string_lossy().into_owned());
    let out = out_dir.join(format!("{stem}-debug"));
    let path = |p: &Path| p.to_string_lossy().into_owned();
    let gdb = |program: &Path, build: Vec<Vec<String>>, cwd: &Path| Plan {
        adapter: "gdb".into(),
        cmd: vec!["gdb".into(), "-q".into(), "-nx".into(), "-i".into(), "dap".into()],
        build,
        launch: json!({ "program": path(program), "cwd": path(cwd), "stopAtBeginningOfMainSubprogram": false }),
        cwd: cwd.to_path_buf(),
    };
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
        Ok(gdb(&out, vec![build], &cwd))
    };
    match ext.as_str() {
        "c" => native("cc", &["-g", "-O0", "-Wall"]),
        "cpp" | "cc" | "cxx" => native("c++", &["-g", "-O0", "-Wall"]),
        "rs" => match tileide_core::run::cargo_root(&cwd, host.folder.as_deref()) {
            Some(root) => {
                if !found("gdb") {
                    return Err("gdb is not installed. Install it with: sudo pacman -S gdb".into());
                }
                if !found("cargo") {
                    return Err("cargo is not installed. Install it with: sudo pacman -S rustup".into());
                }
                let (package, bin, program) = cargo_binary(&root, file)?;
                let build = vec!["cargo".into(), "build".into(), "-p".into(), package, "--bin".into(), bin];
                Ok(gdb(&program, vec![build], &root))
            }
            None => native("rustc", &["-g", "-C", "opt-level=0", "--edition", "2024"]),
        },
        "py" => {
            let Some(python) = host.pythons.iter().find(|p| (host.python_has)(p, "debugpy")) else {
                let first = host.pythons.first().map_or_else(|| "python3".into(), |p| p.display().to_string());
                return Err(format!("debugpy is not installed for {first}. Install it with: sudo pacman -S python-debugpy (or {first} -m pip install debugpy in a .venv)"));
            };
            Ok(Plan {
                adapter: "debugpy".into(),
                cmd: vec![path(python), "-m".into(), "debugpy.adapter".into()],
                build: Vec::new(),
                launch: json!({ "program": path(file), "cwd": path(&cwd), "console": "internalConsole", "justMyCode": true, "redirectOutput": true }),
                cwd,
            })
        }
        "java" => {
            for program in ["javac", "jdb"] {
                if !found(program) {
                    return Err(format!("{program} is not installed (it comes with the JDK). Install it with: sudo pacman -S jdk-openjdk"));
                }
            }
            let source = std::fs::read_to_string(file).unwrap_or_default();
            let package = jdb::package_of(&source);
            let root = jdb::source_root(&cwd, package.as_deref());
            let class = package.map_or_else(|| stem.clone(), |p| format!("{p}.{stem}"));
            let classes = out_dir.join(format!("{stem}-classes"));
            let build = vec!["javac".into(), "-g".into(), "-d".into(), path(&classes), "-sourcepath".into(), path(&root), path(file)];
            Ok(Plan {
                adapter: "jdb".into(),
                cmd: vec![path(&host.bridge), "dap-jdb".into()],
                build: vec![build],
                launch: json!({ "mainClass": class, "classPath": path(&classes), "sourceRoot": path(&root), "cwd": path(&cwd) }),
                cwd,
            })
        }
        "" => Err("This file has no extension, so TileIDE can't pick a debugger.".into()),
        other => Err(format!("No debugger for .{other} files yet (C, C++, Rust with gdb; Python with debugpy; Java with jdb).")),
    }
}

/// The Cargo package and binary that `file` belongs to under `root`, and where the
/// debug build puts it (`cargo metadata`).
fn cargo_binary(root: &Path, file: &Path) -> Result<(String, String, PathBuf), String> {
    let out = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo metadata failed:\n{}", String::from_utf8_lossy(&out.stderr)));
    }
    let meta: Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    pick_binary(&meta, file).ok_or_else(|| "This Cargo project has no binary to debug (only libraries).".into())
}

/// From `cargo metadata`: the package whose folder holds `file` (the deepest), and its
/// binary: the one built from `file`, else the one named like the package, else the first.
fn pick_binary(meta: &Value, file: &Path) -> Option<(String, String, PathBuf)> {
    let target_dir = PathBuf::from(meta["target_directory"].as_str()?);
    let packages = meta["packages"].as_array()?;
    let dir_of = |p: &Value| p["manifest_path"].as_str().and_then(|m| Path::new(m).parent().map(Path::to_path_buf));
    let package = packages.iter().filter(|p| dir_of(p).is_some_and(|d| file.starts_with(d))).max_by_key(|p| dir_of(p).map_or(0, |d| d.components().count()))?;
    let name = package["name"].as_str()?.to_string();
    let bins: Vec<&Value> = package["targets"].as_array()?.iter().filter(|t| t["kind"].as_array().is_some_and(|k| k.iter().any(|k| k == "bin"))).collect();
    let bin = bins
        .iter()
        .find(|t| t["src_path"].as_str().is_some_and(|s| Path::new(s) == file))
        .or_else(|| bins.iter().find(|t| t["name"] == name.as_str()))
        .or_else(|| bins.first())?;
    let bin = bin["name"].as_str()?.to_string();
    Some((name, bin.clone(), target_dir.join("debug").join(bin)))
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
    /// The adapter TileIDE started, or the socket to one it attached to.
    end: End,
    stdin: Arc<Mutex<Box<dyn Write + Send>>>,
    seq: i64,
}

enum End {
    Child(Arc<Mutex<Child>>),
    Socket(TcpStream),
}

type Input = Mutex<Box<dyn Write + Send>>;

fn write(stdin: &Input, message: &Value) -> Result<(), String> {
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
        let stdin: Box<dyn Write + Send> = Box::new(child.stdin.take().ok_or("no stdin")?);
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stdin = Self::read(stdin, stdout, deliver);
        Ok(Session { end: End::Child(Arc::new(Mutex::new(child))), stdin, seq: 0 })
    }

    /// Connects to an adapter listening on `address` (debugpy in a Jupyter kernel).
    /// `stop` then detaches and leaves the debuggee running.
    pub fn connect(address: &str, deliver: impl Fn(Incoming) + Send + 'static) -> Result<Session, String> {
        let socket = TcpStream::connect(address).map_err(|e| format!("cannot reach the debugger at {address}: {e}"))?;
        let clone = |s: &TcpStream| s.try_clone().map_err(|e| e.to_string());
        let stdin = Self::read(Box::new(clone(&socket)?), clone(&socket)?, deliver);
        Ok(Session { end: End::Socket(socket), stdin, seq: 0 })
    }

    /// Reads messages from `output` on a thread; gives back the input end.
    fn read(input: Box<dyn Write + Send>, output: impl Read + Send + 'static, deliver: impl Fn(Incoming) + Send + 'static) -> Arc<Input> {
        let stdin = Arc::new(Mutex::new(input));
        let mut stdout = BufReader::new(output);
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
        stdin
    }

    /// Sends a request; returns its `seq` (the response arrives through `deliver`).
    pub fn request(&mut self, command: &str, arguments: Value) -> Result<i64, String> {
        self.seq += 1;
        write(&self.stdin, &json!({ "seq": self.seq, "type": "request", "command": command, "arguments": arguments }))?;
        Ok(self.seq)
    }

    /// `disconnect` (ending the debuggee), then a kill if the adapter lingers. Over a
    /// socket: detaches, and the debuggee runs on.
    pub fn stop(mut self) {
        let child = match &self.end {
            End::Child(child) => child.clone(),
            End::Socket(socket) => {
                let socket = socket.try_clone();
                let _ = self.request("disconnect", json!({ "terminateDebuggee": false }));
                // After the adapter has read it.
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    if let Ok(s) = socket {
                        let _ = s.shutdown(Shutdown::Both);
                    }
                });
                return;
            }
        };
        let _ = self.request("disconnect", json!({ "terminateDebuggee": true }));
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

    fn host<'a>(found: &'a dyn Fn(&str) -> bool, python_has: &'a dyn Fn(&Path, &str) -> bool) -> Host<'a> {
        Host { found, pythons: vec![PathBuf::from("/w/.venv/bin/python"), PathBuf::from("python3")], python_has, bridge: PathBuf::from("/usr/bin/tileide"), folder: None }
    }

    #[test]
    fn picks_adapters_and_explains_missing_ones() {
        let all = |_: &str| true;
        let any = |_: &Path, _: &str| true;
        let p = plan(Path::new("/w/m.c"), Path::new("/o"), &host(&all, &any)).unwrap();
        assert_eq!(p.adapter, "gdb");
        assert_eq!(p.build[0][..4], ["cc", "-g", "-O0", "-Wall"]);
        assert_eq!(p.launch["program"], "/o/m-debug");
        let py = plan(Path::new("/w/a.py"), Path::new("/o"), &host(&all, &any)).unwrap();
        assert_eq!((py.adapter.as_str(), py.cmd[0].as_str()), ("debugpy", "/w/.venv/bin/python"));
        // The venv lacks debugpy: the next Python that has it.
        let system_only = |p: &Path, _: &str| p == Path::new("python3");
        assert_eq!(plan(Path::new("/w/a.py"), Path::new("/o"), &host(&all, &system_only)).unwrap().cmd[0], "python3");
        assert!(plan(Path::new("/w/a.py"), Path::new("/o"), &host(&all, &|_, _| false)).unwrap_err().contains("debugpy"));
        assert!(plan(Path::new("/w/m.c"), Path::new("/o"), &host(&|p| p != "gdb", &any)).unwrap_err().contains("pacman -S gdb"));
        assert!(plan(Path::new("/w/a.js"), Path::new("/o"), &host(&all, &any)).unwrap_err().contains(".js"));
        let java = plan(Path::new("/w/Main.java"), Path::new("/o"), &host(&all, &any)).unwrap();
        assert_eq!(java.cmd, ["/usr/bin/tileide", "dap-jdb"]);
        assert_eq!((java.launch["mainClass"].as_str(), java.launch["classPath"].as_str()), (Some("Main"), Some("/o/Main-classes")));
        assert!(plan(Path::new("/w/Main.java"), Path::new("/o"), &host(&|p| p != "jdb", &any)).unwrap_err().contains("jdk-openjdk"));
    }

    #[test]
    fn picks_the_cargo_binary_of_a_file() {
        let meta = json!({
            "target_directory": "/p/target",
            "packages": [
                { "name": "app", "manifest_path": "/p/Cargo.toml", "targets": [
                    { "name": "app", "kind": ["bin"], "src_path": "/p/src/main.rs" },
                    { "name": "tool", "kind": ["bin"], "src_path": "/p/src/bin/tool.rs" },
                    { "name": "app", "kind": ["lib"], "src_path": "/p/src/lib.rs" } ] },
                { "name": "inner", "manifest_path": "/p/inner/Cargo.toml", "targets": [
                    { "name": "inner", "kind": ["lib"], "src_path": "/p/inner/src/lib.rs" } ] }
            ]
        });
        assert_eq!(pick_binary(&meta, Path::new("/p/src/bin/tool.rs")), Some(("app".into(), "tool".into(), PathBuf::from("/p/target/debug/tool"))));
        assert_eq!(pick_binary(&meta, Path::new("/p/src/util.rs")).unwrap().1, "app");
        assert_eq!(pick_binary(&meta, Path::new("/p/inner/src/lib.rs")), None, "a library only");
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
        let p = plan(&src, &dir, &Host { found: &on, pythons: Vec::new(), python_has: &|_, _| false, bridge: PathBuf::new(), folder: None }).unwrap();
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
