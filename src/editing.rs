// SPDX-License-Identifier: GPL-3.0-or-later
//! Typing aids for editors and notebook cells (v0.5 phase 1, `tileide.editor`):
//! closing brackets and quotes as they are typed, and the matching-bracket highlight.
//! `configure` applies the settings to every live editor; `attach` sets one up.

use std::cell::{Cell, RefCell};

use gtk4 as gtk;
use gtk::{gdk, glib};
use sourceview5::prelude::*;

const PAIRS: [(char, char); 5] = [('(', ')'), ('[', ']'), ('{', '}'), ('"', '"'), ('\'', '\'')];

thread_local! {
    static AUTO_CLOSE: Cell<bool> = const { Cell::new(true) };
    static MATCH_BRACKETS: Cell<bool> = const { Cell::new(false) };
    /// Every attached buffer, so a settings change reaches the open ones.
    static BUFFERS: RefCell<Vec<glib::WeakRef<sourceview5::Buffer>>> = const { RefCell::new(Vec::new()) };
}

pub fn configure(auto_close: bool, match_brackets: bool) {
    AUTO_CLOSE.set(auto_close);
    MATCH_BRACKETS.set(match_brackets);
    BUFFERS.with_borrow_mut(|buffers| {
        buffers.retain(|b| b.upgrade().inspect(|b| b.set_highlight_matching_brackets(match_brackets)).is_some());
    });
}

pub fn attach(view: &sourceview5::View) {
    let Some(buffer) = view.buffer().downcast::<sourceview5::Buffer>().ok() else { return };
    buffer.set_highlight_matching_brackets(MATCH_BRACKETS.get());
    BUFFERS.with_borrow_mut(|buffers| buffers.push(buffer.downgrade()));
    let keys = gtk::EventControllerKey::new();
    // Before the text view inserts the character itself.
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed(move |_, key, _, mods| {
        let plain = (mods & (gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK | gdk::ModifierType::SUPER_MASK)).is_empty();
        if !AUTO_CLOSE.get() || !plain {
            return glib::Propagation::Proceed;
        }
        let handled = if key == gdk::Key::BackSpace { backspace(&buffer) } else { key.to_unicode().is_some_and(|c| typed(&buffer, c)) };
        if handled { glib::Propagation::Stop } else { glib::Propagation::Proceed }
    });
    view.add_controller(keys);
}

/// Whether a file indents with tabs: Makefiles do, and so does one whose indented lines
/// mostly start with a tab. Anything else gets spaces (Tab, auto-indent, and formatters
/// that follow the editor's choice, such as jdtls).
pub fn indents_with_tabs(path: Option<&std::path::Path>, text: &str) -> bool {
    let name = path.and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("");
    if matches!(name, "Makefile" | "makefile" | "GNUmakefile") || name.ends_with(".mk") {
        return true;
    }
    let (tabs, spaces) = text.lines().fold((0, 0), |(t, s), line| match line.chars().next() {
        Some('\t') => (t + 1, s),
        Some(' ') => (t, s + 1),
        _ => (t, s),
    });
    tabs > spaces
}

/// Headless checks: `c` as if typed (`\x08` = Backspace).
pub fn type_char(buffer: &sourceview5::Buffer, c: char) {
    let handled = AUTO_CLOSE.get() && if c == '\x08' { backspace(buffer) } else { typed(buffer, c) };
    if !handled {
        if c == '\x08' {
            buffer.backspace(&mut buffer.iter_at_mark(&buffer.get_insert()), true, true);
        } else {
            buffer.insert_interactive_at_cursor(&c.to_string(), true);
        }
    }
}

/// What typing a character does beside the cursor.
#[derive(Debug, PartialEq)]
enum Edit {
    /// Insert the character and its partner, cursor between them.
    Pair(char),
    /// Put the selection between the character and its partner.
    Wrap(char, char),
    /// The partner is already next: move over it.
    StepOver,
}

fn partner(open: char) -> Option<char> {
    PAIRS.iter().find(|(o, _)| *o == open).map(|(_, c)| *c)
}

fn is_closer(c: char) -> bool {
    PAIRS.iter().any(|(o, close)| *close == c && *o != c)
}

/// `c` typed between `before` and `after` (`None` = line start/end); `selected`: text
/// is selected. `None` means a plain insert.
fn on_char(c: char, before: Option<char>, after: Option<char>, selected: bool) -> Option<Edit> {
    let quote = c == '"' || c == '\'';
    if !selected && after == Some(c) && (is_closer(c) || quote) {
        return Some(Edit::StepOver);
    }
    let close = partner(c)?;
    if selected {
        return Some(Edit::Wrap(c, close));
    }
    // `don't`, `x'`, and a third quote of `"""` stay plain.
    if quote && before.is_some_and(|b| b.is_alphanumeric() || b == '_' || b == c) {
        return None;
    }
    // Only before nothing that would become part of the pair: `(|foo` stays `(foo`.
    let free = after.is_none_or(|a| a.is_whitespace() || is_closer(a) || matches!(a, ',' | ';' | ':'));
    free.then_some(Edit::Pair(c))
}

/// Backspace between an empty pair removes both.
fn deletes_pair(before: Option<char>, after: Option<char>) -> bool {
    matches!((before, after), (Some(b), Some(a)) if partner(b) == Some(a))
}

/// The characters on each side of the cursor, within its line.
fn around(buffer: &sourceview5::Buffer) -> (gtk::TextIter, Option<char>, Option<char>) {
    let at = buffer.iter_at_mark(&buffer.get_insert());
    let before = (!at.starts_line()).then(|| {
        let mut b = at;
        b.backward_char();
        b.char()
    });
    let after = (!at.ends_line()).then(|| at.char());
    (at, before, after)
}

fn typed(buffer: &sourceview5::Buffer, c: char) -> bool {
    let selection = buffer.selection_bounds();
    let (mut at, before, after) = around(buffer);
    let Some(edit) = on_char(c, before, after, selection.is_some()) else { return false };
    buffer.begin_user_action();
    match edit {
        Edit::StepOver => {
            at.forward_char();
            buffer.place_cursor(&at);
        }
        Edit::Pair(open) => {
            let close = partner(open).unwrap_or(open);
            buffer.insert_interactive_at_cursor(&format!("{open}{close}"), true);
            let mut cursor = buffer.iter_at_mark(&buffer.get_insert());
            cursor.backward_char();
            buffer.place_cursor(&cursor);
        }
        Edit::Wrap(open, close) => {
            let (start, end) = selection.unwrap_or((at, at));
            let (start_mark, end_mark) = (buffer.create_mark(None, &start, true), buffer.create_mark(None, &end, false));
            buffer.insert(&mut buffer.iter_at_mark(&end_mark), &close.to_string());
            buffer.insert(&mut buffer.iter_at_mark(&start_mark), &open.to_string());
            // The wrapped text stays selected, inside the pair.
            let mut inner_start = buffer.iter_at_mark(&start_mark);
            inner_start.forward_char();
            let mut inner_end = buffer.iter_at_mark(&end_mark);
            inner_end.backward_char();
            buffer.select_range(&inner_start, &inner_end);
            buffer.delete_mark(&start_mark);
            buffer.delete_mark(&end_mark);
        }
    }
    buffer.end_user_action();
    true
}

fn backspace(buffer: &sourceview5::Buffer) -> bool {
    if buffer.has_selection() {
        return false;
    }
    let (at, before, after) = around(buffer);
    if !deletes_pair(before, after) {
        return false;
    }
    let (mut start, mut end) = (at, at);
    start.backward_char();
    end.forward_char();
    buffer.begin_user_action();
    buffer.delete_interactive(&mut start, &mut end, true);
    buffer.end_user_action();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indentation_follows_the_file() {
        use std::path::Path;
        assert!(!indents_with_tabs(None, ""));
        assert!(!indents_with_tabs(Some(Path::new("a.py")), "def f():\n    pass\n"));
        assert!(indents_with_tabs(Some(Path::new("main.go")), "func f() {\n\treturn\n}\n"));
        assert!(indents_with_tabs(Some(Path::new("Makefile")), "all:\n"));
    }

    #[test]
    fn brackets_pair_only_before_free_space() {
        assert_eq!(on_char('(', Some('f'), None, false), Some(Edit::Pair('(')));
        assert_eq!(on_char('[', None, Some(')'), false), Some(Edit::Pair('[')));
        assert_eq!(on_char('{', None, Some(' '), false), Some(Edit::Pair('{')));
        assert_eq!(on_char('(', None, Some('x'), false), None, "(|x stays plain");
        assert_eq!(on_char('(', None, None, true), Some(Edit::Wrap('(', ')')));
    }

    #[test]
    fn closers_and_quotes_step_over() {
        assert_eq!(on_char(')', Some('a'), Some(')'), false), Some(Edit::StepOver));
        assert_eq!(on_char(']', None, Some(')'), false), None);
        assert_eq!(on_char('"', Some('a'), Some('"'), false), Some(Edit::StepOver));
        assert_eq!(on_char('x', None, None, false), None);
    }

    #[test]
    fn quotes_stay_plain_inside_words() {
        assert_eq!(on_char('\'', Some('n'), None, false), None, "don't");
        assert_eq!(on_char('"', Some('"'), None, false), None, "the third quote of \"\"\"");
        assert_eq!(on_char('"', Some('('), Some(')'), false), Some(Edit::Pair('"')));
        assert_eq!(on_char('\'', None, None, false), Some(Edit::Pair('\'')));
    }

    #[test]
    fn backspace_removes_an_empty_pair() {
        assert!(deletes_pair(Some('('), Some(')')));
        assert!(deletes_pair(Some('"'), Some('"')));
        assert!(!deletes_pair(Some('('), Some(']')));
        assert!(!deletes_pair(None, Some(')')));
    }
}
