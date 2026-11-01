// SPDX-License-Identifier: GPL-3.0-or-later
//! The LSP messages TileIDE sends and the parts of the replies it shows: diagnostics,
//! hover text, completions and definition locations. Positions here are LSP's
//! (line, column in the server's encoding); `text` converts them.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::text::{Encoding, path_to_uri, uri_to_path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

fn position(v: &Value) -> Option<Position> {
    Some(Position { line: v["line"].as_u64()? as u32, column: v["character"].as_u64()? as u32 })
}

fn range(v: &Value) -> Option<Range> {
    Some(Range { start: position(&v["start"])?, end: position(&v["end"])? })
}

fn position_json(p: Position) -> Value {
    json!({"line": p.line, "character": p.column})
}

/// What the server offered in its `initialize` reply.
#[derive(Debug, Clone, PartialEq)]
pub struct Capabilities {
    pub encoding: Encoding,
    pub hover: bool,
    pub completion: bool,
    pub definition: bool,
    /// Characters that open completion by themselves (`.`, `::`'s `:`, …).
    pub triggers: Vec<char>,
}

impl Capabilities {
    pub fn parse(result: &Value) -> Capabilities {
        let caps = &result["capabilities"];
        let offered = |v: &Value| v.as_bool().unwrap_or(v.is_object());
        Capabilities {
            encoding: Encoding::parse(caps["positionEncoding"].as_str()),
            hover: offered(&caps["hoverProvider"]),
            completion: caps["completionProvider"].is_object(),
            definition: offered(&caps["definitionProvider"]),
            triggers: caps["completionProvider"]["triggerCharacters"].as_array().into_iter().flatten().filter_map(|t| t.as_str()?.chars().last()).collect(),
        }
    }
}

pub fn initialize_params(root: &Path) -> Value {
    let uri = path_to_uri(root);
    let name = root.file_name().map_or_else(|| "root".into(), |n| n.to_string_lossy().into_owned());
    json!({
        "processId": std::process::id(),
        "clientInfo": {"name": "TileIDE", "version": env!("CARGO_PKG_VERSION")},
        "rootUri": uri,
        "rootPath": root.to_string_lossy(),
        "workspaceFolders": [{"uri": uri, "name": name}],
        "capabilities": {
            "general": {"positionEncodings": ["utf-8", "utf-16"]},
            "workspace": {"configuration": true, "workspaceFolders": true},
            "textDocument": {
                "synchronization": {"didSave": true},
                "hover": {"contentFormat": ["markdown", "plaintext"]},
                "completion": {"completionItem": {"snippetSupport": false, "documentationFormat": ["plaintext", "markdown"]}, "contextSupport": true},
                "definition": {"linkSupport": true},
                "publishDiagnostics": {"relatedInformation": false}
            }
        }
    })
}

pub fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

pub fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

pub fn did_open(path: &Path, language: &str, version: i32, text: &str) -> Value {
    notification("textDocument/didOpen", json!({"textDocument": {"uri": path_to_uri(path), "languageId": language, "version": version, "text": text}}))
}

/// Full-text sync: valid whatever sync kind the server asked for.
pub fn did_change(path: &Path, version: i32, text: &str) -> Value {
    notification("textDocument/didChange", json!({"textDocument": {"uri": path_to_uri(path), "version": version}, "contentChanges": [{"text": text}]}))
}

pub fn did_save(path: &Path) -> Value {
    notification("textDocument/didSave", json!({"textDocument": {"uri": path_to_uri(path)}}))
}

pub fn did_close(path: &Path) -> Value {
    notification("textDocument/didClose", json!({"textDocument": {"uri": path_to_uri(path)}}))
}

/// Params for hover, definition and completion.
pub fn at(path: &Path, p: Position) -> Value {
    json!({"textDocument": {"uri": path_to_uri(path)}, "position": position_json(p)})
}

/// The reply to a request the server sends us. Servers stall if these go unanswered
/// (spike 7): configuration gets one `null` per item, everything else an empty success.
pub fn answer_server_request(message: &Value) -> Option<Value> {
    let id = message.get("id")?;
    let method = message.get("method")?.as_str()?;
    let result = match method {
        "workspace/configuration" => Value::Array(vec![Value::Null; message["params"]["items"].as_array().map_or(0, Vec::len)]),
        "workspace/workspaceFolders" => Value::Array(Vec::new()),
        _ => Value::Null,
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub range: Range,
    pub severity: Severity,
    pub message: String,
    /// "rustc", "clang", …
    pub source: Option<String>,
}

/// `textDocument/publishDiagnostics` params → the file and its diagnostics.
pub fn diagnostics(params: &Value) -> Option<(PathBuf, Vec<Diagnostic>)> {
    let path = uri_to_path(params["uri"].as_str()?)?;
    let list = params["diagnostics"]
        .as_array()?
        .iter()
        .filter_map(|d| {
            Some(Diagnostic {
                range: range(&d["range"])?,
                severity: match d["severity"].as_u64() {
                    Some(2) => Severity::Warning,
                    Some(3) => Severity::Info,
                    Some(4) => Severity::Hint,
                    _ => Severity::Error,
                },
                message: d["message"].as_str()?.to_string(),
                source: d["source"].as_str().map(String::from),
            })
        })
        .collect();
    Some((path, list))
}

/// Hover contents as plain text: markdown fences, rules, heading marks and backticks
/// dropped, the words kept.
pub fn hover_text(result: &Value) -> Option<String> {
    fn part(v: &Value) -> String {
        match v {
            Value::String(s) => s.clone(),
            Value::Array(items) => items.iter().map(part).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n\n"),
            Value::Object(_) => v["value"].as_str().unwrap_or("").to_string(),
            _ => String::new(),
        }
    }
    let raw = part(&result["contents"]);
    let mut lines: Vec<String> = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            continue;
        }
        // A rule becomes a paragraph break.
        if !trimmed.is_empty() && trimmed.chars().all(|c| c == '-' || c == '*' || c == '_') && trimmed.len() >= 3 {
            lines.push(String::new());
            continue;
        }
        let line = if trimmed.starts_with('#') { trimmed.trim_start_matches('#').trim_start() } else { line };
        lines.push(line.replace('`', ""));
    }
    let mut text = lines.join("\n").trim().to_string();
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    (!text.is_empty()).then_some(text)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    pub label: String,
    /// Type or signature, shown dimmed.
    pub detail: Option<String>,
    /// What to insert, and the range it replaces (`None`: the word before the cursor).
    pub insert: String,
    pub replace: Option<Range>,
    /// What typing filters against (`filterText`, else the label).
    pub filter: String,
    pub sort: String,
}

/// The completion items, and whether the list is incomplete (re-ask as typing goes on).
pub fn completions(result: &Value) -> (Vec<Completion>, bool) {
    let (items, incomplete) = match result {
        Value::Array(items) => (items.as_slice(), false),
        Value::Object(_) => (result["items"].as_array().map_or(&[][..], Vec::as_slice), result["isIncomplete"].as_bool().unwrap_or(false)),
        _ => (&[][..], false),
    };
    let list = items
        .iter()
        .filter_map(|item| {
            let label = item["label"].as_str()?.trim().to_string();
            let edit = &item["textEdit"];
            // A plain edit has `range`; an insert-replace edit has `insert`/`replace`.
            let replace = range(&edit["range"]).or_else(|| range(&edit["replace"]));
            let insert = edit["newText"].as_str().or(item["insertText"].as_str()).unwrap_or(&label).to_string();
            Some(Completion {
                detail: item["detail"].as_str().map(|d| d.trim().to_string()).filter(|d| !d.is_empty()),
                filter: item["filterText"].as_str().map_or_else(|| label.clone(), String::from),
                sort: item["sortText"].as_str().map_or_else(|| label.clone(), String::from),
                label,
                insert,
                replace,
            })
        })
        .collect();
    (list, incomplete)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    pub path: PathBuf,
    pub start: Position,
}

/// `Location`, `Location[]` or `LocationLink[]` → the first local one.
pub fn definition(result: &Value) -> Option<Location> {
    let first = match result {
        Value::Array(items) => items.first()?,
        Value::Object(_) => result,
        _ => return None,
    };
    let uri = first["uri"].as_str().or(first["targetUri"].as_str())?;
    let start = position(&first["range"]["start"]).or_else(|| position(&first["targetSelectionRange"]["start"]))?;
    Some(Location { path: uri_to_path(uri)?, start })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_read_what_matters() {
        let caps = Capabilities::parse(&json!({"capabilities": {
            "positionEncoding": "utf-8", "hoverProvider": true, "definitionProvider": {},
            "completionProvider": {"triggerCharacters": [".", "::", ">"]}
        }}));
        assert_eq!(caps, Capabilities { encoding: Encoding::Utf8, hover: true, completion: true, definition: true, triggers: vec!['.', ':', '>'] });
        let none = Capabilities::parse(&json!({"capabilities": {}}));
        assert!(!none.hover && !none.completion && !none.definition && none.encoding == Encoding::Utf16);
    }

    #[test]
    fn server_requests_get_answers() {
        let config = json!({"jsonrpc": "2.0", "id": 4, "method": "workspace/configuration", "params": {"items": [{}, {}]}});
        assert_eq!(answer_server_request(&config).unwrap()["result"], json!([null, null]));
        let register = json!({"jsonrpc": "2.0", "id": "a", "method": "client/registerCapability", "params": {}});
        assert_eq!(answer_server_request(&register).unwrap(), json!({"jsonrpc": "2.0", "id": "a", "result": null}));
        assert!(answer_server_request(&json!({"jsonrpc": "2.0", "method": "window/logMessage"})).is_none(), "notifications need none");
        assert!(answer_server_request(&json!({"jsonrpc": "2.0", "id": 1, "result": {}})).is_none(), "responses need none");
    }

    #[test]
    fn diagnostics_parse_with_default_severity() {
        let params = json!({"uri": "file:///a/b.c", "diagnostics": [
            {"range": {"start": {"line": 3, "character": 1}, "end": {"line": 3, "character": 5}}, "message": "boom", "source": "clang"},
            {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "severity": 2, "message": "hm"},
            {"message": "no range is skipped"}
        ]});
        let (path, list) = diagnostics(&params).unwrap();
        assert_eq!(path, Path::new("/a/b.c"));
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].severity, list[0].source.as_deref(), list[0].range.start.line), (Severity::Error, Some("clang"), 3));
        assert_eq!(list[1].severity, Severity::Warning);
    }

    #[test]
    fn hover_flattens_every_shape() {
        assert_eq!(hover_text(&json!({"contents": {"kind": "markdown", "value": "```rust\nfn area(w: i32) -> i32\n```\n\nThe area."}})).as_deref(), Some("fn area(w: i32) -> i32\n\nThe area."));
        assert_eq!(hover_text(&json!({"contents": ["a", {"language": "c", "value": "int x"}]})).as_deref(), Some("a\n\nint x"));
        assert_eq!(hover_text(&json!({"contents": {"kind": "markdown", "value": "### function `area`\n\n---\n→ `int`\n\n---\nstatic int area()"}})).as_deref(), Some("function area\n\n→ int\n\nstatic int area()"));
        assert_eq!(hover_text(&json!({"contents": ""})), None);
        assert_eq!(hover_text(&Value::Null), None);
    }

    #[test]
    fn completions_take_edits_or_insert_text() {
        let result = json!({"isIncomplete": true, "items": [
            {"label": " ratio", "detail": "double", "textEdit": {"newText": "ratio", "range": {"start": {"line": 1, "character": 6}, "end": {"line": 1, "character": 7}}}},
            {"label": "id", "insertText": "id", "sortText": "0"},
            {"label": "push", "textEdit": {"newText": "push", "insert": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "replace": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 3}}}}
        ]});
        let (items, incomplete) = completions(&result);
        assert!(incomplete);
        assert_eq!((items[0].label.as_str(), items[0].detail.as_deref(), items[0].replace.unwrap().start.column), ("ratio", Some("double"), 6));
        assert_eq!((items[1].insert.as_str(), items[1].replace, items[1].sort.as_str()), ("id", None, "0"));
        assert_eq!(items[2].replace.unwrap().end.column, 3, "insert-replace edits use the replace range");
        assert_eq!(completions(&json!([{"label": "x"}])).0.len(), 1, "a bare array is complete");
    }

    #[test]
    fn definitions_accept_locations_and_links() {
        let at = |line| json!({"start": {"line": line, "character": 2}, "end": {"line": line, "character": 4}});
        assert_eq!(definition(&json!({"uri": "file:///x.c", "range": at(9)})), Some(Location { path: "/x.c".into(), start: Position { line: 9, column: 2 } }));
        assert_eq!(definition(&json!([{"targetUri": "file:///y.rs", "targetRange": at(1), "targetSelectionRange": at(2)}])).unwrap().start.line, 2);
        assert_eq!(definition(&json!([])), None);
        assert_eq!(definition(&Value::Null), None);
    }
}
