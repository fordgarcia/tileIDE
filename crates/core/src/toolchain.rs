// SPDX-License-Identifier: GPL-3.0-or-later
//! A project's toolchains (v0.6 phase 5): the environment its runs, debug sessions
//! and language servers get. In order of precedence: a version pinned for the
//! workspace in TileIDE (`Alt+P` in a JavaScript, TypeScript, Java or Go file), then
//! what mise says for the folder (`mise env`), else the versions the project's
//! `.tool-versions` / `mise.toml` ask for, found among mise's and asdf's installs.
//! Python has its own choice (`python`). Nothing is installed or written to the project.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A tool that can be pinned: (name as mise and asdf call it, label, its program,
/// the variable that points at its install).
pub struct Tool {
    pub name: &'static str,
    pub label: &'static str,
    pub program: &'static str,
    pub home_var: Option<&'static str>,
}

pub const TOOLS: &[Tool] = &[
    Tool { name: "node", label: "Node.js", program: "node", home_var: None },
    Tool { name: "java", label: "Java", program: "java", home_var: Some("JAVA_HOME") },
    Tool { name: "go", label: "Go", program: "go", home_var: None },
];

/// The tool that runs files with extension `ext`.
pub fn tool_for(ext: &str) -> Option<&'static Tool> {
    let name = match ext.to_ascii_lowercase().as_str() {
        "js" | "mjs" | "cjs" | "ts" | "mts" | "cts" | "jsx" | "tsx" => "node",
        "java" => "java",
        "go" => "go",
        _ => return None,
    };
    TOOLS.iter().find(|t| t.name == name)
}

/// Variables to set on a child process; `PATH` (when present) is complete.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Env {
    pub vars: Vec<(String, String)>,
}

impl Env {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    fn set(&mut self, key: &str, value: String) {
        self.vars.retain(|(k, _)| k != key);
        self.vars.push((key.into(), value));
    }

    /// The PATH a child gets: this one's, else TileIDE's own.
    pub fn path(&self) -> Vec<PathBuf> {
        match self.get("PATH") {
            Some(p) => std::env::split_paths(p).collect(),
            None => std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default(),
        }
    }

    /// `dirs` first on PATH (each once).
    fn prepend(&mut self, dirs: &[PathBuf]) {
        let mut path: Vec<PathBuf> = dirs.to_vec();
        path.extend(self.path().into_iter().filter(|d| !dirs.contains(d)));
        if let Ok(joined) = std::env::join_paths(&path) {
            self.set("PATH", joined.to_string_lossy().into_owned());
        }
    }

    /// Where `program` resolves on this PATH (a path with a `/` is taken as it is).
    pub fn find(&self, program: &str) -> Option<PathBuf> {
        if program.contains('/') {
            return executable(Path::new(program)).then(|| PathBuf::from(program));
        }
        self.path().into_iter().map(|d| d.join(program)).find(|p| executable(p))
    }

    pub fn apply<'a>(&self, command: &'a mut Command) -> &'a mut Command {
        command.envs(self.vars.iter().map(|(k, v)| (k, v)))
    }
}

fn executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// The environment for things run in `dir`.
pub fn env(dir: &Path) -> Env {
    let home = home();
    let mut env = from_mise(dir).unwrap_or_else(|| from_files(dir, home.as_deref()));
    if let Some(file) = pins_file() {
        for (tool, install) in pinned_in(&file, dir) {
            apply_install(&mut env, &tool, &install);
        }
    }
    env
}

/// An install's `bin` folder: `<install>/bin`, or one level down where some mise
/// backends unpack (`gradle-9.8.0/bin`).
fn bin_dir(install: &Path) -> PathBuf {
    let direct = install.join("bin");
    if direct.is_dir() {
        return direct;
    }
    std::fs::read_dir(install).into_iter().flatten().flatten().map(|e| e.path().join("bin")).find(|b| b.is_dir()).unwrap_or(direct)
}

/// `install`'s `bin` first on PATH, and its home variable (JAVA_HOME).
fn apply_install(env: &mut Env, tool: &str, install: &Path) {
    env.prepend(&[bin_dir(install)]);
    if let Some(var) = TOOLS.iter().find(|t| t.name == tool).and_then(|t| t.home_var) {
        env.set(var, install.to_string_lossy().into_owned());
    }
}

/// `mise env` for `dir`: its PATH entries first, then its variables. `None` without
/// mise, or when mise refuses (an untrusted config with an `[env]`).
fn from_mise(dir: &Path) -> Option<Env> {
    let mise = Env::default().find("mise").or_else(|| home().map(|h| h.join(".local/bin/mise")).filter(|p| executable(p)))?;
    let run = |flag: &str| -> Option<String> {
        let out = Command::new(&mise).args(["env", flag, "-C"]).arg(dir).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let paths: Vec<PathBuf> = run("--paths")?.lines().filter(|l| !l.is_empty()).map(PathBuf::from).collect();
    let mut env = Env { vars: parse_dotenv(&run("--dotenv")?) };
    env.vars.retain(|(k, _)| k != "PATH");
    env.prepend(&paths);
    Some(env)
}

/// `KEY=value` lines; a value may be quoted ("…" with `\` escapes, or '…').
fn parse_dotenv(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| l.trim_start().strip_prefix("export ").unwrap_or(l).split_once('='))
        .map(|(k, v)| {
            let v = v.trim();
            let value = if let Some(inner) = v.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
                let mut out = String::new();
                let mut chars = inner.chars();
                while let Some(c) = chars.next() {
                    match (c, c == '\\') {
                        (_, true) => match chars.next() {
                            Some('n') => out.push('\n'),
                            Some(c) => out.push(c),
                            None => {}
                        },
                        (c, false) => out.push(c),
                    }
                }
                out
            } else if let Some(inner) = v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
                inner.to_string()
            } else {
                v.to_string()
            };
            (k.trim().to_string(), value)
        })
        .collect()
}

/// Without mise: the versions asked for by the nearest `.tool-versions` / `mise.toml`
/// (then the global ones), found among mise's and asdf's installs.
fn from_files(dir: &Path, home: Option<&Path>) -> Env {
    let mut env = Env::default();
    let mut dirs = Vec::new();
    for (tool, version) in requested(dir, home) {
        if let Some(install) = installed(&tool, &version, home) {
            dirs.push(bin_dir(&install));
            if let Some(var) = TOOLS.iter().find(|t| t.name == tool).and_then(|t| t.home_var) {
                env.set(var, install.to_string_lossy().into_owned());
            }
        }
    }
    env.prepend(&dirs);
    env
}

/// (tool, version) the project asks for, nearest file first for each tool.
pub fn requested(dir: &Path, home: Option<&Path>) -> Vec<(String, String)> {
    let mut files: Vec<PathBuf> = Vec::new();
    for d in dir.ancestors() {
        files.extend(["mise.toml", ".mise.toml", ".config/mise.toml", ".tool-versions"].iter().map(|f| d.join(f)));
    }
    if let Some(home) = home {
        files.push(home.join(".config/mise/config.toml"));
    }
    let mut out: Vec<(String, String)> = Vec::new();
    for file in files.iter().filter(|f| f.is_file()) {
        let text = std::fs::read_to_string(file).unwrap_or_default();
        let found = if file.file_name().is_some_and(|n| n == ".tool-versions") { parse_tool_versions(&text) } else { parse_mise_tools(&text) };
        for (tool, version) in found {
            // asdf's plugin names.
            let tool = match tool.as_str() {
                "nodejs" => "node".to_string(),
                "golang" => "go".to_string(),
                _ => tool,
            };
            if !out.iter().any(|(t, _)| *t == tool) {
                out.push((tool, version));
            }
        }
    }
    out
}

/// `.tool-versions`: `tool version [fallbacks…]` per line.
fn parse_tool_versions(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or(""))
        .filter_map(|l| {
            let mut words = l.split_whitespace();
            Some((words.next()?.to_string(), words.next()?.to_string()))
        })
        .collect()
}

/// mise.toml's `[tools]`: `node = "22"`, `node = ["22", "20"]` (first wins), or
/// `node = { version = "22" }`.
fn parse_mise_tools(text: &str) -> Vec<(String, String)> {
    let mut in_tools = false;
    let mut out = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_tools = line == "[tools]";
            continue;
        }
        let Some((key, value)) = line.split_once('=').filter(|_| in_tools) else { continue };
        let key = key.trim().trim_matches('"');
        let value = value.split_once("version").map_or(value, |(_, v)| v.trim_start().trim_start_matches('='));
        if let Some(version) = value.split('"').nth(1).filter(|v| !v.is_empty()) {
            out.push((key.to_string(), version.to_string()));
        }
    }
    out
}

/// Folders mise and asdf install `tool`'s versions in.
fn install_roots(tool: &str, home: Option<&Path>) -> Vec<PathBuf> {
    let mise = std::env::var_os("MISE_DATA_DIR").map(PathBuf::from).or_else(|| home.map(|h| h.join(".local/share/mise")));
    let asdf = std::env::var_os("ASDF_DATA_DIR").map(PathBuf::from).or_else(|| home.map(|h| h.join(".asdf")));
    let asdf_name = match tool {
        "node" => "nodejs",
        "go" => "golang",
        other => other,
    };
    [mise.map(|d| d.join("installs").join(tool)), asdf.map(|d| d.join("installs").join(asdf_name))].into_iter().flatten().collect()
}

/// The install of `tool` at `version`: that folder, else the newest whose name
/// starts with `version` and a `.` or `-` ("22" → "22.11.0").
fn installed(tool: &str, version: &str, home: Option<&Path>) -> Option<PathBuf> {
    for root in install_roots(tool, home) {
        let exact = root.join(version);
        if bin_dir(&exact).is_dir() {
            return Some(exact);
        }
        let mut matches: Vec<PathBuf> = std::fs::read_dir(&root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_str().is_some_and(|n| n.strip_prefix(version).is_some_and(|rest| rest.starts_with(['.', '-'])))) && bin_dir(p).is_dir())
            .collect();
        matches.sort_by(|a, b| version_order(a, b));
        if let Some(best) = matches.pop() {
            return Some(best);
        }
    }
    None
}

/// Folder names compared as versions: numbers by value ("9" < "10").
fn version_order(a: &Path, b: &Path) -> std::cmp::Ordering {
    let parts = |p: &Path| -> Vec<(u64, String)> {
        p.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()).split(['.', '-', '+']).map(|s| (s.parse().unwrap_or(0), s.to_string())).collect()
    };
    parts(a).cmp(&parts(b))
}

/// One install that can be pinned.
#[derive(Debug, Clone, PartialEq)]
pub struct Install {
    /// "mise · 22.11.0", "system · java-21-openjdk".
    pub label: String,
    /// Its root: `bin/<program>` is inside.
    pub dir: PathBuf,
}

/// Every install of `tool` found here, newest first per source: mise's and asdf's
/// (real folders, not their alias links), the JDKs in `/usr/lib/jvm`, and `/usr`.
pub fn installs(tool: &Tool, home: Option<&Path>) -> Vec<Install> {
    let mut out = Vec::new();
    for (root, source) in install_roots(tool.name, home).into_iter().zip(["mise", "asdf"]) {
        let mut found: Vec<PathBuf> = std::fs::read_dir(&root).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| !p.is_symlink() && executable(&bin_dir(p).join(tool.program))).collect();
        found.sort_by(|a, b| version_order(b, a));
        out.extend(found.into_iter().map(|dir| Install { label: format!("{source} · {}", dir.file_name().unwrap_or_default().to_string_lossy()), dir }));
    }
    if tool.name == "java" {
        let mut jvms: Vec<PathBuf> = std::fs::read_dir("/usr/lib/jvm").into_iter().flatten().flatten().map(|e| e.path()).filter(|p| !p.is_symlink() && executable(&p.join("bin/java"))).collect();
        jvms.sort_by(|a, b| version_order(b, a));
        out.extend(jvms.into_iter().map(|dir| Install { label: format!("system · {}", dir.file_name().unwrap_or_default().to_string_lossy()), dir }));
    } else if executable(&Path::new("/usr/bin").join(tool.program)) {
        out.push(Install { label: format!("system · /usr/bin/{}", tool.program), dir: PathBuf::from("/usr") });
    }
    out
}

// ---- pins: one `folder<TAB>tool<TAB>install` line each in
// `$XDG_STATE_HOME/tileide/toolchains`; a path takes its nearest pinned ancestor's.

pub fn pins_file() -> Option<PathBuf> {
    crate::python::choices_file().map(|f| f.with_file_name("toolchains"))
}

type Pin = (PathBuf, String, PathBuf);

fn parse_pins(text: &str) -> Vec<Pin> {
    text.lines()
        .filter_map(|l| {
            let mut parts = l.split('\t');
            Some((PathBuf::from(parts.next()?), parts.next()?.to_string(), PathBuf::from(parts.next()?)))
        })
        .collect()
}

fn key(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// (tool, install) pinned for `path`: the nearest pinned folder's, per tool.
fn pinned_in(file: &Path, path: &Path) -> Vec<(String, PathBuf)> {
    let pins = parse_pins(&std::fs::read_to_string(file).unwrap_or_default());
    let path = key(path);
    let mut out: Vec<(String, PathBuf, usize)> = Vec::new();
    for (folder, tool, install) in pins.into_iter().filter(|(f, ..)| path.starts_with(f)) {
        let depth = folder.components().count();
        match out.iter_mut().find(|(t, ..)| *t == tool) {
            Some(slot) if slot.2 < depth => *slot = (tool, install, depth),
            Some(_) => {}
            None => out.push((tool, install, depth)),
        }
    }
    out.into_iter().map(|(t, i, _)| (t, i)).collect()
}

/// The install of `tool` pinned for `path`, if any.
pub fn pinned(path: &Path, tool: &str) -> Option<PathBuf> {
    pinned_in(&pins_file()?, path).into_iter().find(|(t, _)| t == tool).map(|(_, i)| i)
}

/// Pins `install` of `tool` for everything in `folder` (`None`: unpinned).
pub fn pin(folder: &Path, tool: &str, install: Option<&Path>) -> Result<(), String> {
    pin_in(&pins_file().ok_or("no home folder")?, folder, tool, install)
}

fn pin_in(file: &Path, folder: &Path, tool: &str, install: Option<&Path>) -> Result<(), String> {
    let folder = key(folder);
    let mut pins = parse_pins(&std::fs::read_to_string(file).unwrap_or_default());
    pins.retain(|(f, t, _)| !(*f == folder && t == tool));
    if let Some(i) = install {
        pins.push((folder, tool.to_string(), i.to_path_buf()));
    }
    pins.sort();
    let text: String = pins.iter().map(|(f, t, i)| format!("{}\t{t}\t{}\n", f.display(), i.display())).collect();
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(file, text).map_err(|e| format!("{}: {e}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tileide-toolchain-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fake_install(dir: &Path, program: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let p = dir.join("bin").join(program);
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn reads_tool_files() {
        assert_eq!(parse_tool_versions("nodejs 20.1.0\n# c\njava temurin-21 system\n"), [("nodejs".into(), "20.1.0".into()), ("java".into(), "temurin-21".into())]);
        let toml = "[env]\nnode = \"x\"\n[tools]\nnode = \"22\"\n\"go\" = [\"1.23\", \"1.22\"]\njava = { version = \"temurin-21\" }\n[settings]\nx = \"y\"\n";
        assert_eq!(parse_mise_tools(toml), [("node".into(), "22".into()), ("go".into(), "1.23".into()), ("java".into(), "temurin-21".into())]);
        assert_eq!(parse_dotenv("A=1\nB=\"a \\\"b\\\"\"\nexport C='x y'\n"), [("A".into(), "1".into()), ("B".into(), "a \"b\"".into()), ("C".into(), "x y".into())]);
    }

    #[test]
    fn project_files_pick_installed_versions() {
        let home = temp("files");
        let mise = home.join(".local/share/mise/installs");
        for v in ["20.9.0", "22.2.0", "22.11.0"] {
            fake_install(&mise.join("node").join(v), "node");
        }
        fake_install(&mise.join("java/temurin-21.0.1"), "java");
        let project = home.join("p/sub");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(home.join("p/.tool-versions"), "nodejs 18\njava temurin-21\n").unwrap();
        std::fs::write(project.join("mise.toml"), "[tools]\nnode = \"22\"\n").unwrap();
        // The nearest file wins per tool; "22" is the newest 22.x.
        assert_eq!(requested(&project, Some(&home)), [("node".into(), "22".into()), ("java".into(), "temurin-21".into())]);
        let env = from_files(&project, Some(&home));
        assert_eq!(env.find("node"), Some(mise.join("node/22.11.0/bin/node")));
        assert_eq!(env.get("JAVA_HOME"), Some(mise.join("java/temurin-21.0.1").to_str().unwrap()));
        let node = TOOLS.iter().find(|t| t.name == "node").unwrap();
        let labels: Vec<String> = installs(node, Some(&home)).into_iter().map(|i| i.label).filter(|l| l.starts_with("mise")).collect();
        assert_eq!(labels, ["mise · 22.11.0", "mise · 22.2.0", "mise · 20.9.0"]);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn pins_apply_to_the_folder_and_below() {
        let dir = temp("pins");
        let file = dir.join("state/toolchains");
        let (outer, inner) = (dir.join("w"), dir.join("w/inner"));
        std::fs::create_dir_all(&inner).unwrap();
        pin_in(&file, &outer, "node", Some(Path::new("/n/20"))).unwrap();
        pin_in(&file, &inner, "node", Some(Path::new("/n/22"))).unwrap();
        pin_in(&file, &outer, "java", Some(Path::new("/j/21"))).unwrap();
        let mut got = pinned_in(&file, &inner.join("a.ts"));
        got.sort();
        assert_eq!(got, [("java".into(), PathBuf::from("/j/21")), ("node".into(), PathBuf::from("/n/22"))]);
        pin_in(&file, &inner, "node", None).unwrap();
        assert!(pinned_in(&file, &inner).contains(&("node".into(), PathBuf::from("/n/20"))));
        let mut env = Env::default();
        apply_install(&mut env, "java", Path::new("/j/21"));
        assert_eq!((env.path()[0].as_path(), env.get("JAVA_HOME")), (Path::new("/j/21/bin"), Some("/j/21")));
        let _ = std::fs::remove_dir_all(dir);
    }
}
