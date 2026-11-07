// SPDX-License-Identifier: GPL-3.0-or-later
//! Debugging (v0.4 phase 4) without a display: which debug adapter serves a file
//! (`plan`), and a DAP session over stdio, or TCP to a debugger already listening (a
//! notebook's kernel), with the LSP framing (`Session`). Built on
//! the spike 7 client (gdb's own DAP, 10/10 runs). TileIDE installs nothing: a missing
//! adapter says what to install.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
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
    /// The adapter listens on a port it prints (`Session::serve`), not on stdio.
    pub listens: bool,
    /// The adapter's environment (the project's toolchains; the app sets it).
    pub env: Vec<(String, String)>,
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
    /// A Node.js that runs here (a mise shim may not, outside a project).
    pub node: Option<PathBuf>,
    /// js-debug's `dapDebugServer.js` (`js_debug`).
    pub js_debug: Option<PathBuf>,
}

/// js-debug's DAP server: the Arch package's (`vscode-js-debug-bin`), one behind a
/// `js-debug-dap` on PATH, or a release unpacked into `~/.local/share/js-debug`.
pub fn js_debug(home: Option<&Path>) -> Option<PathBuf> {
    let on_path = std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).find_map(|d| std::fs::canonicalize(d.join("js-debug-dap")).ok()));
    let mut candidates = vec![PathBuf::from("/usr/lib/js-debug/dapDebugServer.js")];
    candidates.extend(on_path);
    if let Some(home) = home {
        let share = home.join(".local/share/js-debug");
        candidates.extend([share.join("src/dapDebugServer.js"), share.join("js-debug/src/dapDebugServer.js"), share.join("dapDebugServer.js")]);
    }
    candidates.into_iter().find(|p| p.is_file())
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
        listens: false,
        env: Vec::new(),
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
                listens: false,
                env: Vec::new(),
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
            if let Some(build) = tileide_core::java::build_of(file, host.folder.as_deref()) {
                let program = build.program();
                if !program.contains('/') && !found(&program) {
                    let package = if program == "mvn" { "maven" } else { "gradle" };
                    return Err(format!("{program} is not installed (this is a {package} project). Install it with: sudo pacman -S {package}"));
                }
                let classpath_file = out_dir.join(format!("{stem}-classpath"));
                let class = package.map_or_else(|| stem.clone(), |p| format!("{p}.{stem}"));
                let classes: Vec<String> = build.classes().iter().map(|c| path(c)).collect();
                let roots: Vec<String> = build.source_roots().iter().map(|r| path(r)).collect();
                return Ok(Plan {
                    adapter: "jdb".into(),
                    cmd: vec![path(&host.bridge), "dap-jdb".into()],
                    build: vec![build.compile(&classpath_file, out_dir)?],
                    launch: json!({ "mainClass": class, "classPath": classes.join(":"), "classPathFile": path(&classpath_file), "sourceRoot": roots.join(":"), "cwd": path(&build.module) }),
                    cwd: build.module.clone(),
                    listens: false,
                    env: Vec::new(),
                });
            }
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
                listens: false,
                env: Vec::new(),
            })
        }
        // TypeScript runs as Node runs it (type stripping, Node ≥ 22.6), or through
        // source maps when a launch config points at compiled JavaScript.
        "js" | "mjs" | "cjs" | "ts" | "mts" | "cts" => {
            let Some(node) = &host.node else {
                return Err("Node.js is not installed. Install it with: sudo pacman -S nodejs".into());
            };
            let Some(server) = &host.js_debug else {
                return Err("js-debug is not installed. Install it with: yay -S vscode-js-debug-bin (AUR)".into());
            };
            Ok(Plan {
                adapter: "js-debug".into(),
                cmd: vec![path(node), path(server), "0".into(), "127.0.0.1".into()],
                build: Vec::new(),
                launch: json!({ "type": "pwa-node", "name": stem, "program": path(file), "cwd": path(&cwd), "runtimeExecutable": path(node), "console": "internalConsole", "outputCapture": "std", "skipFiles": ["<node_internals>/**"] }),
                cwd,
                listens: true,
                env: Vec::new(),
            })
        }
        "" => Err("This file has no extension, so TileIDE can't pick a debugger.".into()),
        other => Err(format!("No debugger for .{other} files yet (C, C++, Rust with gdb; Python with debugpy; Java with jdb; JavaScript and TypeScript with js-debug).")),
    }
}

/// `plan` for a launch config: its program (a source file as `plan` reads it, a Cargo
/// project folder, or an executable built elsewhere, debugged with gdb as it is) with
/// the config's arguments, environment and working folder in the `launch` request.
pub fn plan_launch(launch: &tileide_core::launch::Resolved, out_dir: &Path, host: &Host) -> Result<Plan, String> {
    let program = &launch.program;
    let mut plan = if program.is_dir() {
        let root = tileide_core::run::cargo_root(program, Some(program)).ok_or_else(|| format!("{}: {} is a folder without a Cargo.toml", launch.name, program.display()))?;
        plan(&root.join("src").join("main.rs"), out_dir, host)?
    } else if !program.exists() {
        return Err(format!("{}: {} doesn't exist", launch.name, program.display()));
    } else {
        match plan(program, out_dir, host) {
            Err(_) if tileide_core::launch::is_executable(program) && (host.found)("gdb") => {
                let cwd = program.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
                Plan {
                    adapter: "gdb".into(),
                    cmd: vec!["gdb".into(), "-q".into(), "-nx".into(), "-i".into(), "dap".into()],
                    build: Vec::new(),
                    launch: json!({ "program": program.to_string_lossy(), "cwd": cwd.to_string_lossy(), "stopAtBeginningOfMainSubprogram": false }),
                    cwd,
                    listens: false,
                    env: Vec::new(),
                }
            }
            other => other?,
        }
    };
    // gdb (≥ 15), debugpy, the jdb bridge and js-debug all read `args`, `env` (an object) and `cwd`.
    plan.launch["args"] = json!(launch.args);
    if !launch.env.is_empty() {
        plan.launch["env"] = Value::Object(launch.env.iter().map(|(k, v)| (k.clone(), json!(v))).collect());
    }
    if let Some(cwd) = &launch.cwd {
        plan.launch["cwd"] = json!(cwd.to_string_lossy());
    }
    Ok(plan)
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
    shared: Arc<Shared>,
}

enum End {
    Child(Arc<Mutex<Child>>),
    Socket(TcpStream),
}

type Input = Mutex<Box<dyn Write + Send>>;

/// What the connections' readers share.
struct Shared {
    /// Where requests go: the adapter's connection, then each child session it asked
    /// for (js-debug's `startDebugging`, one per Node process); the newest gets them.
    route: Mutex<Vec<Arc<Input>>>,
    seq: AtomicI64,
    deliver: Box<dyn Fn(Incoming) + Send + Sync>,
    /// A listening adapter's address, where child sessions connect.
    address: Option<String>,
}

fn write(stdin: &Input, message: &Value) -> Result<(), String> {
    let mut stdin = stdin.lock().map_err(|_| "adapter stdin poisoned".to_string())?;
    stdin.write_all(&encode(message)).and_then(|()| stdin.flush()).map_err(|e| format!("write failed: {e}"))
}

/// Sends a request on one connection; returns its `seq` (unique across connections).
fn send(shared: &Shared, to: &Input, command: &str, arguments: Value) -> Result<i64, String> {
    let seq = shared.seq.fetch_add(1, Ordering::SeqCst) + 1;
    write(to, &json!({ "seq": seq, "type": "request", "command": command, "arguments": arguments }))?;
    Ok(seq)
}

/// Starts `cmd` in `cwd` with `env` (the project's toolchains), ending with TileIDE
/// (and the debuggee with it).
fn spawn(cmd: &[String], cwd: &Path, env: &[(String, String)]) -> Result<Child, String> {
    let (program, args) = cmd.split_first().ok_or("empty adapter command")?;
    let mut command = Command::new(program);
    command.args(args).current_dir(cwd).envs(env.iter().map(|(k, v)| (k, v))).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    command.spawn().map_err(|e| format!("cannot start {program}: {e}"))
}

/// Reads messages from `output` on a thread. Only the first connection's end is
/// `Exited`. `then`: a child session's `initialize` seq and the request (`launch` or
/// `attach`) sent once it's answered; the child gets the client's requests from then on.
fn read(shared: Arc<Shared>, input: Arc<Input>, output: impl Read + Send + 'static, first: bool, mut then: Option<(i64, String, Value)>) {
    std::thread::spawn(move || {
        let mut output = BufReader::new(output);
        while let Ok(Some(message)) = read_message(&mut output) {
            if message["type"] == "request" {
                // Reverse requests other than `startDebugging` (`runInTerminal`) are refused.
                let started = message["command"] == "startDebugging" && start_child(&shared, &message["arguments"]).is_ok();
                let reason = if started { "" } else { "not supported by TileIDE" };
                let reply = json!({ "seq": 0, "type": "response", "request_seq": message["seq"], "command": message["command"], "success": started, "message": reason });
                let _ = write(&input, &reply);
                continue;
            }
            if let Some((_, command, arguments)) = then.take_if(|(seq, ..)| message["type"] == "response" && message["request_seq"] == *seq) {
                if let Ok(mut route) = shared.route.lock() {
                    route.push(input.clone());
                }
                let _ = send(&shared, &input, &command, arguments);
                continue;
            }
            (shared.deliver)(Incoming::Message(message));
        }
        if first {
            (shared.deliver)(Incoming::Exited);
        }
    });
}

/// A child session (js-debug): a new connection to the adapter, initialized, then
/// given the configuration the adapter asked for.
fn start_child(shared: &Arc<Shared>, arguments: &Value) -> Result<(), String> {
    let address = shared.address.as_deref().ok_or("not a listening adapter")?;
    let socket = TcpStream::connect(address).map_err(|e| e.to_string())?;
    let input: Arc<Input> = Arc::new(Mutex::new(Box::new(socket.try_clone().map_err(|e| e.to_string())?)));
    let command = arguments["request"].as_str().unwrap_or("launch").to_string();
    let seq = send(shared, &input, "initialize", initialize_args())?;
    read(shared.clone(), input, socket, false, Some((seq, command, arguments["configuration"].clone())));
    Ok(())
}

impl Session {
    /// Starts the adapter; messages arrive through `deliver` on a reader thread.
    pub fn start(cmd: &[String], cwd: &Path, env: &[(String, String)], deliver: impl Fn(Incoming) + Send + Sync + 'static) -> Result<Session, String> {
        let mut child = spawn(cmd, cwd, env)?;
        let stdin: Box<dyn Write + Send> = Box::new(child.stdin.take().ok_or("no stdin")?);
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let shared = Self::read_first(stdin, stdout, deliver, None);
        Ok(Session { end: End::Child(Arc::new(Mutex::new(child))), shared })
    }

    /// Starts an adapter that listens on a port it prints ("… listening at HOST:PORT",
    /// js-debug's `dapDebugServer.js 0 127.0.0.1`) and connects to it.
    pub fn serve(cmd: &[String], cwd: &Path, env: &[(String, String)], deliver: impl Fn(Incoming) + Send + Sync + 'static) -> Result<Session, String> {
        let mut child = spawn(cmd, cwd, env)?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        // The adapter writes on: its stdout stays read until it ends.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        });
        let address = loop {
            match rx.recv_timeout(std::time::Duration::from_secs(10)) {
                Ok(line) => match line.trim().rsplit(' ').next().filter(|a| a.parse::<std::net::SocketAddr>().is_ok()) {
                    Some(address) => break address.to_string(),
                    None => continue,
                },
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{} didn't say where it listens", cmd.join(" ")));
                }
            }
        };
        let socket = TcpStream::connect(&address).map_err(|e| format!("cannot reach the debugger at {address}: {e}"))?;
        let input = Box::new(socket.try_clone().map_err(|e| e.to_string())?);
        let shared = Self::read_first(input, socket, deliver, Some(address));
        Ok(Session { end: End::Child(Arc::new(Mutex::new(child))), shared })
    }

    /// Connects to an adapter listening on `address` (debugpy in a Jupyter kernel).
    /// `stop` then detaches and leaves the debuggee running.
    pub fn connect(address: &str, deliver: impl Fn(Incoming) + Send + Sync + 'static) -> Result<Session, String> {
        let socket = TcpStream::connect(address).map_err(|e| format!("cannot reach the debugger at {address}: {e}"))?;
        let clone = |s: &TcpStream| s.try_clone().map_err(|e| e.to_string());
        let shared = Self::read_first(Box::new(clone(&socket)?), clone(&socket)?, deliver, None);
        Ok(Session { end: End::Socket(socket), shared })
    }

    fn read_first(input: Box<dyn Write + Send>, output: impl Read + Send + 'static, deliver: impl Fn(Incoming) + Send + Sync + 'static, address: Option<String>) -> Arc<Shared> {
        let input = Arc::new(Mutex::new(input));
        let shared = Arc::new(Shared { route: Mutex::new(vec![input.clone()]), seq: AtomicI64::new(0), deliver: Box::new(deliver), address });
        read(shared.clone(), input, output, true, None);
        shared
    }

    /// Sends a request; returns its `seq` (the response arrives through `deliver`).
    pub fn request(&mut self, command: &str, arguments: Value) -> Result<i64, String> {
        let to = self.shared.route.lock().ok().and_then(|r| r.last().cloned()).ok_or("no connection")?;
        send(&self.shared, &to, command, arguments)
    }

    /// `disconnect` (ending the debuggee) on every connection, newest first, then a
    /// kill if the adapter lingers. Over a socket: detaches, and the debuggee runs on.
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
        let route = self.shared.route.lock().map(|r| r.clone()).unwrap_or_default();
        for to in route.iter().rev() {
            let _ = send(&self.shared, to, "disconnect", json!({ "terminateDebuggee": true }));
        }
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
        Host { found, pythons: vec![PathBuf::from("/w/.venv/bin/python"), PathBuf::from("python3")], python_has, bridge: PathBuf::from("/usr/bin/tileide"), folder: None, node: Some(PathBuf::from("/usr/bin/node")), js_debug: Some(PathBuf::from("/usr/lib/js-debug/dapDebugServer.js")) }
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
        assert!(plan(Path::new("/w/a.go"), Path::new("/o"), &host(&all, &any)).unwrap_err().contains(".go"));
        let js = plan(Path::new("/w/a.ts"), Path::new("/o"), &host(&all, &any)).unwrap();
        assert_eq!((js.adapter.as_str(), js.listens), ("js-debug", true));
        assert_eq!(js.cmd, ["/usr/bin/node", "/usr/lib/js-debug/dapDebugServer.js", "0", "127.0.0.1"]);
        assert_eq!((js.launch["program"].as_str(), js.launch["runtimeExecutable"].as_str()), (Some("/w/a.ts"), Some("/usr/bin/node")));
        let no_js_debug = Host { js_debug: None, ..host(&all, &any) };
        assert!(plan(Path::new("/w/a.js"), Path::new("/o"), &no_js_debug).unwrap_err().contains("vscode-js-debug"));
        let java = plan(Path::new("/w/Main.java"), Path::new("/o"), &host(&all, &any)).unwrap();
        assert_eq!(java.cmd, ["/usr/bin/tileide", "dap-jdb"]);
        assert_eq!((java.launch["mainClass"].as_str(), java.launch["classPath"].as_str()), (Some("Main"), Some("/o/Main-classes")));
        assert!(plan(Path::new("/w/Main.java"), Path::new("/o"), &host(&|p| p != "jdb", &any)).unwrap_err().contains("jdk-openjdk"));
    }

    #[test]
    fn a_launch_config_reaches_the_program_through_gdb() {
        let on = tileide_core::run::on_path;
        if !on("gdb") || !on("cc") {
            eprintln!("gdb or cc missing: skipped");
            return;
        }
        let dir = std::env::temp_dir().join(format!("tileide-dap-launch-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("run")).unwrap();
        let src = dir.join("t.c");
        // Writes its argument and $K into out.txt, in its working folder.
        std::fs::write(&src, "#include <stdio.h>\n#include <stdlib.h>\nint main(int c, char **v) {\n  FILE *f = fopen(\"out.txt\", \"w\");\n  fprintf(f, \"%s|%s\", c > 1 ? v[1] : \"\", getenv(\"K\"));\n  return fclose(f);\n}\n").unwrap();
        let launch = tileide_core::launch::Resolved { name: "t".into(), program: src.clone(), args: vec!["a b".into()], env: vec![("K".into(), "v".into())], cwd: Some(dir.join("run")) };
        let p = plan_launch(&launch, &dir, &Host { found: &on, pythons: Vec::new(), python_has: &|_, _| false, bridge: PathBuf::new(), folder: None, node: None, js_debug: None }).unwrap();
        assert!(Command::new(&p.build[0][0]).args(&p.build[0][1..]).status().unwrap().success());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut s = Session::start(&p.cmd, &p.cwd, &[], move |m| {
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
        s.request("launch", p.launch.clone()).unwrap();
        next(&|m| m["event"] == "initialized");
        s.request("configurationDone", json!({})).unwrap();
        next(&|m| m["event"] == "exited" || m["event"] == "terminated");
        s.stop();
        assert_eq!(std::fs::read_to_string(dir.join("run/out.txt")).unwrap(), "a b|v");
        let _ = std::fs::remove_dir_all(dir);
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
        let p = plan(&src, &dir, &Host { found: &on, pythons: Vec::new(), python_has: &|_, _| false, bridge: PathBuf::new(), folder: None, node: None, js_debug: None }).unwrap();
        let status = Command::new(&p.build[0][0]).args(&p.build[0][1..]).status().unwrap();
        assert!(status.success());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut s = Session::start(&p.cmd, &p.cwd, &[], move |m| {
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

    /// js-debug end to end on a TypeScript file (Node strips the types): the child
    /// session it starts gets the breakpoint and stops. Skipped without node or
    /// js-debug (`TILEIDE_TEST_JS_DEBUG` may point at an unpacked `dapDebugServer.js`).
    #[test]
    fn js_debug_stops_at_a_breakpoint_in_typescript() {
        let server = std::env::var_os("TILEIDE_TEST_JS_DEBUG").map(PathBuf::from).or_else(|| js_debug(std::env::var_os("HOME").map(PathBuf::from).as_deref()));
        let node_runs = Command::new("node").arg("--version").stdout(Stdio::null()).status().is_ok_and(|s| s.success());
        let Some(server) = server.filter(|_| node_runs) else {
            eprintln!("node or js-debug missing: skipped");
            return;
        };
        let dir = std::env::temp_dir().join(format!("tileide-dap-js-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("t.ts");
        std::fs::write(&src, "function add(a: number, b: number): number {\n  const sum: number = a + b;\n  return sum;\n}\nconsole.log(add(40, 2));\n").unwrap();
        let on = tileide_core::run::on_path;
        let host = Host { found: &on, pythons: Vec::new(), python_has: &|_, _| false, bridge: PathBuf::new(), folder: None, node: Some(PathBuf::from("node")), js_debug: Some(server) };
        let p = plan(&src, &dir, &host).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut s = Session::serve(&p.cmd, &p.cwd, &[], move |m| {
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
        s.request("launch", p.launch.clone()).unwrap();
        // The parent session, then the child session for the Node process: each is
        // configured as the app does it.
        for _ in 0..2 {
            next(&|m| m["event"] == "initialized");
            let bp = s.request("setBreakpoints", json!({ "source": { "path": src }, "breakpoints": [{ "line": 3 }] })).unwrap();
            next(&|m| m["request_seq"] == bp);
            s.request("configurationDone", json!({})).unwrap();
        }
        let stopped = next(&|m| m["event"] == "stopped");
        let stack = s.request("stackTrace", json!({ "threadId": stopped["body"]["threadId"] })).unwrap();
        let stack = next(&|m| m["request_seq"] == stack);
        assert_eq!(stack["body"]["stackFrames"][0]["line"], 3);
        assert!(stack["body"]["stackFrames"][0]["source"]["path"].as_str().is_some_and(|p| p.ends_with("t.ts")));
        let frame = stack["body"]["stackFrames"][0]["id"].clone();
        let scopes = s.request("scopes", json!({ "frameId": frame })).unwrap();
        let scopes = next(&|m| m["request_seq"] == scopes);
        let locals = s.request("variables", json!({ "variablesReference": scopes["body"]["scopes"][0]["variablesReference"] })).unwrap();
        let locals = next(&|m| m["request_seq"] == locals);
        assert!(locals["body"]["variables"].as_array().unwrap().iter().any(|v| v["name"] == "sum" && v["value"] == "42"));
        let cont = s.request("continue", json!({ "threadId": stopped["body"]["threadId"] })).unwrap();
        next(&|m| m["request_seq"] == cont);
        next(&|m| m["event"] == "output" && m["body"]["output"].as_str().is_some_and(|o| o.contains("42")));
        s.stop();
        let _ = std::fs::remove_dir_all(dir);
    }
}
