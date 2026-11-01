// SPDX-License-Identifier: GPL-3.0-or-later
//! Which Python runs, debugs and hosts notebook kernels for a file (v0.4 phase 5): an
//! active `$VIRTUAL_ENV`, else the nearest virtualenv at or above the file's folder, else
//! `python3` on PATH, else the distro's `/usr/bin/python3` (where `pacman` installs
//! modules, which differs from PATH's when mise/pyenv/asdf manage Python).

use std::path::{Path, PathBuf};

/// Folder names checked first at each level; any other child with a `pyvenv.cfg` counts too.
const NAMES: &[&str] = &[".venv", "venv", "env"];

fn venv_python(dir: &Path) -> Option<PathBuf> {
    let python = dir.join("bin/python");
    (dir.join("pyvenv.cfg").is_file() || NAMES.iter().any(|n| dir.ends_with(n))).then_some(python).filter(|p| p.is_file())
}

/// The virtualenvs directly inside `dir`, the usual names first, then others by name.
fn venvs_in(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = NAMES.iter().filter_map(|n| venv_python(&dir.join(n))).collect();
    let mut others: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| !NAMES.iter().any(|n| e.file_name() == *n))
        .filter_map(|e| e.path().join("pyvenv.cfg").is_file().then(|| venv_python(&e.path())).flatten())
        .collect();
    others.sort();
    found.extend(others);
    found
}

/// Virtualenv Pythons for a file in `start`, nearest first: `$VIRTUAL_ENV`, then each
/// folder from `start` up to `stop` (the workspace folder). Without a `stop` (or with
/// `start` outside it) the walk ends at the home folder, or at `/` outside home.
pub fn venvs(start: &Path, stop: Option<&Path>, virtual_env: Option<&Path>, home: Option<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = virtual_env.and_then(venv_python_any).into_iter().collect();
    let stop = stop.filter(|s| start.starts_with(s)).or(home.filter(|h| start.starts_with(h)));
    let mut dir = Some(start);
    while let Some(d) = dir {
        for p in venvs_in(d) {
            if !out.contains(&p) {
                out.push(p);
            }
        }
        if stop.is_some_and(|s| s == d) {
            break;
        }
        dir = d.parent();
    }
    out
}

/// `$VIRTUAL_ENV` needn't carry a usual name.
fn venv_python_any(dir: &Path) -> Option<PathBuf> {
    Some(dir.join("bin/python")).filter(|p| p.is_file())
}

/// Every Python to try for a file in `start`, best first: the virtualenvs (`venvs`),
/// `python3`, then `/usr/bin/python3` when PATH's `python3` is another one.
pub fn candidates(start: &Path, stop: Option<&Path>) -> Vec<PathBuf> {
    let virtual_env = std::env::var_os("VIRTUAL_ENV").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut out = venvs(start, stop, virtual_env.as_deref(), home.as_deref());
    out.push(PathBuf::from("python3"));
    let system = Path::new("/usr/bin/python3");
    if system.is_file() && which("python3").is_some_and(|p| p.canonicalize().ok() != system.canonicalize().ok()) {
        out.push(system.to_path_buf());
    }
    out
}

/// The full path of `program` on PATH.
pub fn which(program: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).map(|d| d.join(program)).find(|p| p.is_file())
}

/// Whether `python` can import every module in `modules`.
pub fn imports(python: &Path, modules: &str) -> bool {
    std::process::Command::new(python)
        .args(["-c", &format!("import {modules}")])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The module a Python traceback in `output` says is missing ("No module named 'x'").
pub fn missing_module(output: &str) -> Option<String> {
    let rest = &output[output.find("No module named '")? + "No module named '".len()..];
    let name = &rest[..rest.find('\'')?];
    // `a.b` missing: the package to install is `a`.
    Some(name.split('.').next().unwrap_or(name).to_string()).filter(|n| !n.is_empty())
}

/// What to do about a missing `module` for `python`: pip in a virtualenv or a
/// mise/pyenv Python, pacman for the distro's (`/usr`).
pub fn install_hint(module: &str, python: &Path) -> String {
    let resolved = if python.is_absolute() { Some(python.to_path_buf()) } else { which(&python.to_string_lossy()) };
    match resolved.filter(|p| !p.starts_with("/usr")) {
        Some(p) => {
            let env = p.parent().and_then(Path::parent).filter(|e| e.join("pyvenv.cfg").is_file());
            let place = env.map_or_else(|| format!("for {}", p.display()), |e| format!("in {}", e.display()));
            format!("Python can't find the module '{module}'. Install it {place}: {} -m pip install {module}", p.display())
        }
        None => format!("Python can't find the module '{module}'. Install it with: sudo pacman -S python-{module} (or pip install {module} in a .venv)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn venv(dir: &Path, cfg: bool) {
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/python"), "").unwrap();
        if cfg {
            std::fs::write(dir.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
        }
    }

    #[test]
    fn finds_the_nearest_virtualenv_walking_up() {
        let root = std::env::temp_dir().join(format!("tileide-py-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let sub = root.join("project/src/pkg");
        std::fs::create_dir_all(&sub).unwrap();
        venv(&root.join(".venv"), false);
        venv(&root.join("project/myenv"), true);
        // A folder with bin/python but no pyvenv.cfg and an unusual name is not a venv.
        venv(&root.join("project/tools"), false);
        let found = venvs(&sub, Some(&root), None, None);
        assert_eq!(found, vec![root.join("project/myenv/bin/python"), root.join(".venv/bin/python")]);
        // The walk stops at the workspace folder.
        assert_eq!(venvs(&sub, Some(&root.join("project")), None, None), vec![root.join("project/myenv/bin/python")]);
        // $VIRTUAL_ENV comes first.
        venv(&root.join("active"), true);
        assert_eq!(venvs(&sub, Some(&root), Some(&root.join("active")), None)[0], root.join("active/bin/python"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reads_missing_modules_and_hints() {
        assert_eq!(missing_module("Traceback…\nModuleNotFoundError: No module named 'numpy'\n").as_deref(), Some("numpy"));
        assert_eq!(missing_module("No module named 'scipy.stats'").as_deref(), Some("scipy"));
        assert_eq!(missing_module("all fine"), None);
        assert!(install_hint("numpy", Path::new("/w/.venv/bin/python")).contains("/w/.venv/bin/python -m pip install numpy"));
        assert!(install_hint("numpy", Path::new("/usr/bin/python3")).contains("pacman -S python-numpy"));
    }
}
