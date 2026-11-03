// SPDX-License-Identifier: GPL-3.0-or-later
//! Jupyter notebooks (v0.4 phase 4) without a display: the nbformat 4 model (`.ipynb`
//! read and written so notebooks round-trip with Jupyter and VS Code; unknown fields
//! are kept) and the kernel bridge (`kernel`).

pub mod kernel;
pub mod latex;
pub mod markdown;
pub mod table;

use serde_json::{Map, Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    Code,
    Markdown,
    Raw,
}

impl CellKind {
    fn key(self) -> &'static str {
        match self {
            CellKind::Code => "code",
            CellKind::Markdown => "markdown",
            CellKind::Raw => "raw",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub kind: CellKind,
    pub source: String,
    /// nbformat outputs, kept as JSON (code cells only).
    pub outputs: Vec<Value>,
    pub count: Option<i64>,
    /// The cell as read: its id, metadata and anything else, written back unchanged.
    raw: Map<String, Value>,
}

impl Cell {
    pub fn new(kind: CellKind) -> Cell {
        let mut raw = Map::new();
        raw.insert("id".into(), Value::String(new_id()));
        raw.insert("metadata".into(), json!({}));
        Cell { kind, source: String::new(), outputs: Vec::new(), count: None, raw }
    }

    /// A copy with its own id (pasting a cell next to the one it came from).
    pub fn duplicate(&self) -> Cell {
        let mut copy = self.clone();
        copy.raw.insert("id".into(), Value::String(new_id()));
        copy
    }

    pub fn id(&self) -> Option<&str> {
        self.raw.get("id")?.as_str()
    }

    /// Same content as another kind (the cell keeps its id).
    pub fn set_kind(&mut self, kind: CellKind) {
        self.kind = kind;
        if kind != CellKind::Code {
            self.outputs.clear();
            self.count = None;
        }
    }
}

/// An nbformat 4.5 cell id: 8 lowercase hex digits, unique enough within a notebook.
fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    format!("{:08x}", (nanos ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15)) as u32)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Notebook {
    pub cells: Vec<Cell>,
    /// Everything but the cells (metadata, nbformat version), written back unchanged.
    raw: Map<String, Value>,
}

/// nbformat text: a string or a list of lines.
fn text(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(lines)) => lines.iter().filter_map(Value::as_str).collect(),
        _ => String::new(),
    }
}

/// The list-of-lines form Jupyter writes (each line keeps its `\n`).
fn lines(s: &str) -> Value {
    Value::Array(s.split_inclusive('\n').map(|l| Value::String(l.into())).collect())
}

impl Notebook {
    /// An empty Python notebook with one code cell.
    pub fn new() -> Notebook {
        let raw = json!({
            "metadata": {
                "kernelspec": { "display_name": "Python 3", "language": "python", "name": "python3" },
                "language_info": { "name": "python" }
            },
            "nbformat": 4,
            "nbformat_minor": 5
        });
        Notebook { cells: vec![Cell::new(CellKind::Code)], raw: raw.as_object().cloned().unwrap_or_default() }
    }

    /// Reads `.ipynb` text; an empty file is a new notebook.
    pub fn parse(text_in: &str) -> Result<Notebook, String> {
        if text_in.trim().is_empty() {
            return Ok(Notebook::new());
        }
        let Value::Object(mut raw) = serde_json::from_str(text_in).map_err(|e| format!("not a notebook (invalid JSON: {e})"))? else {
            return Err("not a notebook (expected a JSON object)".into());
        };
        let major = raw.get("nbformat").and_then(Value::as_i64).unwrap_or(0);
        if major != 4 {
            return Err(format!("nbformat {major} isn't supported (only 4); convert it with: jupyter nbconvert --to notebook --nbformat 4"));
        }
        let cells = match raw.remove("cells") {
            Some(Value::Array(cells)) => cells,
            _ => Vec::new(),
        };
        let cells = cells
            .into_iter()
            .filter_map(|c| match c {
                Value::Object(mut raw) => {
                    let kind = match raw.get("cell_type").and_then(Value::as_str) {
                        Some("markdown") => CellKind::Markdown,
                        Some("raw") => CellKind::Raw,
                        _ => CellKind::Code,
                    };
                    let source = text(raw.remove("source").as_ref());
                    let outputs = match raw.remove("outputs") {
                        Some(Value::Array(o)) => o,
                        _ => Vec::new(),
                    };
                    let count = raw.remove("execution_count").and_then(|v| v.as_i64());
                    raw.remove("cell_type");
                    Some(Cell { kind, source, outputs, count, raw })
                }
                _ => None,
            })
            .collect();
        Ok(Notebook { cells, raw })
    }

    /// The kernel the notebook asks for (`metadata.kernelspec.name`), else `python3`.
    pub fn kernel(&self) -> String {
        self.raw.get("metadata").and_then(|m| m["kernelspec"]["name"].as_str()).unwrap_or("python3").to_string()
    }

    /// Asks for another kernel (`metadata.kernelspec`); the cells' language follows.
    pub fn set_kernel(&mut self, name: &str, display: &str, language: &str) {
        if !self.raw.get("metadata").is_some_and(Value::is_object) {
            self.raw.insert("metadata".into(), Value::Object(Map::new()));
        }
        if let Some(Value::Object(meta)) = self.raw.get_mut("metadata") {
            meta.insert("kernelspec".into(), serde_json::json!({ "name": name, "display_name": display, "language": language }));
            // Written by the kernel on its next run; a stale one would name the old language.
            meta.remove("language_info");
        }
    }

    /// The GtkSourceView language for its code cells.
    pub fn language(&self) -> String {
        let lang = self.raw.get("metadata").and_then(|m| m["kernelspec"]["language"].as_str().or_else(|| m["language_info"]["name"].as_str())).unwrap_or("python");
        match lang {
            "python" => "python3".into(),
            other => other.to_lowercase(),
        }
    }

    /// nbformat 4 JSON, indented with one space like Jupyter, ending in a newline.
    pub fn to_json(&self) -> String {
        let cells: Vec<Value> = self
            .cells
            .iter()
            .map(|c| {
                let mut cell = Map::new();
                cell.insert("cell_type".into(), Value::String(c.kind.key().into()));
                for (k, v) in &c.raw {
                    cell.insert(k.clone(), v.clone());
                }
                cell.entry("metadata").or_insert_with(|| json!({}));
                if c.kind == CellKind::Code {
                    cell.insert("execution_count".into(), c.count.map_or(Value::Null, Value::from));
                    cell.insert("outputs".into(), Value::Array(c.outputs.clone()));
                } else {
                    cell.remove("execution_count");
                    cell.remove("outputs");
                }
                cell.insert("source".into(), lines(&c.source));
                Value::Object(cell)
            })
            .collect();
        // Cells first, as Jupyter writes them.
        let mut out = Map::new();
        out.insert("cells".into(), Value::Array(cells));
        for (k, v) in &self.raw {
            out.insert(k.clone(), v.clone());
        }
        // Jupyter's layout: one-space indents.
        let mut buf = Vec::new();
        let formatter = serde_json::ser::PrettyFormatter::with_indent(b" ");
        let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
        if serde::Serialize::serialize(&Value::Object(out), &mut ser).is_err() {
            return String::new();
        }
        String::from_utf8(buf).unwrap_or_default() + "\n"
    }
}

impl Default for Notebook {
    fn default() -> Self {
        Notebook::new()
    }
}

/// A code cell as plain Python for language servers, line for line: IPython syntax
/// (`%magic`, `!shell`, `obj?`) becomes a blank line, or `pass` when indented (a block
/// can't be empty), and a `%%` cell magic blanks the whole cell. The line count never
/// changes, so positions map back.
pub fn python_view(source: &str) -> String {
    let cell_magic = source.lines().find(|l| !l.trim().is_empty()).is_some_and(|l| l.trim_start().starts_with("%%"));
    source
        .split('\n')
        .map(|line| {
            let trimmed = line.trim_start();
            let ipython = trimmed.starts_with('%') || trimmed.starts_with('!') || (trimmed.ends_with('?') && !trimmed.starts_with('#'));
            if cell_magic || trimmed.is_empty() || (ipython && trimmed.len() == line.len()) {
                String::new()
            } else if ipython {
                format!("{}pass", &line[..line.len() - trimmed.len()])
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Adds a kernel output to a cell's outputs: consecutive stream text of the same
/// stream joins the last output, as Jupyter shows it.
pub fn push_output(outputs: &mut Vec<Value>, output: Value) {
    if output["output_type"] == "stream"
        && let Some(last) = outputs.last_mut()
        && last["output_type"] == "stream"
        && last["name"] == output["name"]
    {
        let joined = text(last.get("text")) + &text(output.get("text"));
        last["text"] = Value::String(carriage(&joined));
        return;
    }
    outputs.push(output);
}

/// `\r` as a terminal treats it: what follows replaces the line (a progress bar
/// redrawn in place keeps only its latest state). A line ending in `\r` keeps it, so
/// the next piece of a stream still replaces that line.
pub fn carriage(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    if !text.contains('\r') {
        return text;
    }
    text.split('\n')
        .map(|line| {
            let last = line.rsplit('\r').find(|s| !s.is_empty()).unwrap_or("");
            if line.ends_with('\r') { format!("{last}\r") } else { last.to_string() }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A place a traceback line points at.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// A file and its 1-based line (`~` expanded by the caller).
    File(String, u32),
    /// A notebook cell by its execution count, and the 1-based line in it.
    Cell(i64, u32),
}

/// The frame a traceback `line` names, and the byte span to make clickable. Knows
/// IPython's `File ~/a.py:12, in f()` and `Cell In[3], line 5`, and Python's
/// `File "/a.py", line 12, in f`.
pub fn frame(line: &str) -> Option<(std::ops::Range<usize>, Frame)> {
    let start = line.find("Cell In[").or_else(|| line.find("File "))?;
    let rest = &line[start..];
    if let Some(after) = rest.strip_prefix("Cell In[") {
        let count: i64 = after[..after.find(']')?].parse().ok()?;
        let tail = &after[after.find("], line ")? + "], line ".len()..];
        let digits = tail.chars().take_while(char::is_ascii_digit).count();
        let n: u32 = tail[..digits].parse().ok()?;
        let end = line.len() - tail.len() + digits;
        return Some((start..end, Frame::Cell(count, n)));
    }
    let after = rest.strip_prefix("File ")?;
    if let Some(quoted) = after.strip_prefix('"') {
        let path = &quoted[..quoted.find('"')?];
        let tail = quoted[path.len() + 1..].strip_prefix(", line ")?;
        let digits = tail.chars().take_while(char::is_ascii_digit).count();
        let n: u32 = tail[..digits].parse().ok()?;
        let end = line.len() - tail.len() + digits;
        return Some((start..end, Frame::File(path.to_string(), n)));
    }
    // `File <path>:<line>, in …` (the path may hold `:`; the line is the last one).
    let spot = after.find(", in ").map_or(after.trim_end(), |e| &after[..e]);
    let (path, n) = spot.rsplit_once(':')?;
    let n: u32 = n.parse().ok()?;
    let end = start + "File ".len() + spot.len();
    Some((start..end, Frame::File(path.to_string(), n)))
}

/// What to show for one output.
#[derive(Debug, Clone, PartialEq)]
pub enum Shown {
    Text(String),
    Error(String),
    /// Image bytes (PNG, JPEG or SVG; GTK tells them apart).
    Image(Vec<u8>),
    /// An HTML table (a pandas DataFrame); the plain text comes along as a fallback.
    Table(table::Table, String),
}

/// The richest output TileIDE can show: an image, else plain text (rich HTML is not
/// rendered yet; its text form is shown).
pub fn shown(output: &Value) -> Option<Shown> {
    match output["output_type"].as_str()? {
        "stream" => Some(Shown::Text(tileide_core::run::strip_ansi(&carriage(&text(output.get("text")))).replace('\r', ""))),
        "error" => {
            let trace: Vec<&str> = output["traceback"].as_array().map(|t| t.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            let body = if trace.is_empty() { format!("{}: {}", text(output.get("ename")), text(output.get("evalue"))) } else { trace.join("\n") };
            Some(Shown::Error(tileide_core::run::strip_ansi(&body)))
        }
        "display_data" | "execute_result" => {
            let data = &output["data"];
            for kind in ["image/png", "image/jpeg"] {
                if let Some(bytes) = data.get(kind).map(|v| text(Some(v))).and_then(|b| base64(&b)) {
                    return Some(Shown::Image(bytes));
                }
            }
            // SVG is text in the notebook, not base64.
            if let Some(svg) = data.get("image/svg+xml").map(|v| text(Some(v))).filter(|s| !s.is_empty()) {
                return Some(Shown::Image(svg.into_bytes()));
            }
            let plain = data.get("text/plain").map(|v| tileide_core::run::strip_ansi(&text(Some(v))));
            if let Some(table) = data.get("text/html").and_then(|v| table::parse(&text(Some(v)))) {
                return Some(Shown::Table(table, plain.unwrap_or_default()));
            }
            if let Some(latex) = data.get("text/latex").map(|v| text(Some(v))).filter(|s| !s.trim().is_empty()) {
                return Some(Shown::Text(latex::to_unicode(&latex)));
            }
            plain.map(Shown::Text)
        }
        _ => None,
    }
}

/// Standard base64 (whitespace ignored); `None` if it isn't valid.
pub fn base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace()) {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_bars_keep_their_latest_state() {
        assert_eq!(carriage("a\r\nb"), "a\nb");
        assert_eq!(carriage("start\n 10%\r 50%\r"), "start\n 50%\r");
        let mut outputs = vec![json!({"output_type": "stream", "name": "stderr", "text": "\r 10%|#"})];
        push_output(&mut outputs, json!({"output_type": "stream", "name": "stderr", "text": "\r 90%|#####"}));
        push_output(&mut outputs, json!({"output_type": "stream", "name": "stderr", "text": "\r100%|######\ndone\n"}));
        assert_eq!(outputs[0]["text"], "100%|######\ndone\n");
        assert_eq!(shown(&json!({"output_type": "stream", "name": "stderr", "text": " 5%\r"})), Some(Shown::Text(" 5%".into())));
    }

    #[test]
    fn rich_outputs_pick_images_then_tables_then_latex() {
        let svg = json!({"output_type": "display_data", "data": {"image/svg+xml": ["<svg/>"], "text/plain": "<Figure>"}});
        assert_eq!(shown(&svg), Some(Shown::Image(b"<svg/>".to_vec())));
        let jpeg = json!({"output_type": "display_data", "data": {"image/jpeg": "/9j/", "text/plain": "x"}});
        assert_eq!(shown(&jpeg), Some(Shown::Image(vec![0xff, 0xd8, 0xff])));
        let sympy = json!({"output_type": "execute_result", "data": {"text/latex": "$\\displaystyle x^{2}$", "text/plain": "x**2"}});
        assert_eq!(shown(&sympy), Some(Shown::Text("x²".into())));
    }

    #[test]
    fn finds_traceback_frames() {
        let l = "File ~/lab/util.py:12, in load(path)";
        assert_eq!(frame(l), Some((0..21, Frame::File("~/lab/util.py".into(), 12))));
        let l = "Cell In[3], line 5, in f()";
        assert_eq!(frame(l), Some((0..18, Frame::Cell(3, 5))));
        let l = "  File \"/usr/lib/python3.14/json/__init__.py\", line 346, in loads";
        let (span, f) = frame(l).unwrap();
        assert_eq!((&l[span], f), ("File \"/usr/lib/python3.14/json/__init__.py\", line 346", Frame::File("/usr/lib/python3.14/json/__init__.py".into(), 346)));
        assert_eq!(frame("ValueError: bad file"), None);
    }

    #[test]
    fn duplicated_cells_get_their_own_id() {
        let mut cell = Cell::new(CellKind::Code);
        cell.source = "x = 1".into();
        let copy = cell.duplicate();
        assert_eq!(copy.source, "x = 1");
        assert_ne!(copy.id(), cell.id());
        assert_eq!(copy.id().map(str::len), Some(8));
    }

    #[test]
    fn cells_become_python_line_for_line() {
        let src = "%matplotlib inline\nimport pandas as pd\nfor x in y:\n    !echo hi\ndf?\n# why?\n";
        let py = python_view(src);
        assert_eq!(py, "\nimport pandas as pd\nfor x in y:\n    pass\n\n# why?\n");
        assert_eq!(py.split('\n').count(), src.split('\n').count());
        assert_eq!(python_view("\n%%bash\nls -l"), "\n\n");
        assert_eq!(python_view(""), "");
    }

    const SAMPLE: &str = r##"{
 "cells": [
  { "cell_type": "markdown", "id": "a1", "metadata": {"tags": ["x"]}, "source": ["# Title\n", "text"] },
  { "cell_type": "code", "execution_count": 3, "id": "b2", "metadata": {}, "outputs": [
     { "name": "stdout", "output_type": "stream", "text": ["hi\n"] }
   ], "source": "print('hi')" }
 ],
 "metadata": { "kernelspec": { "name": "python3", "language": "python", "display_name": "Python 3" }, "custom": 1 },
 "nbformat": 4, "nbformat_minor": 5
}"##;

    #[test]
    fn round_trips_and_keeps_unknown_fields() {
        let nb = Notebook::parse(SAMPLE).unwrap();
        assert_eq!(nb.cells.len(), 2);
        assert_eq!(nb.cells[0].source, "# Title\ntext");
        assert_eq!(nb.cells[1].count, Some(3));
        assert_eq!((nb.kernel().as_str(), nb.language().as_str()), ("python3", "python3"));
        let mut other = nb.clone();
        other.set_kernel("ir", "R", "R");
        assert_eq!((other.kernel().as_str(), other.language().as_str()), ("ir", "r"));
        assert_eq!(serde_json::from_str::<Value>(&other.to_json()).unwrap()["metadata"]["custom"], 1, "other metadata stays");
        let again = Notebook::parse(&nb.to_json()).unwrap();
        assert_eq!(again, nb);
        let json: Value = serde_json::from_str(&nb.to_json()).unwrap();
        assert_eq!(json["metadata"]["custom"], 1);
        assert_eq!(json["cells"][0]["metadata"]["tags"][0], "x");
        assert_eq!(json["cells"][0]["source"], json!(["# Title\n", "text"]));
        assert!(json["cells"][0].get("outputs").is_none(), "markdown cells have no outputs");
        assert!(nb.to_json().starts_with("{\n \"cells\""), "Jupyter's one-space indent");
    }

    #[test]
    fn empty_files_and_old_formats() {
        let nb = Notebook::parse("").unwrap();
        assert_eq!(nb.cells.len(), 1);
        assert_eq!(nb.cells[0].raw["id"].as_str().map(str::len), Some(8));
        assert!(Notebook::parse(r#"{"nbformat": 3, "worksheets": []}"#).unwrap_err().contains("nbformat 3"));
        assert!(Notebook::parse("[1]").is_err());
    }

    #[test]
    fn streams_join_and_outputs_show() {
        let mut outputs = Vec::new();
        push_output(&mut outputs, json!({"output_type": "stream", "name": "stdout", "text": "a"}));
        push_output(&mut outputs, json!({"output_type": "stream", "name": "stdout", "text": "b\n"}));
        push_output(&mut outputs, json!({"output_type": "stream", "name": "stderr", "text": "c"}));
        assert_eq!(outputs.len(), 2);
        assert_eq!(shown(&outputs[0]), Some(Shown::Text("ab\n".into())));
        let err = json!({"output_type": "error", "ename": "E", "evalue": "v", "traceback": ["\u{1b}[31mTrace\u{1b}[0m", "E: v"]});
        assert_eq!(shown(&err), Some(Shown::Error("Trace\nE: v".into())));
        let img = json!({"output_type": "display_data", "data": {"image/png": "iVBORw==\n", "text/plain": "<Figure>"}});
        assert_eq!(shown(&img), Some(Shown::Image(vec![0x89, b'P', b'N', b'G'])));
    }
}
