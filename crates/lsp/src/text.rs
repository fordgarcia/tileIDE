// SPDX-License-Identifier: GPL-3.0-or-later
//! Positions and file URIs. The editor counts columns in characters; LSP counts them
//! in UTF-16 code units unless the server agrees to UTF-8 or UTF-32 (spike 7 finding).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Encoding {
    Utf8,
    Utf16,
    Utf32,
}

impl Encoding {
    /// The server's `capabilities.positionEncoding`; absent means UTF-16.
    pub fn parse(name: Option<&str>) -> Encoding {
        match name {
            Some("utf-8") => Encoding::Utf8,
            Some("utf-32") => Encoding::Utf32,
            _ => Encoding::Utf16,
        }
    }

    fn width(self, c: char) -> usize {
        match self {
            Encoding::Utf8 => c.len_utf8(),
            Encoding::Utf16 => c.len_utf16(),
            Encoding::Utf32 => 1,
        }
    }
}

/// Character column `chars` in `line` → LSP column.
pub fn to_lsp(line: &str, chars: usize, encoding: Encoding) -> u32 {
    line.chars().take(chars).map(|c| encoding.width(c)).sum::<usize>() as u32
}

/// LSP column → character column in `line`, clamped to its end. A column inside a
/// character (half a surrogate pair) lands on that character.
pub fn from_lsp(line: &str, column: u32, encoding: Encoding) -> usize {
    let mut units = 0;
    for (i, c) in line.chars().enumerate() {
        if units >= column as usize {
            return i;
        }
        units += encoding.width(c);
    }
    line.chars().count()
}

/// Percent-encodes everything but unreserved characters and `/`.
pub fn path_to_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `None` for anything but a local `file://` URI.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?;
    let bytes = encoded.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8(out).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_convert_per_encoding() {
        let line = "a€😀b";
        // chars: a(1/1/1) €(3/1/1) 😀(4/2/1) b
        assert_eq!(to_lsp(line, 3, Encoding::Utf16), 4);
        assert_eq!(to_lsp(line, 3, Encoding::Utf8), 8);
        assert_eq!(to_lsp(line, 3, Encoding::Utf32), 3);
        for encoding in [Encoding::Utf8, Encoding::Utf16, Encoding::Utf32] {
            for chars in 0..=4 {
                assert_eq!(from_lsp(line, to_lsp(line, chars, encoding), encoding), chars, "{encoding:?} {chars}");
            }
        }
        assert_eq!(from_lsp(line, 99, Encoding::Utf16), 4, "clamped to the line end");
        assert_eq!(Encoding::parse(None), Encoding::Utf16);
    }

    #[test]
    fn uris_round_trip_odd_paths() {
        let path = Path::new("/home/me/my project/naïve#1.rs");
        let uri = path_to_uri(path);
        assert_eq!(uri, "file:///home/me/my%20project/na%C3%AFve%231.rs");
        assert_eq!(uri_to_path(&uri).as_deref(), Some(path));
        assert_eq!(uri_to_path("https://x"), None);
    }
}
