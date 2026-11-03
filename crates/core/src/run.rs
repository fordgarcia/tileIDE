// SPDX-License-Identifier: GPL-3.0-or-later
//! What `Alt+R` runs (v0.4): the focused file by its extension, or the Cargo project
//! around it. Your own commands come first (`tileide.runner(".py", "python3 -u {file}")`).
//! Only plans here; the app runs the steps and shows their output. Never installs
//! anything: a missing compiler gives the distro package to install.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Shown in the card's title ("main.py", "cargo run").
    pub label: String,
    /// Run in order; a step that fails stops the rest (compile, then run).
    pub steps: Vec<Vec<String>>,
    pub cwd: PathBuf,
}

/// Built-in runners by extension: program to check, then the steps. Placeholders:
/// `{file}`, `{dir}`, `{stem}`, `{out}` (a binary in TileIDE's runtime folder).
type Runner = (&'static [&'static str], &'static str, &'static [&'static [&'static str]]);

const BUILT_IN: &[Runner] = &[
    (&["py"], "python3", &[&["python3", "-u", "{file}"]]),
    (&["js", "mjs", "cjs"], "node", &[&["node", "{file}"]]),
    (&["sh", "bash"], "bash", &[&["bash", "{file}"]]),
    (&["lua"], "lua", &[&["lua", "{file}"]]),
    (&["rb"], "ruby", &[&["ruby", "{file}"]]),
    (&["go"], "go", &[&["go", "run", "{file}"]]),
    (&["java"], "java", &[&["java", "{file}"]]),
    (&["c"], "cc", &[&["cc", "-Wall", "-g", "{file}", "-o", "{out}", "-lm"], &["{out}"]]),
    (&["cpp", "cc", "cxx"], "c++", &[&["c++", "-Wall", "-g", "{file}", "-o", "{out}"], &["{out}"]]),
    (&["rs"], "rustc", &[&["rustc", "--edition", "2024", "{file}", "-o", "{out}"], &["{out}"]]),
];

/// The Arch package that provides `program` (the distro is Arch-first).
pub fn package_for(program: &str) -> &str {
    match program {
        "python3" => "python",
        "node" => "nodejs",
        "cc" | "c++" => "gcc",
        "rustc" | "cargo" => "rustup",
        "java" => "jdk-openjdk",
        other => other,
    }
}

fn fill(part: &str, file: &Path, out: &Path) -> String {
    let dir = file.parent().unwrap_or(Path::new("."));
    let stem = file.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    part.replace("{file}", &file.to_string_lossy()).replace("{dir}", &dir.to_string_lossy()).replace("{stem}", &stem).replace("{out}", &out.to_string_lossy())
}

/// `s` quoted for `sh -c` (user commands go through the shell).
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The nearest folder at or above `from` (not above `stop`, if given) with a `Cargo.toml`.
pub fn cargo_root(from: &Path, stop: Option<&Path>) -> Option<PathBuf> {
    let mut dir = Some(from);
    while let Some(d) = dir {
        if d.join("Cargo.toml").is_file() {
            return Some(d.to_path_buf());
        }
        if stop.is_some_and(|s| s == d) {
            return None;
        }
        dir = d.parent();
    }
    None
}

/// Plans a run of `file` (the focused editor's) or else of the workspace `folder`.
/// `runners`: the config's commands by extension (".py") with placeholders, run by
/// `sh -c` (paths are quoted). `out_dir`: where compiled binaries go. `found`: whether
/// a program is on PATH.
pub fn plan(file: Option<&Path>, folder: Option<&Path>, runners: &BTreeMap<String, String>, out_dir: &Path, found: impl Fn(&str) -> bool) -> Result<Plan, String> {
    let cargo = |root: PathBuf| -> Result<Plan, String> {
        if !found("cargo") {
            return Err(format!("cargo is not installed. Install it with: sudo pacman -S {}", package_for("cargo")));
        }
        Ok(Plan { label: "cargo run".into(), steps: vec![vec!["cargo".into(), "run".into()]], cwd: root })
    };
    let Some(file) = file else {
        return match folder.and_then(|f| cargo_root(f, Some(f))) {
            Some(root) => cargo(root),
            None => Err("Nothing to run: focus a file (or open a Cargo project folder).".into()),
        };
    };
    let ext = file.extension().map_or_else(String::new, |e| e.to_string_lossy().to_lowercase());
    let name = file.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let dir = file.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let stem = file.file_stem().map_or_else(|| "a".into(), |s| s.to_string_lossy().into_owned());
    let out = out_dir.join(stem);

    if let Some(cmd) = runners.get(&format!(".{ext}")) {
        let cmd = cmd.replace("{file}", &quote(&file.to_string_lossy())).replace("{dir}", &quote(&dir.to_string_lossy())).replace("{out}", &quote(&out.to_string_lossy())).replace("{stem}", &quote(&file.file_stem().unwrap_or_default().to_string_lossy()));
        return Ok(Plan { label: name, steps: vec![vec!["sh".into(), "-c".into(), cmd]], cwd: dir });
    }
    if ext == "rs"
        && let Some(root) = cargo_root(&dir, folder)
    {
        return cargo(root);
    }
    let Some((_, program, steps)) = BUILT_IN.iter().find(|(exts, ..)| exts.contains(&ext.as_str())) else {
        return Err(match ext.as_str() {
            "" => format!("{name}: no extension, so TileIDE doesn't know how to run it. Add one with tileide.runner(\".ext\", \"command {{file}}\")."),
            _ => format!("No runner for .{ext} files. Add one in tileide.lua: tileide.runner(\".{ext}\", \"command {{file}}\")."),
        });
    };
    // A Python file runs with the Python chosen for it, else its virtualenv's.
    let venv = (*program == "python3")
        .then(|| crate::python::chosen(&dir).or_else(|| crate::python::venvs(&dir, folder, std::env::var_os("VIRTUAL_ENV").map(PathBuf::from).as_deref(), std::env::var_os("HOME").map(PathBuf::from).as_deref()).into_iter().next()))
        .flatten();
    if venv.is_none() && !found(program) {
        return Err(format!("{program} is not installed. Install it with: sudo pacman -S {}", package_for(program)));
    }
    let mut steps: Vec<Vec<String>> = steps.iter().map(|step| step.iter().map(|part| fill(part, file, &out)).collect()).collect();
    if let Some(python) = venv {
        steps[0][0] = python.to_string_lossy().into_owned();
    }
    Ok(Plan { label: name, steps, cwd: dir })
}

/// Whether `program` is an executable file on `PATH`.
pub fn on_path(program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)))
}

/// Terminal colour and cursor escapes removed (the card shows plain text).
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            // Carriage returns (progress bars redrawing a line) are dropped.
            if c != '\r' {
                out.push(c);
            }
            continue;
        }
        match chars.next() {
            // CSI: parameters, then one final byte in @..~.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: up to BEL or ESC \.
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' || (c == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(_: &str) -> bool {
        true
    }

    #[test]
    fn picks_a_runner_by_extension() {
        let out = Path::new("/run/t");
        let p = plan(Some(Path::new("/w/a b.py")), None, &BTreeMap::new(), out, all).unwrap();
        assert_eq!(p.steps, [["python3", "-u", "/w/a b.py"]]);
        assert_eq!((p.label.as_str(), p.cwd.as_path()), ("a b.py", Path::new("/w")));
        let c = plan(Some(Path::new("/w/m.c")), None, &BTreeMap::new(), out, all).unwrap();
        assert_eq!(c.steps.len(), 2, "compile, then run");
        assert_eq!(c.steps[1], ["/run/t/m"]);
    }

    #[test]
    fn missing_tools_and_unknown_files_explain_themselves() {
        let none = |_: &str| false;
        let e = plan(Some(Path::new("/w/a.js")), None, &BTreeMap::new(), Path::new("/o"), none).unwrap_err();
        assert!(e.contains("node is not installed") && e.contains("pacman -S nodejs"), "{e}");
        assert!(plan(Some(Path::new("/w/a.xyz")), None, &BTreeMap::new(), Path::new("/o"), all).unwrap_err().contains("tileide.runner(\".xyz\""));
        assert!(plan(None, None, &BTreeMap::new(), Path::new("/o"), all).unwrap_err().contains("focus a file"));
    }

    #[test]
    fn own_runners_win_and_quote_paths() {
        let mut runners = BTreeMap::new();
        runners.insert(".py".to_string(), "uv run {file}".to_string());
        let p = plan(Some(Path::new("/w/it's.py")), None, &runners, Path::new("/o"), all).unwrap();
        assert_eq!(p.steps, [["sh", "-c", "uv run '/w/it'\\''s.py'"]]);
    }

    #[test]
    fn rust_files_in_a_cargo_project_run_the_project() {
        let root = std::env::temp_dir().join(format!("tileide-run-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]").unwrap();
        let p = plan(Some(&root.join("src/main.rs")), Some(&root), &BTreeMap::new(), Path::new("/o"), all).unwrap();
        assert_eq!((p.steps[0].join(" "), p.cwd.clone()), ("cargo run".to_string(), root.clone()));
        assert_eq!(plan(None, Some(&root), &BTreeMap::new(), Path::new("/o"), all).unwrap().label, "cargo run");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn python_files_use_the_nearest_virtualenv() {
        let root = std::env::temp_dir().join(format!("tileide-run-venv-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".venv/bin")).unwrap();
        std::fs::create_dir_all(root.join("sub/deeper")).unwrap();
        std::fs::write(root.join(".venv/bin/python"), "").unwrap();
        let p = plan(Some(&root.join("sub/deeper/a.py")), Some(&root), &BTreeMap::new(), Path::new("/o"), |_| false).unwrap();
        assert_eq!(p.steps[0][0], root.join(".venv/bin/python").to_string_lossy());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn strips_colours_and_carriage_returns() {
        assert_eq!(strip_ansi("\u{1b}[1;32mok\u{1b}[0m\r\nnext\u{1b}]0;title\u{7}!"), "ok\nnext!");
        assert_eq!(strip_ansi("50%\r100%"), "50%100%");
    }
}
