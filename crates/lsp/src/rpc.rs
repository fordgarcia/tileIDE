// SPDX-License-Identifier: GPL-3.0-or-later
//! `Content-Length` framed JSON (from spike 7). LSP and DAP share this framing;
//! only the message shapes differ.

use std::io::{self, BufRead};

use serde_json::Value;

pub fn encode(message: &Value) -> Vec<u8> {
    let body = message.to_string();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

/// One framed message; `Ok(None)` at a clean end of stream.
pub fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let length = length.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "header without Content-Length"))?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trips_several_messages() {
        let a = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"});
        let b = json!({"seq": 2, "type": "event", "event": "stopped", "body": {"reason": "breakpoint ü"}});
        let mut bytes = encode(&a);
        bytes.extend(encode(&b));
        let mut reader = io::Cursor::new(bytes);
        assert_eq!(read_message(&mut reader).unwrap(), Some(a));
        assert_eq!(read_message(&mut reader).unwrap(), Some(b), "length counts bytes, not chars");
        assert_eq!(read_message(&mut reader).unwrap(), None);
    }

    #[test]
    fn accepts_extra_headers_and_rejects_missing_length() {
        let mut ok = io::Cursor::new(b"Content-Type: application/vscode-jsonrpc\r\ncontent-length: 2\r\n\r\n{}".to_vec());
        assert_eq!(read_message(&mut ok).unwrap(), Some(json!({})));
        let mut bad = io::Cursor::new(b"X: 1\r\n\r\n{}".to_vec());
        assert!(read_message(&mut bad).is_err());
    }
}
