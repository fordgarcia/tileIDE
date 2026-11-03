// SPDX-License-Identifier: GPL-3.0-or-later
//! Running pytest (v0.5 phase 9): the project's Python runs `pytest -p tileide_pytest`,
//! a small plugin (`tileide_pytest.py`) that reports what was collected and each result
//! as JSON lines; everything else pytest prints is kept as plain output. The plugin is
//! written to the runtime folder and found through `PYTHONPATH`, so nothing is
//! installed into the project.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use serde_json::Value;

const PLUGIN: &str = include_str!("tileide_pytest.py");
const MARK: &str = "\x1etileide ";

#[derive(Debug, Clone, PartialEq)]
pub struct Test {
    /// pytest's node id (`tests/test_a.py::TestX::test_y`).
    pub id: String,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    Passed,
    Failed,
    Skipped,
    /// A fixture (setup or teardown) failed.
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub id: String,
    pub outcome: Outcome,
    pub seconds: f64,
    /// One line on why it failed or was skipped.
    pub message: String,
    /// pytest's whole report of a failure.
    pub text: String,
    /// Where it failed: the deepest project line of the traceback (file relative to
    /// the run's folder, 1-based line).
    pub at: Option<(String, u32)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Collected(Vec<Test>),
    Result(Report),
    /// pytest's exit status (0 all passed, 1 some failed, 2 interrupted or collection
    /// errors, 4 usage error, 5 no tests).
    Finished(i64),
    /// Any other line pytest printed.
    Output(String),
    /// The process ended.
    Exited,
}

pub fn parse(line: &str) -> Event {
    let Some(json) = line.strip_prefix(MARK) else { return Event::Output(line.to_string()) };
    let Ok(v) = serde_json::from_str::<Value>(json) else { return Event::Output(line.to_string()) };
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    match v["ev"].as_str() {
        Some("collected") => Event::Collected(
            v["tests"].as_array().into_iter().flatten().map(|t| Test { id: s(&t["id"]), file: s(&t["file"]), line: t["line"].as_u64().unwrap_or(1) as u32 }).collect(),
        ),
        Some("result") => Event::Result(Report {
            id: s(&v["id"]),
            outcome: match v["outcome"].as_str() {
                Some("passed") => Outcome::Passed,
                Some("skipped") => Outcome::Skipped,
                Some("error") => Outcome::Error,
                _ => Outcome::Failed,
            },
            seconds: v["seconds"].as_f64().unwrap_or(0.0),
            message: s(&v["message"]),
            text: s(&v["text"]),
            at: v["at"].as_array().and_then(|a| Some((a.first()?.as_str()?.to_string(), a.get(1)?.as_u64()? as u32))),
        }),
        Some("finished") => Event::Finished(v["status"].as_i64().unwrap_or(0)),
        _ => Event::Output(line.to_string()),
    }
}

/// Whether `path` is a test file by pytest's default names (`test_*.py`, `*_test.py`).
pub fn is_test_file(path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    name.ends_with(".py") && (name.starts_with("test_") || name.ends_with("_test.py"))
}

/// The folder pytest runs in for `file` inside `project`: the nearest one with pytest's
/// configuration (`pytest.ini`, `pyproject.toml`, `tox.ini`, `setup.cfg`), else `project`.
pub fn run_dir(file: Option<&Path>, project: &Path) -> PathBuf {
    let start = file.and_then(Path::parent).filter(|d| d.starts_with(project)).unwrap_or(project);
    start
        .ancestors()
        .take_while(|d| d.starts_with(project))
        .find(|d| ["pytest.ini", "pyproject.toml", "tox.ini", "setup.cfg"].iter().any(|n| d.join(n).is_file()))
        .unwrap_or(project)
        .to_path_buf()
}

/// The node id of the test around 0-based `line` of `source` (the file at `rel`, as
/// pytest names it): the nearest `def test…` above it, with its `class Test…` if it is
/// a method. `None` outside a test.
pub fn test_at(source: &str, line: usize, rel: &str) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    let indent = |l: &str| l.len() - l.trim_start().len();
    let def_name = |l: &str| {
        let t = l.trim_start();
        let t = t.strip_prefix("async ").unwrap_or(t);
        t.strip_prefix("def ").map(|r| r.split(|c: char| c == '(' || c.is_whitespace()).next().unwrap_or("").to_string())
    };
    let class_name = |l: &str| l.trim_start().strip_prefix("class ").map(|r| r.split(|c: char| c == '(' || c == ':' || c.is_whitespace()).next().unwrap_or("").to_string());
    let mut i = line.min(lines.len().checked_sub(1)?);
    // The `def` that encloses the line: the nearest above indented less than every
    // line passed on the way (blank lines don't count); the line itself may be the def.
    let mut limit = usize::MAX;
    loop {
        let l = lines[i];
        if !l.trim().is_empty() {
            let n = indent(l);
            if let Some(name) = def_name(l).filter(|_| n < limit) {
                if !name.starts_with("test") {
                    return None;
                }
                // A method: its class, at a lower indent.
                let class = (n > 0).then(|| (0..i).rev().map(|j| lines[j]).find(|c| !c.trim().is_empty() && indent(c) < n).and_then(class_name)).flatten();
                return Some(match class {
                    Some(c) => format!("{rel}::{c}::{name}"),
                    None if n == 0 => format!("{rel}::{name}"),
                    None => return None,
                });
            }
            if let Some(c) = class_name(l).filter(|_| n < limit && n == 0) {
                // On a test class's own line: the whole class.
                return c.starts_with("Test").then(|| format!("{rel}::{c}"));
            }
            limit = limit.min(n);
        }
        if i == 0 {
            return None;
        }
        i -= 1;
    }
}

/// A running pytest.
pub struct Run {
    child: Child,
}

impl Run {
    /// Runs pytest with `python` in `dir` on `args` (node ids; none = everything);
    /// events arrive through `deliver` on reader threads. `collect` only collects.
    pub fn start(python: &Path, dir: &Path, args: &[String], collect: bool, deliver: impl Fn(Event) + Send + Clone + 'static) -> Result<Run, String> {
        let plugin_dir = gtk4::glib::user_runtime_dir().join("tileide-pytest");
        std::fs::create_dir_all(&plugin_dir).map_err(|e| format!("{}: {e}", plugin_dir.display()))?;
        std::fs::write(plugin_dir.join("tileide_pytest.py"), PLUGIN).map_err(|e| format!("{}: {e}", plugin_dir.display()))?;
        let mut path = vec![plugin_dir];
        path.extend(std::env::var_os("PYTHONPATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default());
        let mut cmd = Command::new(python);
        cmd.args(["-m", "pytest", "-p", "tileide_pytest", "-q", "--color=no"]);
        if collect {
            cmd.arg("--collect-only");
        }
        cmd.args(args)
            .current_dir(dir)
            .env("PYTHONPATH", std::env::join_paths(path).map_err(|e| e.to_string())?)
            .env("PYTHONUNBUFFERED", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Tests end with TileIDE.
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        let mut child = cmd.spawn().map_err(|e| format!("cannot start {}: {e}", python.display()))?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr = child.stderr.take().ok_or("no stderr")?;
        let d = deliver.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                d(Event::Output(line));
            }
        });
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if !line.is_empty() {
                    deliver(parse(&line));
                }
            }
            deliver(Event::Exited);
        });
        Ok(Run { child })
    }

    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Reaps the finished process.
    pub fn finish(mut self) {
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_plugin_lines() {
        assert_eq!(parse("1 passed in 0.01s"), Event::Output("1 passed in 0.01s".into()));
        let line = format!("{MARK}{}", r#"{"ev": "result", "id": "t.py::test_a", "outcome": "failed", "seconds": 0.5, "message": "assert 1 == 2", "text": "…", "at": ["t.py", 4]}"#);
        let Event::Result(r) = parse(&line) else { panic!() };
        assert_eq!((r.outcome, r.at), (Outcome::Failed, Some(("t.py".into(), 4))));
        let line = format!("{MARK}{}", r#"{"ev": "collected", "tests": [{"id": "t.py::test_a", "file": "t.py", "line": 3, "name": "test_a"}]}"#);
        assert_eq!(parse(&line), Event::Collected(vec![Test { id: "t.py::test_a".into(), file: "t.py".into(), line: 3 }]));
    }

    #[test]
    fn finds_the_test_at_the_cursor() {
        let src = "import pytest\n\ndef helper():\n    pass\n\ndef test_a():\n    x = 1\n\n    assert x\n\nclass TestB:\n    def test_c(self):\n        assert True\n\n    def setup(self):\n        pass\n";
        let at = |line| test_at(src, line, "tests/t.py");
        assert_eq!(at(8).as_deref(), Some("tests/t.py::test_a"), "after a blank line in the test");
        assert_eq!(at(5).as_deref(), Some("tests/t.py::test_a"), "on the def");
        assert_eq!(at(12).as_deref(), Some("tests/t.py::TestB::test_c"));
        assert_eq!(at(10).as_deref(), Some("tests/t.py::TestB"), "on the class line");
        assert_eq!(at(15), None, "a non-test method");
        assert_eq!(at(3), None, "a helper");
        assert_eq!(at(0), None, "module level");
        assert_eq!(test_at("def test_a():\n    pass\nx = 1\n", 2, "t.py"), None, "module code after a test");
        assert!(is_test_file(Path::new("/p/tests/test_x.py")) && is_test_file(Path::new("x_test.py")) && !is_test_file(Path::new("conftest.py")));
    }
}
