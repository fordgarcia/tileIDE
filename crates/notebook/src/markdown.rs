// SPDX-License-Identifier: GPL-3.0-or-later
//! Markdown cells shown rendered: Markdown → Pango markup for a GTK label. Covers what
//! notebooks mostly use: headings, emphasis, inline and fenced code, lists, quotes,
//! links, rules and pipe tables (kept monospaced). Images show as their alt text; LaTeX
//! is left as written.

/// Pango markup for `source`.
pub fn to_markup(source: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    // Plain lines of the paragraph being built (Markdown joins them with spaces).
    let mut paragraph: Vec<&str> = Vec::new();
    let mut fence: Option<Vec<String>> = None;
    let flush = |paragraph: &mut Vec<&str>, out: &mut Vec<String>| {
        if !paragraph.is_empty() {
            out.push(inline(&paragraph.join(" ")));
            paragraph.clear();
        }
    };
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(code) = fence.as_mut() {
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                out.push(format!("<tt>{}</tt>", escape(&code.join("\n"))));
                fence = None;
            } else {
                code.push(line.to_string());
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            flush(&mut paragraph, &mut out);
            fence = Some(Vec::new());
            continue;
        }
        if trimmed.is_empty() {
            flush(&mut paragraph, &mut out);
            if out.last().is_some_and(|l| !l.is_empty()) {
                out.push(String::new());
            }
            continue;
        }
        let hashes = trimmed.chars().take_while(|&c| c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            flush(&mut paragraph, &mut out);
            let size = ["xx-large", "x-large", "large", "medium", "medium", "medium"][hashes - 1];
            out.push(format!("<span size=\"{size}\" weight=\"bold\">{}</span>", inline(trimmed[hashes..].trim())));
            continue;
        }
        if trimmed.len() >= 3 && ["-", "*", "_"].iter().any(|m| trimmed.chars().all(|c| c.to_string() == *m || c == ' ')) {
            flush(&mut paragraph, &mut out);
            out.push("──────────────────".into());
            continue;
        }
        let indent = "    ".repeat((line.len() - line.trim_start().len()) / 2);
        if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")).or_else(|| trimmed.strip_prefix("+ ")) {
            flush(&mut paragraph, &mut out);
            out.push(format!("{indent}  •  {}", inline(rest)));
            continue;
        }
        let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && trimmed[digits..].starts_with(". ") {
            flush(&mut paragraph, &mut out);
            out.push(format!("{indent}  {}.  {}", &trimmed[..digits], inline(&trimmed[digits + 2..])));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('>') {
            flush(&mut paragraph, &mut out);
            out.push(format!("<span alpha=\"75%\">▎ <i>{}</i></span>", inline(rest.trim())));
            continue;
        }
        if trimmed.starts_with('|') {
            flush(&mut paragraph, &mut out);
            out.push(format!("<tt>{}</tt>", escape(trimmed)));
            continue;
        }
        paragraph.push(trimmed);
    }
    flush(&mut paragraph, &mut out);
    if let Some(code) = fence {
        out.push(format!("<tt>{}</tt>", escape(&code.join("\n"))));
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out.join("\n")
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Emphasis, code, links and images in one line; unclosed marks stay as written.
fn inline(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let find = |from: usize, pattern: &[char]| (from..chars.len()).find(|&j| chars[j..].starts_with(pattern));
    let slice = |a: usize, b: usize| chars[a..b].iter().collect::<String>();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            out.push_str(&escape(&chars[i + 1].to_string()));
            i += 2;
            continue;
        }
        if c == '`'
            && let Some(end) = find(i + 1, &['`'])
        {
            out.push_str(&format!("<tt>{}</tt>", escape(&slice(i + 1, end))));
            i = end + 1;
            continue;
        }
        if (c == '*' || c == '_') && chars.get(i + 1) == Some(&c)
            && let Some(end) = find(i + 2, &[c, c]).filter(|&e| e > i + 2)
        {
            out.push_str(&format!("<b>{}</b>", inline(&slice(i + 2, end))));
            i = end + 2;
            continue;
        }
        // `_` inside a word (snake_case) is not emphasis.
        let word_before = i > 0 && chars[i - 1].is_alphanumeric();
        if (c == '*' || (c == '_' && !word_before)) && chars.get(i + 1).is_some_and(|n| !n.is_whitespace())
            && let Some(end) = find(i + 1, &[c]).filter(|&e| e > i + 1 && !chars[e - 1].is_whitespace() && chars[e - 1] != '\\')
        {
            out.push_str(&format!("<i>{}</i>", inline(&slice(i + 1, end))));
            i = end + 1;
            continue;
        }
        let image = c == '!' && chars.get(i + 1) == Some(&'[');
        let start = if image { i + 1 } else { i };
        if chars[start] == '['
            && let Some(close) = find(start + 1, &[']', '('])
            && let Some(end) = find(close + 2, &[')'])
        {
            let (label, url) = (slice(start + 1, close), slice(close + 2, end));
            if image {
                out.push_str(&format!("<i>[{}]</i>", escape(if label.is_empty() { "image" } else { &label })));
            } else {
                out.push_str(&format!("<a href=\"{}\">{}</a>", escape(url.split_whitespace().next().unwrap_or("")), inline(&label)));
            }
            i = end + 1;
            continue;
        }
        out.push_str(&escape(&c.to_string()));
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_render() {
        let md = "# Title\n\nSome *soft*\nwrapped text.\n\n- one\n  - two\n1. first\n> quoted\n---\n```python\nx = 1 < 2\n```\n| a | b |";
        assert_eq!(
            to_markup(md),
            "<span size=\"xx-large\" weight=\"bold\">Title</span>\n\nSome <i>soft</i> wrapped text.\n\n  •  one\n      •  two\n  1.  first\n<span alpha=\"75%\">▎ <i>quoted</i></span>\n──────────────────\n<tt>x = 1 &lt; 2</tt>\n<tt>| a | b |</tt>"
        );
    }

    #[test]
    fn inline_marks_render_and_unclosed_ones_stay() {
        assert_eq!(inline("**bold** and __b__, `a<b`, snake_case_name"), "<b>bold</b> and <b>b</b>, <tt>a&lt;b</tt>, snake_case_name");
        assert_eq!(inline("[docs](https://x.org \"t\") ![plot](a.png)"), "<a href=\"https://x.org\">docs</a> <i>[plot]</i>");
        assert_eq!(inline("2 * 3 = 6, *open, a & b, \\*lit\\*"), "2 * 3 = 6, *open, a &amp; b, *lit*");
        assert_eq!(to_markup(""), "");
    }
}
