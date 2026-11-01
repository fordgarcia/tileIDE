// SPDX-License-Identifier: GPL-3.0-or-later
//! What TileIDE can run, debug and execute notebooks with on this machine (v0.4): each
//! toolchain's version and path, or the command that installs it. Detection only;
//! nothing is ever installed.

use std::process::{Command, Stdio};

/// (name, what TileIDE uses it for, how to check, Arch package).
enum Check {
    /// A program and the argument that prints its version.
    Program(&'static str, &'static str),
    /// Python modules `python3` must import.
    Modules(&'static str),
}

const TOOLCHAINS: &[(&str, &str, Check, &str)] = &[
    ("Python", "Alt+R, notebooks", Check::Program("python3", "--version"), "python"),
    ("Jupyter", "Notebook kernels", Check::Modules("jupyter_client, ipykernel"), "python-ipykernel python-jupyter-client"),
    ("debugpy", "Python debugging (F5)", Check::Modules("debugpy"), "python-debugpy"),
    ("GCC", "C and C++: Alt+R, F5", Check::Program("gcc", "--version"), "gcc"),
    ("Clang", "C and C++ (clangd for LSP)", Check::Program("clang", "--version"), "clang"),
    ("GDB", "Debugging C, C++, Rust (F5)", Check::Program("gdb", "--version"), "gdb"),
    ("Rust", "rustc and cargo", Check::Program("rustc", "--version"), "rustup"),
    ("Node.js", "JavaScript: Alt+R", Check::Program("node", "--version"), "nodejs"),
    ("Java", "Java: Alt+R (single file)", Check::Program("java", "-version"), "jdk-openjdk"),
    ("Go", "Go: Alt+R", Check::Program("go", "version"), "go"),
    ("Lua", "Lua: Alt+R", Check::Program("lua", "-v"), "lua"),
    ("Ruby", "Ruby: Alt+R", Check::Program("ruby", "--version"), "ruby"),
];

#[derive(Debug, Clone)]
pub struct Found {
    pub name: &'static str,
    pub used_for: &'static str,
    /// Version line and where, or `None` when missing.
    pub detail: Option<String>,
    pub install: String,
}

fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string()
}

fn which(program: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join(program)).find(|p| p.is_file()))
}

/// Checks every toolchain (spawns each once; call off the main thread).
pub fn detect() -> Vec<Found> {
    TOOLCHAINS
        .iter()
        .map(|(name, used_for, check, package)| {
            let detail = match check {
                Check::Program(program, flag) => which(program).and_then(|path| {
                    let out = Command::new(&path).arg(flag).stdin(Stdio::null()).output().ok()?;
                    // Some print their version on stderr (java -version).
                    let line = Some(first_line(&out.stdout)).filter(|l| !l.is_empty()).unwrap_or_else(|| first_line(&out.stderr));
                    Some(format!("{line} · {}", path.display()))
                }),
                Check::Modules(modules) => {
                    let ok = Command::new("python3").args(["-c", &format!("import {modules}")]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
                    ok.then(|| format!("importable by {}", which("python3").map_or_else(|| "python3".into(), |p| p.display().to_string())))
                }
            };
            Found { name, used_for, detail, install: format!("sudo pacman -S {package}") }
        })
        .collect()
}
