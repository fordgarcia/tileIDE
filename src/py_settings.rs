// SPDX-License-Identifier: GPL-3.0-or-later
//! A Python project's own tool settings, shown read-only in Settings → Toolchains
//! (v0.5 phase 9): Ruff's, pyright's and pytest's sections of `pyproject.toml`, and
//! `ruff.toml` / `.ruff.toml` / `pyrightconfig.json`. Ruff and pyright read these
//! themselves; TileIDE only shows what they will use. TOML is read line by line
//! (sections and `key = value`, multi-line arrays joined), not fully parsed.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    /// "Ruff · pyproject.toml [tool.ruff.lint]".
    pub title: String,
    pub file: PathBuf,
    pub entries: Vec<(String, String)>,
}

const FILES: &[&str] = &["pyproject.toml", "ruff.toml", ".ruff.toml", "pyrightconfig.json"];

/// The folder whose settings apply to `start`: the nearest at or above it (up to
/// `stop`) holding one of the files, else `start`.
pub fn project_dir(start: &Path, stop: Option<&Path>) -> PathBuf {
    for d in start.ancestors() {
        if FILES.iter().any(|f| d.join(f).is_file()) {
            return d.to_path_buf();
        }
        if stop.is_some_and(|s| s == d) {
            break;
        }
    }
    start.to_path_buf()
}

/// The tool for a `pyproject.toml` table, if it is one shown here.
fn tool(table: &str) -> Option<&'static str> {
    let t = table.strip_prefix("tool.")?;
    let first = t.split('.').next()?;
    match first {
        "ruff" => Some("Ruff"),
        "pyright" => Some("pyright"),
        "basedpyright" => Some("basedpyright"),
        "pytest" => Some("pytest"),
        _ => None,
    }
}

/// `[table]` → (table name, its `key = value` lines), in file order.
fn toml_tables(text: &str) -> Vec<(String, Vec<(String, String)>)> {
    let mut tables: Vec<(String, Vec<(String, String)>)> = vec![(String::new(), Vec::new())];
    let mut open: Option<(String, String)> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if let Some((_, value)) = open.as_mut() {
            // A multi-line array: joined until its closing bracket.
            value.push(' ');
            value.push_str(&without_comment(line));
            if line.starts_with(']') || line.ends_with(']') {
                if let (Some((k, v)), Some(t)) = (open.take(), tables.last_mut()) {
                    t.1.push((k, v.replace("[ ", "[").replace(", ]", "]").replace(",]", "]").replace(" ]", "]")));
                }
            }
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            let name = line.trim_start_matches('[').split(']').next().unwrap_or("").trim().to_string();
            tables.push((name, Vec::new()));
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let (k, v) = (k.trim().to_string(), without_comment(v.trim()));
            let opens = v.matches('[').count() > v.matches(']').count();
            if opens {
                open = Some((k, v));
            } else if let Some(t) = tables.last_mut() {
                t.1.push((k, v));
            }
        }
    }
    tables
}

/// `text` without a trailing `# comment` (a `#` inside quotes stays).
fn without_comment(text: &str) -> String {
    let mut quote = None;
    for (i, c) in text.char_indices() {
        match (c, quote) {
            ('"' | '\'', None) => quote = Some(c),
            (q, Some(open)) if q == open => quote = None,
            ('#', None) => return text[..i].trim_end().to_string(),
            _ => {}
        }
    }
    text.to_string()
}

/// Every section for the project in `dir`, in the order the tools prefer their files.
pub fn read(dir: &Path) -> Vec<Section> {
    let mut out = Vec::new();
    let pyproject = dir.join("pyproject.toml");
    let ruff_own = [".ruff.toml", "ruff.toml"].iter().map(|f| dir.join(f)).find(|p| p.is_file());
    let pyright_own = Some(dir.join("pyrightconfig.json")).filter(|p| p.is_file());
    if let Some(path) = &ruff_own
        && let Ok(text) = std::fs::read_to_string(path)
    {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        for (table, entries) in toml_tables(&text).into_iter().filter(|(_, e)| !e.is_empty()) {
            let title = if table.is_empty() { format!("Ruff · {name}") } else { format!("Ruff · {name} [{table}]") };
            out.push(Section { title, file: path.clone(), entries });
        }
    }
    if let Some(path) = &pyright_own
        && let Ok(text) = std::fs::read_to_string(path)
    {
        let entries = match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(serde_json::Value::Object(map)) => map.into_iter().map(|(k, v)| (k, v.to_string())).collect(),
            _ => vec![("(unreadable)".into(), "not valid JSON".into())],
        };
        out.push(Section { title: "pyright · pyrightconfig.json".into(), file: path.clone(), entries });
    }
    if let Ok(text) = std::fs::read_to_string(&pyproject) {
        for (table, entries) in toml_tables(&text).into_iter().filter(|(_, e)| !e.is_empty()) {
            let Some(tool) = tool(&table) else { continue };
            let mut title = format!("{tool} · pyproject.toml [{table}]");
            // The tool reads its own file instead, when there is one.
            let ignored = match tool {
                "Ruff" => ruff_own.is_some(),
                "pyright" | "basedpyright" => pyright_own.is_some(),
                _ => false,
            };
            if ignored {
                title.push_str(" · not used: its own file wins");
            }
            out.push(Section { title, file: pyproject.clone(), entries });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tool_tables_and_own_files() {
        let dir = std::env::temp_dir().join(format!("tileide-pyset-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src/pkg")).unwrap();
        std::fs::write(
            dir.join("pyproject.toml"),
            "[project]\nname = \"x\"\n\n[tool.ruff]\nline-length = 100 # wide\n\n[tool.ruff.lint]\nselect = [\n  \"E\",\n  \"I\",\n]\n\n[tool.black]\nline-length = 88\n\n[tool.pyright]\ntypeCheckingMode = \"strict\"\n",
        )
        .unwrap();
        assert_eq!(project_dir(&dir.join("src/pkg"), Some(&dir)), dir);
        let s = read(&dir);
        let titles: Vec<&str> = s.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["Ruff · pyproject.toml [tool.ruff]", "Ruff · pyproject.toml [tool.ruff.lint]", "pyright · pyproject.toml [tool.pyright]"]);
        assert_eq!(s[0].entries, [("line-length".to_string(), "100".to_string())]);
        assert_eq!(s[1].entries, [("select".to_string(), r#"["E", "I"]"#.to_string())]);
        assert_eq!(without_comment(r#""a#b" # c"#), r#""a#b""#);
        std::fs::write(dir.join("pyrightconfig.json"), r#"{"venv": ".venv"}"#).unwrap();
        let s = read(&dir);
        assert_eq!(s[0].title, "pyright · pyrightconfig.json");
        assert_eq!(s[0].entries, [("venv".to_string(), "\".venv\"".to_string())]);
        assert!(s.last().unwrap().title.ends_with("not used: its own file wins"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
