// SPDX-License-Identifier: GPL-3.0-or-later
//! The completion list: a themed card under the cursor. Typing keeps going to the
//! editor and filters the list (names starting with what's typed first, then fuzzy
//! matches, private names last); ↑/↓ or Ctrl+N/P select, Enter or Tab insert, Esc
//! closes. The selected item's missing letters also show faded after the cursor
//! (`prin` → `print`), so Tab reads as "take the suggestion", and its documentation
//! beside the list. Accepting an auto-import also adds its `import` line.
//! `lsp.rs` decides when it opens.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4 as gtk;
use tileide_core::search;
use tileide_core::theme::{self, Theme};
use tileide_lsp::protocol::{Completion, Range};
use tileide_lsp::text::Encoding;

const SHOWN: usize = 100;
/// Longest documentation shown per block, in chars.
const DOC_CHARS: usize = 1200;

type Resolve = Box<dyn Fn(&serde_json::Value, Box<dyn FnOnce(Completion)>) -> bool>;
type ApplyEdits = Box<dyn Fn(Vec<(Range, String)>, Encoding)>;

/// What the list asks of `lsp.rs`.
pub struct Hooks {
    /// Whether the server offers `completionItem/resolve`.
    pub can_resolve: Box<dyn Fn() -> bool>,
    /// Sends `completionItem/resolve` for an item's `raw`; `false` when the server
    /// doesn't offer it. The reply comes later.
    pub resolve: Resolve,
    /// Applies an accepted item's extra edits (ranges as the server sent them).
    pub edits: ApplyEdits,
}

pub fn css(t: &Theme) -> String {
    let c = theme::css;
    format!(
        "popover.tileide-completion > contents {{ background-color: {surface}; color: {text}; border: 1px solid {edge}; border-radius: 12px; padding: 4px; box-shadow: 0 10px 30px rgba(0, 0, 0, 0.3); }}
popover.tileide-completion list {{ background: transparent; }}
popover.tileide-completion row {{ border-radius: 8px; padding: 3px 8px; color: {text}; }}
popover.tileide-completion row:selected {{ background-color: {select}; }}
popover.tileide-completion .detail {{ color: {muted}; font-size: 0.9em; }}
popover.tileide-completion .kind {{ color: {accent}; font-size: 0.8em; }}
popover.tileide-completion .doc {{ color: {text}; padding: 4px 8px; }}
popover.tileide-completion .docdivider {{ background-color: {edge}; min-width: 1px; margin: 4px 2px; }}
textview label.tileide-ghost {{ color: {ghost}; padding: 0; margin: 0; }}",
        surface = c(t.surface, 0.98),
        text = c(t.text, 1.0),
        muted = c(t.muted, 1.0),
        edge = c(t.text, 0.12),
        select = c(t.accent, 0.18),
        ghost = c(t.muted, 0.85),
        accent = c(t.accent, 0.9),
    )
}

#[derive(Default)]
struct State {
    items: Vec<Completion>,
    /// Indexes into `items`, best match first.
    shown: Vec<usize>,
    incomplete: bool,
    /// The start of the word being completed.
    anchor: Option<gtk::TextMark>,
    encoding: Option<Encoding>,
    /// Bumped per `show`, so a late resolve reply never lands in a newer list.
    generation: u64,
    /// Items already sent for resolving.
    asked: HashSet<usize>,
}

pub struct Popup {
    view: glib::WeakRef<sourceview5::View>,
    popover: gtk::Popover,
    list: gtk::ListBox,
    scroll: gtk::ScrolledWindow,
    /// The selected item's documentation, right of the list.
    doc: gtk::Label,
    doc_side: gtk::Box,
    hooks: Hooks,
    me: RefCell<std::rc::Weak<Popup>>,
    /// The inline suggestion, a text-view overlay (the view removes it itself).
    ghost: gtk::Label,
    state: RefCell<State>,
}

impl Popup {
    pub fn new(view: &sourceview5::View, hooks: Hooks) -> Rc<Popup> {
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Browse);
        list.set_can_focus(false);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&list));
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroll.set_propagate_natural_height(true);
        scroll.set_propagate_natural_width(true);
        scroll.set_max_content_height(260);
        scroll.set_min_content_width(320);
        let popover = gtk::Popover::new();
        popover.add_css_class("tileide-completion");
        popover.set_has_arrow(false);
        // Not modal: the editor keeps the keyboard, so typing goes on while it is shown.
        popover.set_autohide(false);
        popover.set_can_focus(false);
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_halign(gtk::Align::Start);
        let doc = gtk::Label::new(None);
        doc.add_css_class("doc");
        doc.set_wrap(true);
        doc.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        doc.set_max_width_chars(48);
        doc.set_width_chars(36);
        doc.set_xalign(0.0);
        doc.set_yalign(0.0);
        let doc_scroll = gtk::ScrolledWindow::new();
        doc_scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        doc_scroll.set_propagate_natural_height(true);
        doc_scroll.set_propagate_natural_width(true);
        doc_scroll.set_max_content_height(260);
        doc_scroll.set_child(Some(&doc));
        let doc_side = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let divider = gtk::Box::new(gtk::Orientation::Vertical, 0);
        divider.add_css_class("docdivider");
        doc_side.append(&divider);
        doc_side.append(&doc_scroll);
        doc_side.set_visible(false);
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        content.append(&scroll);
        content.append(&doc_side);
        popover.set_child(Some(&content));
        // Unparented by `lsp::release_popovers` before the view goes away: GtkTextView's
        // dispose loops forever on a child it didn't add itself (GTK 4.24).
        popover.set_parent(view);

        let ghost = gtk::Label::new(None);
        ghost.add_css_class("tileide-ghost");
        ghost.set_can_target(false);
        ghost.set_visible(false);
        view.add_overlay(&ghost, 0, 0);

        let popup = Rc::new(Popup { view: view.downgrade(), popover, list, scroll, doc, doc_side, hooks, me: RefCell::new(std::rc::Weak::new()), ghost, state: RefCell::new(State::default()) });
        *popup.me.borrow_mut() = Rc::downgrade(&popup);

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&popup);
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(popup) = weak.upgrade().filter(|p| p.is_visible()) else { return glib::Propagation::Proceed };
            let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
            match key {
                gdk::Key::Down => popup.step(1),
                gdk::Key::Up => popup.step(-1),
                gdk::Key::n if ctrl => popup.step(1),
                gdk::Key::p if ctrl => popup.step(-1),
                gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::Tab => popup.accept(),
                gdk::Key::Escape => popup.hide(),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        view.add_controller(keys);

        let focus = gtk::EventControllerFocus::new();
        let weak = Rc::downgrade(&popup);
        focus.connect_leave(move |_| {
            if let Some(popup) = weak.upgrade() {
                popup.hide();
            }
        });
        view.add_controller(focus);

        let weak = Rc::downgrade(&popup);
        popup.list.connect_row_activated(move |list, row| {
            if let Some(popup) = weak.upgrade() {
                list.select_row(Some(row));
                popup.accept();
            }
        });
        popup
    }

    /// Test actions: the labels shown, best first (empty when closed).
    pub fn shown_labels(&self) -> Vec<String> {
        if !self.is_visible() {
            return Vec::new();
        }
        let state = self.state.borrow();
        state.shown.iter().map(|&i| state.items[i].label.clone()).collect()
    }

    pub fn is_visible(&self) -> bool {
        self.popover.is_visible()
    }

    /// Whether the server said the list is partial (re-ask as the word grows).
    pub fn incomplete(&self) -> bool {
        self.state.borrow().incomplete
    }

    /// Shows `items` for the word starting at `anchor`.
    pub fn show(&self, items: Vec<Completion>, incomplete: bool, anchor: &gtk::TextIter, encoding: Encoding) {
        let Some(view) = self.view.upgrade() else { return };
        let buffer = view.buffer();
        {
            let mut st = self.state.borrow_mut();
            if let Some(old) = st.anchor.take() {
                buffer.delete_mark(&old);
            }
            st.anchor = Some(buffer.create_mark(None, anchor, true));
            st.items = items;
            st.incomplete = incomplete;
            st.encoding = Some(encoding);
            st.generation += 1;
            st.asked.clear();
        }
        if !self.refilter() {
            return;
        }
        self.popover.set_pointing_to(Some(&crate::lsp::cursor_rect(&view, anchor)));
        self.align();
        if !self.popover.is_visible() {
            self.popover.popup();
        }
    }

    /// Re-filters against the word typed so far; hides (and returns false) when the
    /// cursor left the word or nothing matches.
    pub fn refilter(&self) -> bool {
        let Some(view) = self.view.upgrade() else { return false };
        let buffer = view.buffer();
        let cursor = buffer.iter_at_mark(&buffer.get_insert());
        let prefix = {
            let st = self.state.borrow();
            let Some(anchor) = st.anchor.as_ref().map(|m| buffer.iter_at_mark(m)) else { return false };
            if cursor.line() != anchor.line() || cursor.offset() < anchor.offset() {
                None
            } else {
                Some(buffer.text(&anchor, &cursor, false).to_string())
            }
        };
        let Some(prefix) = prefix.filter(|p| p.chars().all(crate::lsp::is_word_char)) else {
            self.hide();
            return false;
        };
        let shown = {
            let mut st = self.state.borrow_mut();
            st.shown = rank(&prefix, &st.items).into_iter().take(SHOWN).collect();
            st.shown.clone()
        };
        if shown.is_empty() {
            self.hide();
            return false;
        }
        self.list.remove_all();
        let st = self.state.borrow();
        for &i in &shown {
            self.list.append(&row(&st.items[i]));
        }
        self.list.select_row(self.list.row_at_index(0).as_ref());
        self.scroll.vadjustment().set_value(0.0);
        drop(st);
        self.selected_changed();
        true
    }

    /// The selection moved: the ghost text and the documentation follow it.
    fn selected_changed(&self) {
        self.update_doc();
        self.align();
        self.update_ghost();
    }

    fn selected(&self) -> Option<usize> {
        let index = usize::try_from(self.list.selected_row()?.index()).ok()?;
        self.state.borrow().shown.get(index).copied()
    }

    /// Shows the selected item's documentation, asking the server for it once.
    fn update_doc(&self) {
        let Some(i) = self.selected() else {
            self.doc_side.set_visible(false);
            return;
        };
        let (blocks, ask) = {
            let mut st = self.state.borrow_mut();
            let item = &st.items[i];
            let ask = (!item.raw.is_null()).then(|| item.raw.clone()).filter(|_| item.doc.is_empty() || item.edits.is_empty());
            let ask = ask.filter(|_| st.asked.insert(i)).map(|raw| (raw, st.generation));
            (st.items[i].doc.clone(), ask)
        };
        self.doc.set_markup(&doc_markup(&blocks));
        // Shown for the whole list when any item has (or may get) documentation: an open
        // popover that changes width can close itself.
        let (has_doc, resolvable) = {
            let st = self.state.borrow();
            (st.items.iter().any(|i| !i.doc.is_empty()), st.items.iter().any(|i| !i.raw.is_null()))
        };
        self.doc_side.set_visible(has_doc || (resolvable && (self.hooks.can_resolve)()));
        let Some((raw, generation)) = ask else { return };
        let weak = self.me.borrow().clone();
        (self.hooks.resolve)(
            &raw,
            Box::new(move |resolved| {
                let Some(popup) = weak.upgrade() else { return };
                {
                    let mut st = popup.state.borrow_mut();
                    if st.generation != generation {
                        return;
                    }
                    let item = &mut st.items[i];
                    if item.doc.is_empty() {
                        item.doc = resolved.doc;
                    }
                    if item.edits.is_empty() {
                        item.edits = resolved.edits;
                    }
                }
                if popup.selected() == Some(i) && popup.is_visible() {
                    popup.update_doc();
                }
            }),
        );
    }

    /// Shows the rest of the selected item after the cursor, faded: only when what's
    /// typed is its start and the cursor ends the line (it would cover text otherwise).
    fn update_ghost(&self) {
        let Some(view) = self.view.upgrade() else { return };
        let buffer = view.buffer();
        let cursor = buffer.iter_at_mark(&buffer.get_insert());
        let tail = (|| {
            let st = self.state.borrow();
            let index = self.list.selected_row()?.index();
            let item = st.items.get(*st.shown.get(usize::try_from(index).ok()?)?)?;
            let anchor = buffer.iter_at_mark(st.anchor.as_ref()?);
            let typed = buffer.text(&anchor, &cursor, false);
            let mut end = cursor;
            if !end.ends_line() {
                end.forward_to_line_end();
            }
            if !buffer.text(&cursor, &end, false).trim().is_empty() {
                return None;
            }
            let tail = item.insert.strip_prefix(typed.as_str())?;
            (!tail.is_empty() && !tail.contains('\n')).then(|| tail.to_string())
        })();
        match tail {
            Some(tail) => {
                let at = view.iter_location(&cursor);
                self.ghost.set_text(&tail);
                view.move_overlay(&self.ghost, at.x(), at.y());
                self.ghost.set_visible(true);
            }
            None => self.ghost.set_visible(false),
        }
    }

    /// A popover centres on what it points at; shifting it by half its width keeps the
    /// list's left edge under the word, however wide the matches make it.
    fn align(&self) {
        let (_, width, _, _) = self.popover.measure(gtk::Orientation::Horizontal, -1);
        self.popover.set_offset(width / 2, 0);
    }

    pub fn hide(&self) {
        self.ghost.set_visible(false);
        if self.popover.is_visible() {
            self.popover.popdown();
        }
        let mut st = self.state.borrow_mut();
        if let (Some(mark), Some(view)) = (st.anchor.take(), self.view.upgrade()) {
            view.buffer().delete_mark(&mark);
        }
        st.items.clear();
        st.shown.clear();
    }

    /// Tab completion: one match left is filled in at once (as Jupyter does). Returns
    /// whether it was.
    pub fn take_single(&self) -> bool {
        if self.is_visible() && self.state.borrow().shown.len() == 1 {
            self.accept();
            return true;
        }
        false
    }

    fn step(&self, by: i32) {
        let count = self.state.borrow().shown.len() as i32;
        if count == 0 {
            return;
        }
        let current = self.list.selected_row().map_or(0, |r| r.index());
        if let Some(row) = self.list.row_at_index((current + by).rem_euclid(count)) {
            self.list.select_row(Some(&row));
            if let Some(bounds) = row.compute_bounds(&self.list) {
                self.scroll.vadjustment().clamp_page(bounds.y() as f64, (bounds.y() + bounds.height()) as f64);
            }
        }
        self.selected_changed();
    }

    /// Replaces the word (or the server's range, up to the cursor) with the pick.
    pub fn accept(&self) {
        let Some(view) = self.view.upgrade() else { return };
        let buffer = view.buffer();
        let (item, anchor, encoding) = {
            let st = self.state.borrow();
            let index = self.list.selected_row().map_or(0, |r| r.index().max(0) as usize);
            let Some(&i) = st.shown.get(index) else { return };
            let Some(anchor) = st.anchor.as_ref().map(|m| buffer.iter_at_mark(m)) else { return };
            (st.items[i].clone(), anchor, st.encoding.unwrap_or(Encoding::Utf16))
        };
        self.hide();
        let end = buffer.iter_at_mark(&buffer.get_insert());
        let start = item.replace.map(|r| crate::lsp::iter_at(&buffer, r.start, encoding)).filter(|s| s.line() == end.line() && s.offset() <= end.offset()).unwrap_or(anchor);
        // Marks: the extra edits (an import line above) shift the text first, and
        // their ranges are in the text as the server last saw it.
        let (start_mark, end_mark) = (buffer.create_mark(None, &start, true), buffer.create_mark(None, &end, false));
        buffer.begin_user_action();
        if !item.edits.is_empty() {
            (self.hooks.edits)(item.edits, encoding);
        }
        let (mut start, mut end) = (buffer.iter_at_mark(&start_mark), buffer.iter_at_mark(&end_mark));
        buffer.delete(&mut start, &mut end);
        buffer.insert(&mut start, &item.insert);
        buffer.end_user_action();
        buffer.delete_mark(&start_mark);
        buffer.delete_mark(&end_mark);
        view.scroll_mark_onscreen(&buffer.get_insert());
    }
}

/// Indexes of `items` that match `prefix`, best first: names that start with it
/// (same case first), then fuzzy matches; private (`_x`) and dunder (`__x__`) names
/// after the rest unless `prefix` starts with `_`; then the server's order.
fn rank(prefix: &str, items: &[Completion]) -> Vec<usize> {
    let lower = prefix.to_lowercase();
    let mut keyed: Vec<((u8, u8, i64), usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| {
            let name = item.filter.as_str();
            let tier = if name.starts_with(prefix) {
                0
            } else if name.to_lowercase().starts_with(&lower) {
                1
            } else {
                2
            };
            let score = search::score(prefix, name)?;
            let private = match name {
                _ if prefix.starts_with('_') => 0,
                n if n.starts_with("__") && n.ends_with("__") && n.len() > 4 => 2,
                n if n.starts_with('_') => 1,
                _ => 0,
            };
            // Fuzzy matches keep their own order; prefix matches keep the server's.
            Some(((tier, private, if tier == 2 { -score } else { 0 }), i))
        })
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| items[a.1].sort.cmp(&items[b.1].sort)));
    keyed.into_iter().map(|(_, i)| i).collect()
}

/// Documentation blocks as Pango markup: code in a monospace font.
fn doc_markup(blocks: &[(bool, String)]) -> String {
    blocks
        .iter()
        .map(|(code, text)| {
            let text = match text.char_indices().nth(DOC_CHARS) {
                Some((at, _)) => format!("{}…", &text[..at]),
                None => text.clone(),
            };
            let text = glib::markup_escape_text(&text);
            if *code { format!("<tt>{text}</tt>") } else { text.to_string() }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn row(item: &Completion) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let label = gtk::Label::new(Some(&item.label));
    label.set_xalign(0.0);
    row.append(&label);
    if let Some(kind) = item.kind {
        let tag = gtk::Label::new(Some(kind));
        tag.add_css_class("kind");
        row.append(&tag);
    }
    if let Some(detail) = &item.detail {
        let detail = gtk::Label::new(Some(detail));
        detail.add_css_class("detail");
        detail.set_hexpand(true);
        detail.set_xalign(1.0);
        detail.set_ellipsize(gtk::pango::EllipsizeMode::End);
        detail.set_max_width_chars(36);
        row.append(&detail);
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, sort: &str) -> Completion {
        Completion { label: name.into(), detail: None, insert: name.into(), replace: None, filter: name.into(), sort: sort.into(), kind: None, doc: Vec::new(), edits: Vec::new(), raw: serde_json::Value::Null }
    }

    fn names(prefix: &str, items: &[Completion]) -> Vec<String> {
        rank(prefix, items).into_iter().map(|i| items[i].label.clone()).collect()
    }

    #[test]
    fn prefix_matches_come_before_fuzzy_ones() {
        // `df.pri`: pandas' plain names before `__array_priority__`.
        let items = [item("__array_priority__", "0"), item("prod", "1"), item("print_table", "2"), item("pipe", "3"), item("Print", "4")];
        assert_eq!(names("pri", &items), ["print_table", "Print", "__array_priority__"]);
    }

    #[test]
    fn private_and_dunder_names_go_last_unless_typed() {
        // `os.` as pyright orders it: dunders first.
        let items = [item("__package__", "14.9999.__package__"), item("_path", "12.9999._path"), item("path", "12.9999.path"), item("pathsep", "12.9999.pathsep")];
        assert_eq!(names("", &items), ["path", "pathsep", "_path", "__package__"]);
        assert_eq!(names("pa", &items), ["path", "pathsep", "_path", "__package__"]);
        assert_eq!(names("_", &items)[..2], ["_path", "__package__"]);
    }

    #[test]
    fn documentation_is_escaped_and_code_monospace() {
        assert_eq!(doc_markup(&[(true, "f(a: int) -> list[int]".into()), (false, "Returns <a> & b.".into())]), "<tt>f(a: int) -&gt; list[int]</tt>\n\nReturns &lt;a&gt; &amp; b.");
    }
}
