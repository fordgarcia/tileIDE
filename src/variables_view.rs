// SPDX-License-Identifier: GPL-3.0-or-later
//! A notebook's variables as a tile (v0.5 phase 8): name, type, size or shape and a
//! short value, asked of the running kernel without a count or output
//! (`Kernel::eval`), refreshed after each run while the tile is open. A DataFrame's
//! name opens it whole in a table tile.

use std::rc::Rc;

use gtk::prelude::*;
use gtk4 as gtk;

#[derive(Debug, Clone, PartialEq)]
pub struct Var {
    pub name: String,
    pub kind: String,
    pub size: String,
    pub value: String,
    /// Shown as a table when opened (has `to_html` and a `shape`).
    pub table: bool,
}

/// The kernel's answer to `_tileide_variables(globals())`, sorted by name.
pub fn parse(json: &str) -> Result<Vec<Var>, String> {
    let rows: Vec<(String, String, String, String, bool)> = serde_json::from_str(json).map_err(|e| format!("unexpected answer: {e}"))?;
    let mut vars: Vec<Var> = rows.into_iter().map(|(name, kind, size, value, table)| Var { name, kind, size, value, table }).collect();
    vars.sort_by_key(|v| v.name.to_lowercase());
    Ok(vars)
}

/// The expression that lists a kernel's variables (helper defined by the bridge).
pub const LIST: &str = "_tileide_variables(globals())";

/// The expression for variable `name` as an HTML table (at most 1000 rows, 50 columns).
pub fn table_expr(name: &str) -> String {
    // A JSON string is a valid Python string literal.
    format!("globals()[{}].to_html(max_rows=1000, max_cols=50)", serde_json::Value::String(name.into()))
}

type OpenTable = Rc<dyn Fn(String)>;

pub struct VariablesView {
    root: gtk::Box,
    list: gtk::Box,
    note: gtk::Label,
    open_table: OpenTable,
}

impl VariablesView {
    /// `notebook` names the tile; `open_table` gets a DataFrame's name; `refresh` asks again.
    pub fn new(notebook: &str, open_table: OpenTable, refresh: impl Fn() + 'static) -> Rc<VariablesView> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        // The notebook's styles (bar, tables) apply here too.
        root.add_css_class("notebook");
        root.add_css_class("variables");
        root.set_widget_name("Variables");
        root.set_focusable(true);
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.add_css_class("bar");
        let title = gtk::Label::new(Some(&format!("Variables · {notebook}")));
        title.add_css_class("kernel");
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        let again = gtk::Button::with_label("Refresh");
        again.add_css_class("pill");
        again.set_valign(gtk::Align::Center);
        again.connect_clicked(move |_| refresh());
        bar.append(&title);
        bar.append(&spacer);
        bar.append(&again);
        root.append(&bar);
        let note = gtk::Label::new(None);
        note.add_css_class("frame-table-note");
        note.set_xalign(0.0);
        note.set_wrap(true);
        root.append(&note);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_vexpand(true);
        scroll.set_child(Some(&list));
        root.append(&scroll);
        Rc::new(VariablesView { root, list, note, open_table })
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// Whether the tile is open (refreshes are skipped otherwise).
    pub fn is_shown(&self) -> bool {
        self.root.parent().is_some()
    }

    pub fn message(&self, text: &str) {
        self.note.set_text(text);
        self.note.set_visible(!text.is_empty());
    }

    pub fn set(&self, vars: &[Var]) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        self.message(if vars.is_empty() { "No variables yet." } else { "" });
        if vars.is_empty() {
            return;
        }
        let grid = gtk::Grid::new();
        grid.add_css_class("frame-table");
        grid.set_halign(gtk::Align::Start);
        for (x, head) in ["Name", "Type", "Size", "Value"].iter().enumerate() {
            let l = gtk::Label::new(Some(head));
            l.set_xalign(0.0);
            l.add_css_class("th");
            l.add_css_class("head");
            grid.attach(&l, x as i32, 0, 1, 1);
        }
        for (y, v) in vars.iter().enumerate() {
            let y = y as i32 + 1;
            let name: gtk::Widget = if v.table {
                let l = gtk::Label::new(None);
                l.set_markup(&format!("<a href=\"open\">{}</a>", gtk::glib::markup_escape_text(&v.name)));
                l.set_tooltip_text(Some("Open as a table"));
                let (open, name) = (self.open_table.clone(), v.name.clone());
                l.connect_activate_link(move |_, _| {
                    open(name.clone());
                    gtk::glib::Propagation::Stop
                });
                l.upcast()
            } else {
                gtk::Label::new(Some(&v.name)).upcast()
            };
            let cells = [name, gtk::Label::new(Some(&v.kind)).upcast(), gtk::Label::new(Some(&v.size)).upcast(), {
                let l = gtk::Label::new(Some(&v.value));
                l.set_ellipsize(gtk::pango::EllipsizeMode::End);
                l.set_max_width_chars(60);
                l.set_selectable(true);
                l.upcast()
            }];
            for (x, w) in cells.iter().enumerate() {
                if let Some(l) = w.downcast_ref::<gtk::Label>() {
                    l.set_xalign(0.0);
                }
                if y % 2 == 0 {
                    w.add_css_class("odd");
                }
                grid.attach(w, x as i32, y, 1, 1);
            }
        }
        self.list.append(&grid);
    }

    /// Test action: the rows as text.
    pub fn describe(&self) -> String {
        let mut out = self.note.text().to_string();
        let mut child = self.list.first_child().and_then(|g| g.first_child());
        let mut row = Vec::new();
        while let Some(c) = child {
            if let Some(l) = c.downcast_ref::<gtk::Label>() {
                row.push(l.text().to_string());
                if row.len() == 4 {
                    out += &format!(" | {}", row.join(" · "));
                    row.clear();
                }
            }
            child = c.next_sibling();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_kernels_list() {
        let vars = parse(r#"[["x", "int", "", "42", false], ["df", "DataFrame", "3 x 2", "   a  b ...", true]]"#).unwrap();
        assert_eq!(vars[0], Var { name: "df".into(), kind: "DataFrame".into(), size: "3 x 2".into(), value: "   a  b ...".into(), table: true });
        assert_eq!(vars[1].name, "x");
        assert!(parse("Traceback").is_err());
        assert_eq!(table_expr("df"), r#"globals()["df"].to_html(max_rows=1000, max_cols=50)"#);
    }
}
