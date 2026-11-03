// SPDX-License-Identifier: GPL-3.0-or-later
//! Which Python runs, debugs and hosts notebook kernels for a file (v0.4 phase 5): an
//! active `$VIRTUAL_ENV`, else the nearest virtualenv at or above the file's folder, else
//! `python3` on PATH, else the distro's `/usr/bin/python3` (where `pacman` installs
//! modules, which differs from PATH's when mise/pyenv/asdf manage Python). A Python
//! chosen for a folder (v0.5 phase 7, `choose`) comes before all of these.

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

/// The project folder of `file` inside the workspace folder `stop`: the folder holding
/// the virtualenv nearest to the file (`lab/` for `lab/.venv`). Language servers start
/// there, so a project below the workspace folder gets its own packages and tools.
pub fn project_root(file: &Path, stop: &Path) -> Option<PathBuf> {
    let dir = file.parent()?;
    let stop = stop.canonicalize().unwrap_or_else(|_| stop.to_path_buf());
    if !dir.starts_with(&stop) {
        return None;
    }
    // …/<project>/.venv/bin/python
    venvs(dir, Some(&stop), None, None).first()?.ancestors().nth(3).map(Path::to_path_buf)
}

/// `$VIRTUAL_ENV` needn't carry a usual name.
fn venv_python_any(dir: &Path) -> Option<PathBuf> {
    Some(dir.join("bin/python")).filter(|p| p.is_file())
}

/// Every Python to try for a file in `start`, best first: the one chosen for it
/// (`chosen`), the virtualenvs (`venvs`), `python3`, then `/usr/bin/python3` when PATH's
/// `python3` is another one.
pub fn candidates(start: &Path, stop: Option<&Path>) -> Vec<PathBuf> {
    let virtual_env = std::env::var_os("VIRTUAL_ENV").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut out: Vec<PathBuf> = chosen(start).into_iter().collect();
    for p in venvs(start, stop, virtual_env.as_deref(), home.as_deref()) {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out.push(PathBuf::from("python3"));
    let system = Path::new("/usr/bin/python3");
    if system.is_file() && which("python3").is_some_and(|p| p.canonicalize().ok() != system.canonicalize().ok()) {
        out.push(system.to_path_buf());
    }
    out
}

/// The folder a Python choice is made for, for `file` in the workspace folder `folder`:
/// its project (`project_root`), else the workspace folder, else the file's own folder.
pub fn project(file: &Path, folder: Option<&Path>) -> PathBuf {
    let dir = file.parent().unwrap_or(file);
    folder
        .and_then(|f| project_root(file, f).or_else(|| dir.starts_with(f).then(|| f.to_path_buf())))
        .unwrap_or_else(|| dir.to_path_buf())
}

// ---- interpreter choices (v0.5 phase 7) ----
// A folder's chosen Python serves everything in it (Run, debugging, notebook kernels,
// the language server): one `folder<TAB>python` line each in
// `$XDG_STATE_HOME/tileide/interpreters`. A path takes its nearest chosen ancestor's.

pub fn choices_file() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(state.join("tileide/interpreters"))
}

fn parse_choices(text: &str) -> Vec<(PathBuf, PathBuf)> {
    text.lines().filter_map(|l| l.split_once('\t')).map(|(k, v)| (PathBuf::from(k), PathBuf::from(v))).collect()
}

/// Folders are compared as written and canonicalized (an editor may hold either).
fn key(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn chosen_in(file: &Path, path: &Path) -> Option<PathBuf> {
    let choices = parse_choices(&std::fs::read_to_string(file).ok()?);
    let path = key(path);
    choices.into_iter().filter(|(k, _)| path.starts_with(k)).max_by_key(|(k, _)| k.components().count()).map(|(_, p)| p)
}

fn choose_in(file: &Path, folder: &Path, python: Option<&Path>) -> Result<(), String> {
    let folder = key(folder);
    let mut choices = parse_choices(&std::fs::read_to_string(file).unwrap_or_default());
    choices.retain(|(k, _)| *k != folder);
    if let Some(p) = python {
        choices.push((folder, p.to_path_buf()));
    }
    choices.sort();
    let text: String = choices.iter().map(|(k, p)| format!("{}\t{}\n", k.display(), p.display())).collect();
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(file, text).map_err(|e| format!("{}: {e}", file.display()))
}

/// The Python chosen for `path` (a file or folder), if it still exists. Not
/// canonicalized: a virtualenv's `bin/python` is a link, and following it leaves the venv.
pub fn chosen(path: &Path) -> Option<PathBuf> {
    chosen_in(&choices_file()?, path).filter(|p| p.is_file())
}

/// Chooses `python` for everything in `folder` (`None`: back to finding one).
pub fn choose(folder: &Path, python: Option<&Path>) -> Result<(), String> {
    choose_in(&choices_file().ok_or("no home folder")?, folder, python)
}

/// "3.12.3", asked of `python` itself.
pub fn version(python: &Path) -> Option<String> {
    let out = std::process::Command::new(python)
        .args(["-c", "import sys; print('%d.%d.%d' % sys.version_info[:3])"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|v| !v.is_empty())
}

/// The virtualenv folder of `python` (`…/.venv` for `…/.venv/bin/python`), if it is one.
pub fn venv_of(python: &Path) -> Option<PathBuf> {
    python.parent()?.parent().filter(|d| d.join("pyvenv.cfg").is_file()).map(Path::to_path_buf)
}

/// Single quotes for a shell, when needed.
fn quote(text: &str) -> String {
    if text.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+=:@".contains(c)) {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

/// The shell command that sets up `project`'s Python: creates `.venv` when `python` is
/// `None`, then installs `requirements.txt` (else the dependencies in `pyproject.toml`)
/// and `packages`. With `uv`, uv does both; else `venv` and pip. `None`: nothing to do.
pub fn setup_command(project: &Path, python: Option<&Path>, uv: bool, packages: &[&str]) -> Option<String> {
    let mut steps = Vec::new();
    let python = match python {
        Some(p) => quote(&p.to_string_lossy()),
        None => {
            steps.push(if uv { "uv venv .venv".to_string() } else { "python3 -m venv .venv".to_string() });
            ".venv/bin/python".to_string()
        }
    };
    let install = |what: &str| if uv { format!("uv pip install --python {python} {what}") } else { format!("{python} -m pip install {what}") };
    let mut what: Vec<String> = Vec::new();
    if project.join("requirements.txt").is_file() {
        what.push("-r requirements.txt".into());
    } else if project.join("pyproject.toml").is_file() {
        // uv reads a pyproject's dependencies; pip installs the project itself (editable).
        what.push(if uv { "-r pyproject.toml".into() } else { "-e .".into() });
    }
    what.extend(packages.iter().map(|p| quote(p)));
    if !what.is_empty() {
        steps.push(install(&what.join(" ")));
    }
    (!steps.is_empty()).then(|| steps.join(" && "))
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
    fn a_project_below_the_workspace_folder_is_its_own_root() {
        let root = std::env::temp_dir().join(format!("tileide-proj-{}", std::process::id()));
        std::fs::create_dir_all(root.join("lab/pandas")).unwrap();
        // Paths reach `project_root` canonicalized (`/tmp` may be a link).
        let root = root.canonicalize().unwrap();
        venv(&root.join("lab/.venv"), true);
        let notebook = root.join("lab/pandas/practice.ipynb");
        assert_eq!(project_root(&notebook, &root), Some(root.join("lab")));
        assert_eq!(project_root(&root.join("README.md"), &root), None, "no venv between the file and the workspace folder");
        assert_eq!(project_root(Path::new("/elsewhere/a.py"), &root), None, "outside the workspace folder");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn choices_go_to_the_nearest_chosen_folder() {
        let root = std::env::temp_dir().join(format!("tileide-choices-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("w/lab/deep")).unwrap();
        let root = root.canonicalize().unwrap();
        let file = root.join("state/interpreters");
        assert_eq!(chosen_in(&file, &root.join("w/lab")), None, "no file yet");
        choose_in(&file, &root.join("w"), Some(Path::new("/usr/bin/python3"))).unwrap();
        choose_in(&file, &root.join("w/lab"), Some(Path::new("/w/lab/.venv/bin/python"))).unwrap();
        assert_eq!(chosen_in(&file, &root.join("w/lab/deep/a.py")), Some(PathBuf::from("/w/lab/.venv/bin/python")));
        assert_eq!(chosen_in(&file, &root.join("w/b.py")), Some(PathBuf::from("/usr/bin/python3")));
        assert_eq!(chosen_in(&file, &root.join("elsewhere")), None);
        // Choosing again replaces; None forgets.
        choose_in(&file, &root.join("w/lab"), None).unwrap();
        assert_eq!(chosen_in(&file, &root.join("w/lab/a.py")), Some(PathBuf::from("/usr/bin/python3")));
        assert_eq!(std::fs::read_to_string(&file).unwrap().lines().count(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn setup_commands_create_and_install() {
        let root = std::env::temp_dir().join(format!("tileide-setup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(setup_command(&root, None, true, &[]).as_deref(), Some("uv venv .venv"));
        assert_eq!(setup_command(&root, Some(Path::new("/p/.venv/bin/python")), true, &[]), None, "nothing to install");
        std::fs::write(root.join("pyproject.toml"), "").unwrap();
        assert_eq!(setup_command(&root, None, true, &["ipykernel"]).as_deref(), Some("uv venv .venv && uv pip install --python .venv/bin/python -r pyproject.toml ipykernel"));
        assert_eq!(setup_command(&root, None, false, &[]).as_deref(), Some("python3 -m venv .venv && .venv/bin/python -m pip install -e ."));
        std::fs::write(root.join("requirements.txt"), "").unwrap();
        assert_eq!(
            setup_command(&root, Some(Path::new("/my env/bin/python")), false, &["ipykernel"]).as_deref(),
            Some("'/my env/bin/python' -m pip install -r requirements.txt ipykernel")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_file_belongs_to_its_project_or_workspace_folder() {
        let root = std::env::temp_dir().join(format!("tileide-project-{}", std::process::id()));
        std::fs::create_dir_all(root.join("lab/src")).unwrap();
        let root = root.canonicalize().unwrap();
        venv(&root.join("lab/.venv"), true);
        assert_eq!(project(&root.join("lab/src/a.py"), Some(&root)), root.join("lab"));
        assert_eq!(project(&root.join("b.py"), Some(&root)), root);
        assert_eq!(project(Path::new("/elsewhere/c.py"), Some(&root)), PathBuf::from("/elsewhere"));
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
