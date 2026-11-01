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
    /// Characters that open signature help (`(`, `,`); empty when it isn't offered.
    pub signature_triggers: Vec<char>,
    pub signature: bool,
    pub formatting: bool,
    pub range_formatting: bool,
    pub rename: bool,
    pub references: bool,
    pub code_actions: bool,
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
            triggers: chars(&caps["completionProvider"]["triggerCharacters"]),
            signature_triggers: chars(&caps["signatureHelpProvider"]["triggerCharacters"]).into_iter().chain(chars(&caps["signatureHelpProvider"]["retriggerCharacters"])).collect(),
            signature: caps["signatureHelpProvider"].is_object(),
            formatting: offered(&caps["documentFormattingProvider"]),
            range_formatting: offered(&caps["documentRangeFormattingProvider"]),
            rename: offered(&caps["renameProvider"]),
            references: offered(&caps["referencesProvider"]),
            code_actions: offered(&caps["codeActionProvider"]),
        }
    }
}

/// The last character of each string in a JSON array.
fn chars(v: &Value) -> Vec<char> {
    v.as_array().into_iter().flatten().filter_map(|t| t.as_str()?.chars().last()).collect()
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
            "workspace": {"configuration": true, "workspaceFolders": true, "applyEdit": true, "workspaceEdit": {"documentChanges": true}},
            "textDocument": {
                "synchronization": {"didSave": true},
                "hover": {"contentFormat": ["markdown", "plaintext"]},
                "completion": {"completionItem": {"snippetSupport": false, "documentationFormat": ["plaintext", "markdown"]}, "contextSupport": true},
                "signatureHelp": {"signatureInformation": {"documentationFormat": ["plaintext", "markdown"], "parameterInformation": {"labelOffsetSupport": true}, "activeParameterSupport": true}},
                "definition": {"linkSupport": true},
                "formatting": {},
                "rangeFormatting": {},
                "rename": {"prepareSupport": false},
                "references": {},
                "codeAction": {"codeActionLiteralSupport": {"codeActionKind": {"valueSet": ["", "quickfix", "refactor", "refactor.extract", "refactor.inline", "refactor.rewrite", "source", "source.organizeImports", "source.fixAll"]}}, "isPreferredSupport": true},
                "publishDiagnostics": {"relatedInformation": false, "tagSupport": {"valueSet": [1, 2]}}
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

/// `textDocument/rangeFormatting` params.
pub fn range_formatting(path: &Path, range: Range, tab_size: u32, spaces: bool) -> Value {
    json!({"textDocument": {"uri": path_to_uri(path)}, "range": {"start": position_json(range.start), "end": position_json(range.end)}, "options": {"tabSize": tab_size, "insertSpaces": spaces}})
}

pub fn rename(path: &Path, p: Position, new_name: &str) -> Value {
    json!({"textDocument": {"uri": path_to_uri(path)}, "position": position_json(p), "newName": new_name})
}

pub fn references(path: &Path, p: Position) -> Value {
    json!({"textDocument": {"uri": path_to_uri(path)}, "position": position_json(p), "context": {"includeDeclaration": true}})
}

/// `textDocument/codeAction` params: the range and the diagnostics on it, as the server sent them.
pub fn code_action(path: &Path, range: Range, diagnostics: Vec<Value>) -> Value {
    json!({"textDocument": {"uri": path_to_uri(path)}, "range": {"start": position_json(range.start), "end": position_json(range.end)}, "context": {"diagnostics": diagnostics}})
}

/// `textDocument/formatting` params: the whole file.
pub fn formatting(path: &Path, tab_size: u32, spaces: bool) -> Value {
    json!({"textDocument": {"uri": path_to_uri(path)}, "options": {"tabSize": tab_size, "insertSpaces": spaces}})
}

/// The reply to a request the server sends us. Servers stall if these go unanswered
/// (spike 7): configuration gets each item's section of `settings` (dotted, like
/// `python.analysis`; `null` when absent), everything else an empty success.
pub fn answer_server_request(message: &Value, settings: &Value) -> Option<Value> {
    let id = message.get("id")?;
    let method = message.get("method")?.as_str()?;
    let section = |item: &Value| item["section"].as_str().map_or(Value::Null, |s| s.split('.').fold(settings, |v, key| &v[key]).clone());
    let result = match method {
        "workspace/configuration" => Value::Array(message["params"]["items"].as_array().map_or_else(Vec::new, |items| items.iter().map(section).collect())),
        // Applied on the main thread (the message is delivered as well); acknowledged here.
        "workspace/applyEdit" => json!({"applied": true}),
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
    /// The rule ("reportUnusedExpression", "F401", …).
    pub code: Option<String>,
    /// Tagged unnecessary (unused code): shown faded, not marked.
    pub unnecessary: bool,
    /// As the server sent it, for code action requests (Ruff's fixes need its `data`).
    pub raw: Value,
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
                code: d["code"].as_str().map(String::from).or_else(|| d["code"].as_i64().map(|c| c.to_string())),
                unnecessary: d["tags"].as_array().is_some_and(|t| t.iter().any(|t| t == 1)),
                raw: d.clone(),
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

/// Hover contents as blocks: `(true, code)` for fenced code (signatures), `(false,
/// prose)` for the rest, with rules, heading marks and backticks dropped.
pub fn hover_blocks(result: &Value) -> Vec<(bool, String)> {
    fn part(v: &Value) -> Vec<(bool, String)> {
        match v {
            Value::String(s) => vec![(false, s.clone())],
            Value::Array(items) => items.iter().flat_map(part).collect(),
            // MarkedString `{language, value}` is code; MarkupContent `{kind, value}` is markdown.
            Value::Object(_) if v.get("language").is_some() => vec![(true, v["value"].as_str().unwrap_or("").to_string())],
            Value::Object(_) => vec![(false, v["value"].as_str().unwrap_or("").to_string())],
            _ => Vec::new(),
        }
    }
    let mut blocks: Vec<(bool, String)> = Vec::new();
    for (code, raw) in part(&result["contents"]) {
        if code {
            blocks.push((true, raw.trim_end().to_string()));
            continue;
        }
        let mut fenced = false;
        let mut current = String::new();
        let flush = |blocks: &mut Vec<(bool, String)>, current: &mut String, code: bool| {
            let text = std::mem::take(current);
            let text = if code { text.trim_end().to_string() } else { text.trim().to_string() };
            if !text.is_empty() {
                blocks.push((code, text));
            }
        };
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("```") {
                flush(&mut blocks, &mut current, fenced);
                fenced = !fenced;
                continue;
            }
            if fenced {
                current.push_str(line);
                current.push('\n');
                continue;
            }
            if !trimmed.is_empty() && trimmed.len() >= 3 && trimmed.chars().all(|c| c == '-' || c == '*' || c == '_') {
                flush(&mut blocks, &mut current, false);
                continue;
            }
            let line = if trimmed.starts_with('#') { trimmed.trim_start_matches('#').trim_start() } else { line };
            current.push_str(&line.replace('`', ""));
            current.push('\n');
        }
        flush(&mut blocks, &mut current, fenced);
    }
    for (code, text) in &mut blocks {
        if !*code {
            while text.contains("\n\n\n") {
                *text = text.replace("\n\n\n", "\n\n");
            }
        }
    }
    blocks
}

/// The active signature: its label, the active parameter as a char range in it, and
/// the documentation (the parameter's, then the signature's).
#[derive(Debug, Clone, PartialEq)]
pub struct Signature {
    pub label: String,
    pub active: Option<(usize, usize)>,
    pub doc: Option<String>,
}

pub fn signature_help(result: &Value) -> Option<Signature> {
    let signatures = result["signatures"].as_array()?;
    let index = result["activeSignature"].as_u64().unwrap_or(0) as usize;
    let sig = signatures.get(index).or(signatures.first())?;
    let label = sig["label"].as_str()?.to_string();
    let param_index = sig["activeParameter"].as_u64().or(result["activeParameter"].as_u64());
    let param = param_index.and_then(|i| sig["parameters"].as_array()?.get(i as usize));
    let active = param.and_then(|p| match &p["label"] {
        Value::String(name) => {
            // The first whole-word match after the opening parenthesis.
            let open = label.find('(').map_or(0, |i| i + 1);
            let at = label[open..].match_indices(name.as_str()).map(|(i, _)| i + open).find(|&i| {
                let before = label[..i].chars().last();
                let after = label[i + name.len()..].chars().next();
                !before.is_some_and(|c| c.is_alphanumeric() || c == '_') && !after.is_some_and(|c| c.is_alphanumeric() || c == '_')
            })?;
            let start = label[..at].chars().count();
            Some((start, start + name.chars().count()))
        }
        Value::Array(pair) => {
            // UTF-16 offsets into the label.
            let (a, b) = (pair.first()?.as_u64()? as u32, pair.get(1)?.as_u64()? as u32);
            Some((crate::text::from_lsp(&label, a, Encoding::Utf16), crate::text::from_lsp(&label, b, Encoding::Utf16)))
        }
        _ => None,
    });
    let doc_of = |v: &Value| -> Option<String> {
        let text = v.as_str().or(v["value"].as_str())?.trim();
        // The first paragraph is enough under a signature.
        let first = text.split("\n\n").next()?.trim();
        (!first.is_empty()).then(|| first.replace('`', ""))
    };
    let doc = param.and_then(|p| doc_of(&p["documentation"])).or_else(|| doc_of(&sig["documentation"]));
    Some(Signature { label, active, doc })
}

/// A `WorkspaceEdit` (`changes` or `documentChanges`) → each file's text edits.
/// File creates, renames and deletes are skipped.
pub fn workspace_edit(edit: &Value) -> Vec<(PathBuf, Vec<(Range, String)>)> {
    let mut out: Vec<(PathBuf, Vec<(Range, String)>)> = Vec::new();
    if let Some(changes) = edit["changes"].as_object() {
        for (uri, edits) in changes {
            if let Some(path) = uri_to_path(uri) {
                out.push((path, text_edits(edits)));
            }
        }
    }
    for change in edit["documentChanges"].as_array().into_iter().flatten() {
        if let Some(path) = change["textDocument"]["uri"].as_str().and_then(uri_to_path) {
            out.push((path, text_edits(&change["edits"])));
        }
    }
    out
}

/// `Location[]` / `LocationLink[]` (references) → every one, in order.
pub fn locations(result: &Value) -> Vec<Location> {
    result.as_array().into_iter().flatten().filter_map(definition).collect()
}

/// A code action: a quick fix or refactor (`CodeAction`), or a bare `Command`.
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub title: String,
    pub preferred: bool,
    pub edit: Option<Value>,
    /// `{command, arguments}`, sent back as `workspace/executeCommand`.
    pub command: Option<Value>,
}

pub fn code_actions(result: &Value) -> Vec<Action> {
    result
        .as_array()
        .into_iter()
        .flatten()
        .filter(|a| a["disabled"].is_null())
        .filter_map(|a| {
            let title = a["title"].as_str()?.to_string();
            // A bare Command has `command` as a string; a CodeAction nests it.
            let command = if a["command"].is_string() { Some(json!({"command": a["command"], "arguments": a["arguments"]})) } else { a.get("command").filter(|c| c.is_object()).cloned() };
            let edit = a.get("edit").filter(|e| e.is_object()).cloned();
            (edit.is_some() || command.is_some()).then(|| Action { title, preferred: a["isPreferred"].as_bool().unwrap_or(false), edit, command })
        })
        .collect()
}

/// `TextEdit[]` (formatting) → (range, new text), in the order given.
pub fn text_edits(result: &Value) -> Vec<(Range, String)> {
    result.as_array().into_iter().flatten().filter_map(|e| Some((range(&e["range"])?, e["newText"].as_str()?.to_string()))).collect()
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
    /// What it is ("function", "class", "module", …), shown beside it.
    pub kind: Option<&'static str>,
}

/// LSP `CompletionItemKind` → a short word.
fn kind_name(kind: u64) -> Option<&'static str> {
    Some(match kind {
        2 => "method",
        3 => "function",
        4 => "constructor",
        5 => "field",
        6 => "variable",
        7 => "class",
        8 => "interface",
        9 => "module",
        10 => "property",
        12 => "value",
        13 => "enum",
        14 => "keyword",
        15 => "snippet",
        17 => "file",
        19 => "folder",
        20 => "member",
        21 => "constant",
        22 => "struct",
        25 => "type",
        _ => return None,
    })
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
                kind: item["kind"].as_u64().and_then(kind_name),
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
        assert_eq!(caps, Capabilities { encoding: Encoding::Utf8, hover: true, completion: true, definition: true, triggers: vec!['.', ':', '>'], signature_triggers: vec![], signature: false, formatting: false, range_formatting: false, rename: false, references: false, code_actions: false });
        let py = Capabilities::parse(&json!({"capabilities": {"signatureHelpProvider": {"triggerCharacters": ["(", ","], "retriggerCharacters": [")"]}, "documentFormattingProvider": true}}));
        assert!(py.signature && py.formatting);
        assert_eq!(py.signature_triggers, vec!['(', ',', ')']);
        let none = Capabilities::parse(&json!({"capabilities": {}}));
        assert!(!none.hover && !none.completion && !none.definition && none.encoding == Encoding::Utf16);
    }

    #[test]
    fn server_requests_get_answers() {
        let config = json!({"jsonrpc": "2.0", "id": 4, "method": "workspace/configuration", "params": {"items": [{}, {}]}});
        let none = Value::Null;
        assert_eq!(answer_server_request(&config, &none).unwrap()["result"], json!([null, null]));
        let settings = json!({"python": {"pythonPath": "/v/bin/python", "analysis": {"typeCheckingMode": "basic"}}});
        let asked = json!({"jsonrpc": "2.0", "id": 5, "method": "workspace/configuration", "params": {"items": [{"section": "python"}, {"section": "python.analysis"}, {"section": "pyright"}]}});
        assert_eq!(answer_server_request(&asked, &settings).unwrap()["result"], json!([settings["python"], {"typeCheckingMode": "basic"}, null]));
        let register = json!({"jsonrpc": "2.0", "id": "a", "method": "client/registerCapability", "params": {}});
        assert_eq!(answer_server_request(&register, &none).unwrap(), json!({"jsonrpc": "2.0", "id": "a", "result": null}));
        assert!(answer_server_request(&json!({"jsonrpc": "2.0", "method": "window/logMessage"}), &none).is_none(), "notifications need none");
        assert!(answer_server_request(&json!({"jsonrpc": "2.0", "id": 1, "result": {}}), &none).is_none(), "responses need none");
    }

    #[test]
    fn diagnostics_parse_with_default_severity() {
        let params = json!({"uri": "file:///a/b.c", "diagnostics": [
            {"range": {"start": {"line": 3, "character": 1}, "end": {"line": 3, "character": 5}}, "message": "boom", "source": "clang"},
            {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "severity": 2, "message": "hm"},
            {"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 1}}, "severity": 4, "message": "x is not accessed", "code": "reportUnusedVariable", "tags": [1]},
            {"message": "no range is skipped"}
        ]});
        let (path, list) = diagnostics(&params).unwrap();
        assert_eq!(path, Path::new("/a/b.c"));
        assert_eq!(list.len(), 3);
        assert_eq!((list[0].severity, list[0].source.as_deref(), list[0].range.start.line), (Severity::Error, Some("clang"), 3));
        assert_eq!(list[1].severity, Severity::Warning);
        assert!(!list[1].unnecessary && list[2].unnecessary);
        assert_eq!(list[2].code.as_deref(), Some("reportUnusedVariable"));
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
    fn hover_blocks_split_code_from_prose() {
        let pyright = json!({"contents": {"kind": "markdown", "value": "```python\nclass DataFrame(\n    data=None,\n    index=None\n)\n```\n---\nTwo-dimensional, `size`-mutable data."}});
        assert_eq!(hover_blocks(&pyright), vec![(true, "class DataFrame(\n    data=None,\n    index=None\n)".to_string()), (false, "Two-dimensional, size-mutable data.".to_string())]);
        assert_eq!(hover_blocks(&json!({"contents": ["a", {"language": "c", "value": "int x"}]})), vec![(false, "a".to_string()), (true, "int x".to_string())]);
        assert!(hover_blocks(&Value::Null).is_empty());
    }

    #[test]
    fn signature_help_finds_the_active_parameter() {
        let by_name = json!({"signatures": [{"label": "DataFrame(data=None, index=None)", "parameters": [{"label": "data"}, {"label": "index", "documentation": "Row labels.\n\nMore."}]}], "activeSignature": 0, "activeParameter": 1});
        let sig = signature_help(&by_name).unwrap();
        assert_eq!((sig.label.as_str(), sig.active, sig.doc.as_deref()), ("DataFrame(data=None, index=None)", Some((21, 26)), Some("Row labels.")));
        let by_offset = json!({"signatures": [{"label": "f(a, b)", "parameters": [{"label": [2, 3]}, {"label": [5, 6]}], "activeParameter": 0, "documentation": {"kind": "markdown", "value": "Does `f`."}}]});
        let sig = signature_help(&by_offset).unwrap();
        assert_eq!((sig.active, sig.doc.as_deref()), (Some((2, 3)), Some("Does f.")));
        assert_eq!(signature_help(&json!({"signatures": []})), None);
        assert_eq!(signature_help(&Value::Null), None);
    }

    #[test]
    fn workspace_edits_references_and_actions_parse() {
        let at = |l| json!({"start": {"line": l, "character": 0}, "end": {"line": l, "character": 1}});
        let edit = json!({"changes": {"file:///a.py": [{"range": at(1), "newText": "y"}]}, "documentChanges": [{"textDocument": {"uri": "file:///b.py", "version": 3}, "edits": [{"range": at(2), "newText": "z"}]}, {"kind": "rename", "oldUri": "file:///c", "newUri": "file:///d"}]});
        let files = workspace_edit(&edit);
        assert_eq!(files.iter().map(|(p, e)| (p.to_string_lossy().into_owned(), e.len())).collect::<Vec<_>>(), vec![("/a.py".to_string(), 1), ("/b.py".to_string(), 1)]);
        let refs = locations(&json!([{"uri": "file:///a.py", "range": at(4)}, {"uri": "file:///b.py", "range": at(7)}]));
        assert_eq!(refs.iter().map(|l| l.start.line).collect::<Vec<_>>(), vec![4, 7]);
        let actions = code_actions(&json!([
            {"title": "Remove unused import", "kind": "quickfix", "isPreferred": true, "edit": {"changes": {}}},
            {"title": "Organize imports", "command": "pyright.organizeimports", "arguments": ["file:///a.py"]},
            {"title": "Disabled", "edit": {}, "disabled": {"reason": "no"}},
            {"title": "Nothing to do"}
        ]));
        assert_eq!(actions.len(), 2);
        assert!(actions[0].preferred && actions[0].edit.is_some());
        assert_eq!(actions[1].command.as_ref().unwrap()["command"], "pyright.organizeimports");
        let caps = Capabilities::parse(&json!({"capabilities": {"renameProvider": {"prepareProvider": true}, "referencesProvider": true, "codeActionProvider": {}, "documentRangeFormattingProvider": true}}));
        assert!(caps.rename && caps.references && caps.code_actions && caps.range_formatting);
    }

    #[test]
    fn formatting_edits_parse() {
        let edits = text_edits(&json!([{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 2, "character": 0}}, "newText": "x = 1\n"}]));
        assert_eq!(edits.len(), 1);
        assert_eq!((edits[0].0.end.line, edits[0].1.as_str()), (2, "x = 1\n"));
        assert!(text_edits(&Value::Null).is_empty());
    }

    #[test]
    fn completions_take_edits_or_insert_text() {
        let result = json!({"isIncomplete": true, "items": [
            {"label": " ratio", "detail": "double", "textEdit": {"newText": "ratio", "range": {"start": {"line": 1, "character": 6}, "end": {"line": 1, "character": 7}}}},
            {"label": "id", "insertText": "id", "sortText": "0", "kind": 7},
            {"label": "push", "textEdit": {"newText": "push", "insert": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "replace": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 3}}}}
        ]});
        let (items, incomplete) = completions(&result);
        assert!(incomplete);
        assert_eq!((items[0].label.as_str(), items[0].detail.as_deref(), items[0].replace.unwrap().start.column), ("ratio", Some("double"), 6));
        assert_eq!((items[1].insert.as_str(), items[1].replace, items[1].sort.as_str(), items[1].kind), ("id", None, "0", Some("class")));
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
