// SPDX-License-Identifier: GPL-3.0-or-later
//! Which Python a project uses, and setting one up (v0.5 phase 7). `Alt+P` in a file,
//! or the kernel chip in a notebook, lists the Pythons found for it (the one in use
//! marked), a notebook's other Jupyter kernels, and "create .venv / install
//! requirements", which runs uv (else venv + pip) in a terminal tile so its output and
//! any prompt stay visible. A pick is kept per project folder
//! (`tileide_core::python::choose`) and serves Run, debugging, notebooks and the
//! language server alike; a notebook's kernel is kept in the notebook.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk4::glib;
use tileide_core::python;
use tileide_notebook::kernel::{self, Spec};

#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    Python(PathBuf),
    Kernel(Spec),
    /// A shell command to run in a terminal tile in this folder, and the Python it sets
    /// up (chosen right away; used once it exists).
    Setup { command: String, dir: PathBuf, python: PathBuf },
}

#[derive(Debug, Clone)]
pub struct Row {
    pub text: String,
    pub pick: Pick,
    pub current: bool,
}

/// Opens a terminal tile in a folder, running a command line.
type OpenTerminal = Box<dyn Fn(&Path, &str)>;

/// What the app lends this module: a terminal tile running a command, and a notice.
pub struct Hooks {
    pub terminal: OpenTerminal,
    pub notice: Box<dyn Fn(String)>,
}

thread_local! {
    static HOOKS: RefCell<Option<Rc<Hooks>>> = const { RefCell::new(None) };
}

pub fn set_hooks(hooks: Hooks) {
    HOOKS.with_borrow_mut(|h| *h = Some(Rc::new(hooks)));
}

fn hooks() -> Option<Rc<Hooks>> {
    HOOKS.with_borrow(Clone::clone)
}

/// `path` from the project's parent ("lab/.venv"), else from home ("~/envs/ds"), else whole.
fn shown(path: &Path, project: &Path) -> String {
    if let Some(rel) = project.parent().and_then(|p| path.strip_prefix(p).ok()) {
        return rel.display().to_string();
    }
    match std::env::var_os("HOME").map(PathBuf::from).and_then(|h| path.strip_prefix(&h).ok().map(Path::to_path_buf)) {
        Some(rel) => format!("~/{}", rel.display()),
        None => path.display().to_string(),
    }
}

/// A Python as its virtualenv's folder, else its own path.
fn place(python: &Path, project: &Path) -> String {
    shown(&python::venv_of(python).unwrap_or_else(|| python.to_path_buf()), project)
}

/// The label a kernel chip shows for `python`: its virtualenv's folder name, else "system".
pub fn short_name(python: &Path) -> String {
    python::venv_of(python).and_then(|v| v.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "system".into())
}

/// The rows for `file` (blocking: asks each Python its version; run off the main
/// thread). `in_use` is the Python a notebook's kernel runs on; `kernel` the
/// notebook's kernel name (`None` for a plain file).
fn gather_now(file: &Path, folder: Option<&Path>, in_use: Option<&Path>, kernel: Option<&str>) -> Vec<Row> {
    let project = python::project(file, folder);
    let dir = file.parent().unwrap_or(file);
    let mut pythons = python::candidates(dir, folder);
    // `python3` by its real path, so it can be chosen and compared.
    for p in &mut pythons {
        if !p.is_absolute()
            && let Some(full) = python::which(&p.to_string_lossy())
        {
            *p = full;
        }
    }
    let mut seen = Vec::new();
    pythons.retain(|p| {
        let new = !seen.contains(p);
        seen.push(p.clone());
        new
    });
    let current = in_use.map(Path::to_path_buf).or_else(|| python::chosen(dir)).or_else(|| pythons.first().cloned());
    let python_kernel = kernel.is_none_or(|k| k == "python3");
    let mut rows: Vec<Row> = pythons
        .iter()
        .map(|p| {
            let version = python::version(p).map_or_else(|| "not working".into(), |v| format!("Python {v}"));
            Row { text: format!("{} · {version}", place(p, &project)), pick: Pick::Python(p.clone()), current: python_kernel && current.as_ref() == Some(p) }
        })
        .collect();
    if let Some(kernel) = kernel {
        // Kernels other than the plain Python one, as the first Python with Jupyter lists them.
        if let Some(jupyter) = pythons.iter().find(|p| kernel::has_jupyter(p)) {
            for spec in kernel::kernelspecs(jupyter).into_iter().filter(|s| s.name != "python3") {
                rows.push(Row { text: format!("Kernel: {} ({})", spec.display, spec.name), current: spec.name == kernel, pick: Pick::Kernel(spec) });
            }
        }
    }
    let uv = python::which("uv").is_some();
    let tool = if uv { "uv" } else { "pip" };
    let local = python::venvs(&project, Some(&project), None, None).into_iter().next();
    // Installs go to the Python asked for (it may not be running yet), if it is a
    // virtualenv, else to the project's own.
    let target = python::chosen(dir).or_else(|| current.clone()).filter(|p| python::venv_of(p).is_some()).or_else(|| local.clone());
    // A notebook needs ipykernel in the Python it runs on.
    let packages: &[&str] = match &target {
        Some(p) if kernel.is_some() && !kernel::has_jupyter(p) => &["ipykernel"],
        None if kernel.is_some() => &["ipykernel"],
        _ => &[],
    };
    if local.is_none()
        && let Some(command) = python::setup_command(&project, None, uv, packages)
    {
        let text = format!("Create .venv in {} and install ({tool})", shown(&project, &project));
        rows.push(Row { text, pick: Pick::Setup { command, dir: project.clone(), python: project.join(".venv/bin/python") }, current: false });
    }
    if let Some(p) = &target
        && let Some(command) = python::setup_command(&project, Some(p), uv, packages)
    {
        let what = if packages.is_empty() { "requirements" } else if command.contains(" -r ") || command.contains(" -e .") { "requirements and ipykernel" } else { "ipykernel" };
        rows.push(Row { text: format!("Install {what} into {} ({tool})", place(p, &project)), pick: Pick::Setup { command, dir: project.clone(), python: p.clone() }, current: false });
    }
    rows
}

/// `gather_now` on a thread; `done` gets the rows on the main loop.
pub fn gather(file: &Path, folder: Option<&Path>, in_use: Option<&Path>, kernel: Option<&str>, done: impl FnOnce(Vec<Row>) + 'static) {
    let (file, folder, in_use, kernel) = (file.to_path_buf(), folder.map(Path::to_path_buf), in_use.map(Path::to_path_buf), kernel.map(String::from));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(gather_now(&file, folder.as_deref(), in_use.as_deref(), kernel.as_deref()));
    });
    let mut done = Some(done);
    glib::timeout_add_local(Duration::from_millis(40), move || match rx.try_recv() {
        Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Ok(rows) => {
            if let Some(done) = done.take() {
                done(rows);
            }
            glib::ControlFlow::Break
        }
        Err(_) => glib::ControlFlow::Break,
    });
}

/// The list's text for a row: Pythons and kernels as radio choices (the one in use
/// filled), setups as actions.
pub fn label(row: &Row) -> String {
    let mark = match (&row.pick, row.current) {
        (Pick::Setup { .. }, _) => "+",
        (_, true) => "●",
        (_, false) => "○",
    };
    format!("{mark} {}", row.text)
}

/// Uses `python` for everything in `file`'s project; tells the language server.
pub fn use_python(file: &Path, folder: Option<&Path>, python: &Path) {
    let project = python::project(file, folder);
    let message = match python::choose(&project, Some(python)) {
        Ok(()) => {
            crate::lsp::python_changed(&project);
            format!("{} now uses {}", project.display(), python.display())
        }
        Err(e) => format!("Couldn't keep the choice: {e}"),
    };
    println!("interpreter: {message}");
    if let Some(h) = hooks() {
        (h.notice)(message);
    }
}

/// Runs a setup command in a new terminal tile, and chooses the Python it sets up.
pub fn setup(file: &Path, folder: Option<&Path>, command: &str, dir: &Path, python: &Path) {
    let project = python::project(file, folder);
    if let Err(e) = python::choose(&project, Some(python)) {
        eprintln!("interpreter: {e}");
    }
    crate::lsp::python_changed(&project);
    println!("interpreter: running `{command}` in {}", dir.display());
    if let Some(h) = hooks() {
        (h.terminal)(dir, command);
    }
}

/// Runs `command` in a new terminal tile in `dir`.
pub fn terminal(dir: &Path, command: &str) {
    if let Some(h) = hooks() {
        (h.terminal)(dir, command);
    }
}

/// `Alt+P` in a file: the list at the cursor.
pub fn pick_for_file(view: &sourceview5::View, file: &Path, folder: Option<&Path>) {
    let (view, file, folder) = (view.clone(), file.to_path_buf(), folder.map(Path::to_path_buf));
    gather(&file.clone(), folder.clone().as_deref(), None, None, move |rows| {
        let texts: Vec<String> = rows.iter().map(label).collect();
        crate::pick_card::show(&view, "Python for this project", &texts, move |i| apply(&rows[i], &file, folder.as_deref()));
    });
}

/// What picking `row` for a plain file does (a notebook handles kernels itself).
pub fn apply(row: &Row, file: &Path, folder: Option<&Path>) {
    match &row.pick {
        Pick::Python(p) => use_python(file, folder, p),
        Pick::Setup { command, dir, python } => setup(file, folder, command, dir, python),
        Pick::Kernel(_) => {}
    }
}

/// Test action `pythons[:N]`: prints the rows for `file`, then picks row N.
pub fn test(file: &Path, folder: Option<&Path>, pick: Option<usize>) {
    let (file, folder) = (file.to_path_buf(), folder.map(Path::to_path_buf));
    gather(&file.clone(), folder.clone().as_deref(), None, None, move |rows| {
        for r in &rows {
            println!("interpreter row: {}", label(r));
        }
        if let Some(row) = pick.and_then(|i| rows.get(i)) {
            apply(row, &file, folder.as_deref());
        }
    });
}
