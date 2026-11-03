// SPDX-License-Identifier: GPL-3.0-or-later
//! Quick open (Alt+E / Alt+F): the workspace's file list and a fuzzy matcher.

use std::path::{Path, PathBuf};

/// Folders never listed: version control, build output, dependencies, caches.
const SKIP_DIRS: &[&str] = &["target", "node_modules", "__pycache__", "build", "dist", "venv"];

/// Files under `root`, relative to it, sorted. Hidden entries and `SKIP_DIRS` are
/// skipped; at most `limit` files, so a huge folder (e.g. `~`) can't stall the search.
pub fn list_files(root: &Path, limit: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            // `file_type` does not follow symlinks, so a link loop can't recurse forever.
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if !SKIP_DIRS.contains(&name.as_ref()) {
                    stack.push(entry.path());
                }
            } else if let Ok(relative) = entry.path().strip_prefix(root) {
                out.push(relative.to_path_buf());
                if out.len() >= limit {
                    out.sort();
                    return out;
                }
            }
        }
    }
    out.sort();
    out
}

/// Folders under `root` (relative to it), at most `depth` levels down, breadth first so
/// near folders come before deep ones when `limit` cuts the list. Same skips as
/// `list_files`; a folder's contents are not listed below a skipped one.
pub fn list_dirs(root: &Path, depth: usize, limit: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut level = vec![root.to_path_buf()];
    for _ in 0..depth {
        let mut next = Vec::new();
        for dir in level {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            let mut entries: Vec<_> = entries.flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) || !entry.file_type().is_ok_and(|k| k.is_dir()) {
                    continue;
                }
                if let Ok(relative) = entry.path().strip_prefix(root) {
                    out.push(relative.to_path_buf());
                    if out.len() >= limit {
                        return out;
                    }
                }
                next.push(entry.path());
            }
        }
        level = next;
    }
    out
}

/// Scores `candidate` (a relative path) against `query`; `None` if the query's
/// characters don't all appear in order. Case-insensitive. Higher is better: runs of
/// consecutive characters, starts of words, and matches in the file name count most;
/// shorter paths win ties.
pub fn score(query: &str, candidate: &str) -> Option<i64> {
    let query: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect();
    if query.is_empty() {
        return Some(-(candidate.len() as i64));
    }
    let chars: Vec<char> = candidate.chars().collect();
    let name_start = candidate.rfind('/').map_or(0, |i| candidate[..=i].chars().count());
    let mut total = 0i64;
    let mut qi = 0;
    let mut previous: Option<usize> = None;
    for (i, &c) in chars.iter().enumerate() {
        if qi == query.len() {
            break;
        }
        if c.to_lowercase().eq(std::iter::once(query[qi])) {
            let mut points = 1;
            if previous == Some(i.wrapping_sub(1)) {
                points += 5;
            }
            let boundary = i == 0 || matches!(chars[i - 1], '/' | '_' | '-' | '.' | ' ') || (chars[i - 1].is_lowercase() && c.is_uppercase());
            if boundary {
                points += 4;
            }
            if i >= name_start {
                points += 2;
            }
            total += points;
            previous = Some(i);
            qi += 1;
        }
    }
    (qi == query.len()).then(|| total * 1000 - chars.len() as i64)
}

/// The best `max` matches, best first.
pub fn rank<'a>(query: &str, files: &'a [PathBuf], max: usize) -> Vec<&'a Path> {
    let mut scored: Vec<(i64, &Path)> = files.iter().filter_map(|f| score(query, &f.to_string_lossy()).map(|s| (s, f.as_path()))).collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    scored.into_iter().take(max).map(|(_, f)| f).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_in_order_only() {
        assert!(score("mrs", "src/main.rs").is_some());
        assert!(score("srm", "src/main.rs").is_some());
        assert!(score("zz", "src/main.rs").is_none());
        assert!(score("MAIN", "src/main.rs").is_some(), "case-insensitive");
    }

    #[test]
    fn file_names_and_runs_rank_first() {
        let files: Vec<PathBuf> = ["docs/main-notes.md", "src/main.rs", "crates/core/src/domain.rs", "src/tile_view.rs"].iter().map(PathBuf::from).collect();
        assert_eq!(rank("main", &files, 10)[0], Path::new("src/main.rs"));
        assert_eq!(rank("tv", &files, 10)[0], Path::new("src/tile_view.rs"), "word starts");
        assert_eq!(rank("", &files, 2), vec![Path::new("src/main.rs"), Path::new("src/tile_view.rs")], "empty query: shortest first");
    }

    #[test]
    fn lists_files_skipping_hidden_and_build_dirs() {
        let root = std::env::temp_dir().join(format!("tileide-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for f in ["a.rs", "src/b.rs", ".git/config", "target/debug/x", "node_modules/m/i.js", ".env"] {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        assert_eq!(list_files(&root, 100), vec![PathBuf::from("a.rs"), PathBuf::from("src/b.rs")]);
        assert_eq!(list_files(&root, 1).len(), 1, "stops at the limit");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn lists_dirs_breadth_first_to_a_depth() {
        let root = std::env::temp_dir().join(format!("tileide-dirs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["b/deep/deeper", "a", ".git/x", "target/y"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("file"), "").unwrap();
        let dirs = |depth, limit| list_dirs(&root, depth, limit);
        assert_eq!(dirs(2, 100), ["a", "b", "b/deep"].map(PathBuf::from));
        assert_eq!(dirs(2, 2), ["a", "b"].map(PathBuf::from), "near folders first");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
