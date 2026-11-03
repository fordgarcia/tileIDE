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

/// An LSP edit as plain data: (start line, column), (end line, column), new text.
pub type Edit = ((u32, u32), (u32, u32), String);

/// Applies LSP edits to `text`, for files a workspace edit changes that no editor has open.
pub fn apply_edits(text: &str, edits: &[Edit], encoding: Encoding) -> String {
    // Byte offset of each line start.
    let starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let offset = |(line, column): (u32, u32)| -> usize {
        let Some(&start) = starts.get(line as usize) else { return text.len() };
        let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
        let chars = from_lsp(&text[start..end], column, encoding);
        text[start..end].char_indices().nth(chars).map_or(end, |(i, _)| start + i)
    };
    // From the end; edits sharing a start go last-listed first (ranges refer to the original).
    let mut spans: Vec<(usize, usize, usize, &str)> = edits.iter().enumerate().map(|(i, (a, b, t))| (offset(*a), i, offset(*b), t.as_str())).collect();
    spans.sort_by_key(|s| std::cmp::Reverse((s.0, s.1)));
    let mut out = text.to_string();
    for (a, _, b, t) in spans {
        out.replace_range(a..b.max(a), t);
    }
    out
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
    fn edits_apply_to_plain_text() {
        let text = "x = 1\nprint(x)\n";
        let edits = vec![((0, 0), (0, 1), "count".to_string()), ((1, 6), (1, 7), "count".to_string())];
        assert_eq!(apply_edits(text, &edits, Encoding::Utf16), "count = 1\nprint(count)\n");
        assert_eq!(apply_edits("é = 1", &[((0, 4), (0, 5), "2".to_string())], Encoding::Utf16), "é = 2");
        assert_eq!(apply_edits("a", &[((5, 0), (5, 0), "\nb".to_string())], Encoding::Utf8), "a\nb");
        // An insert and a delete at the same place: both against the original text.
        assert_eq!(apply_edits("old rest", &[((0, 0), (0, 0), "new".to_string()), ((0, 0), (0, 3), String::new())], Encoding::Utf8), "new rest");
    }

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
