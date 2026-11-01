// SPDX-License-Identifier: GPL-3.0-or-later
//! HTML tables in outputs (pandas DataFrames, Styler, `to_html`): read into rows of
//! cells so the notebook shows a real table instead of the text/plain fallback. Only
//! the table's structure and text are kept; `rowspan`/`colspan` are not laid out.

#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub text: String,
    /// A `<th>`: a column header, or a row's index label.
    pub header: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Table {
    /// Rows from `<thead>`.
    pub head: Vec<Vec<Cell>>,
    pub body: Vec<Vec<Cell>>,
    /// Text after the table, e.g. pandas' "100 rows × 3 columns".
    pub note: Option<String>,
}

/// The first `<table>` in `html`, or `None` without one.
pub fn parse(html: &str) -> Option<Table> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<table")?;
    let end = lower[start..].find("</table>").map_or(html.len(), |e| start + e + "</table>".len());
    let mut table = Table::default();
    let (mut in_head, mut row, mut cell): (bool, Option<Vec<Cell>>, Option<Cell>) = (false, None, None);
    let mut i = start;
    while i < end {
        if html[i..].starts_with('<') {
            let close = html[i..].find('>').map_or(end, |c| i + c);
            let tag = lower[i + 1..close].trim();
            let (closing, name) = match tag.strip_prefix('/') {
                Some(n) => (true, n.split_whitespace().next().unwrap_or("")),
                None => (false, tag.split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("")),
            };
            match (closing, name) {
                (false, "thead") => in_head = true,
                (true, "thead") => in_head = false,
                (false, "tr") => row = Some(Vec::new()),
                (true, "tr") => {
                    if let Some(mut r) = row.take() {
                        r.extend(cell.take().map(finish));
                        if in_head { table.head.push(r) } else { table.body.push(r) }
                    }
                }
                (false, "th" | "td") => {
                    if let (Some(r), Some(c)) = (row.as_mut(), cell.take()) {
                        r.push(finish(c));
                    }
                    cell = Some(Cell { text: String::new(), header: name == "th" });
                }
                (true, "th" | "td") => {
                    if let (Some(r), Some(c)) = (row.as_mut(), cell.take()) {
                        r.push(finish(c));
                    }
                }
                (false, "br") => {
                    if let Some(c) = cell.as_mut() {
                        c.text.push('\n');
                    }
                }
                // Styles and scripts hold no cell text.
                (false, "style" | "script") => {
                    let closing_tag = format!("</{name}");
                    i = lower[close..].find(&closing_tag).map_or(end, |c| close + c);
                    continue;
                }
                _ => {}
            }
            i = close + 1;
            continue;
        }
        let next = html[i..].find('<').map_or(end, |n| (i + n).min(end));
        if let Some(c) = cell.as_mut() {
            c.text.push_str(&decode(&html[i..next]));
        }
        i = next;
    }
    // A last row left open (`</tr>` is optional in HTML).
    if let Some(mut r) = row.take() {
        r.extend(cell.take().map(finish));
        if in_head { table.head.push(r) } else { table.body.push(r) }
    }
    let after = strip_tags(&html[end.min(html.len())..]);
    table.note = Some(after.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|n| !n.is_empty());
    (!table.head.is_empty() || !table.body.is_empty()).then_some(table)
}

fn finish(mut c: Cell) -> Cell {
    c.text = c.text.split_whitespace().collect::<Vec<_>>().join(" ");
    c
}

fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        out.push_str(&decode(&rest[..i]));
        out.push(' ');
        rest = rest[i..].find('>').map_or("", |j| &rest[i + j + 1..]);
    }
    out.push_str(&decode(rest));
    out
}

/// HTML entities: the named ones tables use, and numeric ones.
fn decode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            "hellip" => Some('…'),
            "times" => Some('×'),
            _ => entity.strip_prefix('#').and_then(|n| match n.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok(),
                None => n.parse().ok(),
            }).and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pandas_frames_parse() {
        let html = r#"<div><style scoped>.dataframe tbody tr th { vertical-align: top; }</style>
<table border="1" class="dataframe">
  <thead><tr style="text-align: right;"><th></th><th>price</th><th>name</th></tr></thead>
  <tbody>
    <tr><th>0</th><td>1.5</td><td>A &amp; B</td></tr>
    <tr><th>1</th><td>2</td><td>&lt;none&gt;</td></tr>
  </tbody>
</table>
<p>2 rows &times; 2 columns</p></div>"#;
        let t = parse(html).unwrap();
        fn texts(rows: &[Vec<Cell>]) -> Vec<Vec<&str>> {
            rows.iter().map(|r| r.iter().map(|c| c.text.as_str()).collect()).collect()
        }
        assert_eq!(texts(&t.head), vec![vec!["", "price", "name"]]);
        assert_eq!(texts(&t.body), vec![vec!["0", "1.5", "A & B"], vec!["1", "2", "<none>"]]);
        assert!(t.body[0][0].header && !t.body[0][1].header);
        assert_eq!(t.note.as_deref(), Some("2 rows × 2 columns"));
    }

    #[test]
    fn non_tables_and_odd_markup() {
        assert_eq!(parse("<b>just html</b>"), None);
        let t = parse("<TABLE><TR><TD>a<br/>b<TD>c &#233; &#x41;</TABLE>").unwrap();
        assert_eq!(t.body[0].iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), vec!["a b", "c é A"]);
        assert_eq!(decode("AT&T &bogus; 5 &lt 6"), "AT&T &bogus; 5 &lt 6");
    }
}
