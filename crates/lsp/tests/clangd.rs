// SPDX-License-Identifier: GPL-3.0-or-later
//! The whole display-free path against a real server: spawn, initialize, open a
//! buffer with an error that exists only in memory, then diagnostics, hover,
//! definition, completion and shutdown. Skipped when clangd isn't installed.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use tileide_lsp::client::{Connection, Incoming};
use tileide_lsp::protocol::{self, Capabilities, Position};
use tileide_lsp::servers;
use tileide_lsp::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(20);

fn wait(rx: &mpsc::Receiver<Incoming>, what: &str, wanted: impl Fn(&Value) -> bool) -> Value {
    loop {
        match rx.recv_timeout(TIMEOUT) {
            Ok(Incoming::Message(m)) if wanted(&m) => return m,
            Ok(Incoming::Message(_)) => {}
            Ok(Incoming::Exited) => panic!("server exited waiting for {what}"),
            Err(_) => panic!("timed out waiting for {what}"),
        }
    }
}

#[test]
fn clangd_end_to_end() {
    let server = servers::by_name("c").unwrap();
    let Some(cmd) = servers::command(server, None, |p| servers::locate(p, &[], &[])) else {
        eprintln!("clangd not installed: skipped");
        return;
    };
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture");
    let file = fixture.join("hello.c");
    let text = std::fs::read_to_string(&file).unwrap().replacen("    return 0;", "    int b = undefined_name;\n    return 0;", 1);

    let (tx, rx) = mpsc::channel();
    let mut c = Connection::spawn(&cmd, &fixture, serde_json::Value::Null, move |m| {
        let _ = tx.send(m);
    })
    .unwrap();
    let id = c.request("initialize", protocol::initialize_params(&fixture, serde_json::Value::Null)).unwrap();
    let init = wait(&rx, "initialize", |m| m["id"] == id);
    let caps = Capabilities::parse(&init["result"]);
    assert!(caps.hover && caps.completion && caps.definition, "{caps:?}");
    assert!(caps.triggers.contains(&'.'));
    c.send(&protocol::notification("initialized", json!({}))).unwrap();

    c.send(&protocol::did_open(&file, "c", 1, &text)).unwrap();
    let published = wait(&rx, "diagnostics", |m| m["method"] == "textDocument/publishDiagnostics" && m["params"]["diagnostics"].as_array().is_some_and(|d| !d.is_empty()));
    let (path, list) = protocol::diagnostics(&published["params"]).unwrap();
    assert_eq!(path, file);
    let error = list.iter().find(|d| d.message.contains("undefined_name")).expect("the in-memory error is reported");
    assert_eq!(error.range.start.line, 15);

    let call = Position { line: 13, column: 12 }; // `area(40, 25)`
    let id = c.request("textDocument/hover", protocol::at(&file, call)).unwrap();
    let hover = protocol::hover_text(&wait(&rx, "hover", |m| m["id"] == id)["result"]).unwrap();
    assert!(hover.contains("area"), "{hover}");

    let id = c.request("textDocument/definition", protocol::at(&file, call)).unwrap();
    let location = protocol::definition(&wait(&rx, "definition", |m| m["id"] == id)["result"]).unwrap();
    assert_eq!((location.path.as_path(), location.start.line), (file.as_path(), 7));

    let id = c.request("textDocument/completion", protocol::at(&file, Position { line: 14, column: 45 })).unwrap(); // after `t.`
    let (items, _) = protocol::completions(&wait(&rx, "completion", |m| m["id"] == id)["result"]);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.iter().any(|l| l.starts_with("id")) && labels.iter().any(|l| l.starts_with("ratio")), "{labels:?}");

    c.shutdown(Duration::from_secs(5));
}
