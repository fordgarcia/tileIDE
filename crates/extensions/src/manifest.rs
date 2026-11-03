// SPDX-License-Identifier: GPL-3.0-or-later
//! `extension.json`: what an extension is, how it starts and what it offers.
//!
//! ```json
//! {
//!   "id": "word-count",
//!   "name": "Word count",
//!   "version": "0.1.0",
//!   "description": "Counts the words of the focused file.",
//!   "run": ["python3", "./main.py"],
//!   "activation": ["onCommand:word-count.count"],
//!   "commands": [{ "id": "word-count.count", "title": "Count words" }]
//! }
//! ```

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activation {
    /// `onStartup` or `*`: starts with TileIDE (the weight meter warns about it).
    Startup,
    /// `onLanguage:<id>`: a file of this GtkSourceView language opens.
    Language(String),
    /// `onCommand:<id>`: one of its commands is run.
    Command(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    /// Program and arguments; a program starting with `./` is inside the extension's folder.
    pub run: Vec<String>,
    pub activation: Vec<Activation>,
    pub commands: Vec<Command>,
}

impl Manifest {
    pub fn starts_always(&self) -> bool {
        self.activation.contains(&Activation::Startup)
    }
}

fn string(v: &Value, key: &str) -> Result<Option<String>, String> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("\"{key}\" must be a string")),
    }
}

fn strings(v: &Value, key: &str) -> Result<Vec<String>, String> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items.iter().map(|i| i.as_str().map(str::to_owned).ok_or_else(|| format!("\"{key}\" must hold strings"))).collect(),
        Some(_) => Err(format!("\"{key}\" must be a list of strings")),
    }
}

/// Lowercase letters, digits, `-`, `_` and `.`: it names the install folder.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && !id.starts_with('.') && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
}

pub fn parse(text: &str) -> Result<Manifest, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
    if !v.is_object() {
        return Err("must be a JSON object".into());
    }
    let id = string(&v, "id")?.ok_or("\"id\" is missing")?;
    if !valid_id(&id) {
        return Err(format!("\"id\" \"{id}\" may only use a-z, 0-9, -, _ and ."));
    }
    let run = strings(&v, "run")?;
    if run.is_empty() {
        return Err("\"run\" is missing (program and arguments)".into());
    }
    let mut activation = Vec::new();
    for event in strings(&v, "activation")? {
        activation.push(match event.split_once(':') {
            _ if event == "*" || event == "onStartup" => Activation::Startup,
            Some(("onLanguage", lang)) if !lang.is_empty() => Activation::Language(lang.into()),
            Some(("onCommand", command)) if !command.is_empty() => Activation::Command(command.into()),
            _ => return Err(format!("unknown activation event \"{event}\" (onStartup, onLanguage:<id>, onCommand:<id>)")),
        });
    }
    let mut commands = Vec::new();
    if let Some(list) = v.get("commands") {
        let list = list.as_array().ok_or("\"commands\" must be a list")?;
        for c in list {
            let id = string(c, "id")?.ok_or("a command has no \"id\"")?;
            let title = string(c, "title")?.unwrap_or_else(|| id.clone());
            commands.push(Command { id, title });
        }
    }
    Ok(Manifest {
        name: string(&v, "name")?.unwrap_or_else(|| id.clone()),
        version: string(&v, "version")?.unwrap_or_default(),
        description: string(&v, "description")?.unwrap_or_default(),
        id,
        run,
        activation,
        commands,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{ "id": "word-count", "name": "Word count", "run": ["python3", "./main.py"],
        "activation": ["onCommand:word-count.count", "onLanguage:rust", "*"],
        "commands": [{ "id": "word-count.count", "title": "Count words" }] }"#;

    #[test]
    fn reads_a_full_manifest() {
        let m = parse(SAMPLE).unwrap();
        assert_eq!(m.id, "word-count");
        assert_eq!(m.run, ["python3", "./main.py"]);
        assert_eq!(m.activation, [Activation::Command("word-count.count".into()), Activation::Language("rust".into()), Activation::Startup]);
        assert_eq!(m.commands[0].title, "Count words");
        assert!(m.starts_always());
        assert_eq!(m.version, "");
    }

    #[test]
    fn rejects_bad_manifests() {
        assert!(parse("[]").is_err());
        assert!(parse(r#"{ "run": ["x"] }"#).unwrap_err().contains("id"));
        assert!(parse(r#"{ "id": "../evil", "run": ["x"] }"#).unwrap_err().contains("may only use"));
        assert!(parse(r#"{ "id": "a" }"#).unwrap_err().contains("run"));
        assert!(parse(r#"{ "id": "a", "run": ["x"], "activation": ["onSave"] }"#).unwrap_err().contains("onSave"));
    }
}
