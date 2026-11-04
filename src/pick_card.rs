// SPDX-License-Identifier: GPL-3.0-or-later
//! A short list at the cursor for choices a language server offers (references, quick
//! fixes). Like the completion list it never takes the keyboard (a grab would hand
//! focus to whatever window is under the mouse when it ends): ↑/↓ select, Enter picks,
//! Esc or leaving the editor closes; a click picks too. Styled as the completion list.
//! `show_filtered` lets typing narrow the list (letters in order, any case).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4 as gtk;

pub fn show(view: &sourceview5::View, title: &str, rows: &[String], on_pick: impl Fn(usize) + 'static) {
    open(view, title, rows, None, on_pick);
}

/// `show`, where typed letters filter the rows by their `keys` (one per row;
/// Backspace takes one back).
pub fn show_filtered(view: &sourceview5::View, title: &str, rows: &[String], keys: Vec<String>, on_pick: impl Fn(usize) + 'static) {
    open(view, title, rows, Some(keys), on_pick);
}

/// The letters of `query` appear in `row` in order, ignoring case.
fn matches(row: &str, query: &str) -> bool {
    let mut chars = row.chars().flat_map(char::to_lowercase);
    query.chars().flat_map(char::to_lowercase).all(|q| chars.any(|c| c == q))
}

/// Shows the rows matching a query; gives back their texts.
type Refilter = Rc<dyn Fn(&str) -> Vec<String>>;
/// The open filtered card, for test actions: (refilter, pick a shown row).
type Open = Option<(Refilter, Rc<dyn Fn(i32)>)>;

thread_local! {
    static TEST: RefCell<Open> = const { RefCell::new(None) };
}

/// Test actions: types `query` into the open filtered card, prints the rows left and
/// picks row `pick` of them.
pub fn test(query: &str, pick: Option<usize>) -> Vec<String> {
    let Some((filter, pick_row)) = TEST.with_borrow(|t| t.clone()) else { return Vec::new() };
    let rows = filter(query);
    if let Some(i) = pick {
        pick_row(i as i32);
    }
    rows
}

fn open(view: &sourceview5::View, title: &str, rows: &[String], keys: Option<Vec<String>>, on_pick: impl Fn(usize) + 'static) {
    let filtered = keys.is_some();
    let keys = keys.unwrap_or_default();
    // One list at a time (TileIDE's own keys never reach the old one to close it).
    let mut child = view.first_child();
    while let Some(c) = child {
        child = c.next_sibling();
        if let Some(p) = c.downcast_ref::<gtk::Popover>().filter(|p| p.has_css_class("tileide-pick")) {
            p.popdown();
        }
    }
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Browse);
    list.set_can_focus(false);
    // Indices into `rows` of the rows shown, in order.
    let shown: Rc<RefCell<Vec<usize>>> = Rc::new(RefCell::new(Vec::new()));
    let fill = {
        let (l, shown, rows) = (list.downgrade(), shown.clone(), rows.to_vec());
        let key = move |i: usize| keys.get(i).cloned().unwrap_or_default();
        move |query: &str| -> Vec<String> {
            let Some(list) = l.upgrade() else { return Vec::new() };
            list.remove_all();
            let keep: Vec<usize> = (0..rows.len()).filter(|&i| query.is_empty() || matches(&key(i), query)).collect();
            for &i in &keep {
                let label = gtk::Label::new(Some(&rows[i]));
                label.set_xalign(0.0);
                label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                label.set_max_width_chars(90);
                list.append(&label);
            }
            list.select_row(list.row_at_index(0).as_ref());
            let texts = keep.iter().map(|&i| rows[i].clone()).collect();
            *shown.borrow_mut() = keep;
            texts
        }
    };
    fill("");
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_child(Some(&list));
    scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroll.set_propagate_natural_height(true);
    scroll.set_propagate_natural_width(true);
    scroll.set_max_content_height(320);
    scroll.set_min_content_width(360);
    let heading = gtk::Label::new(Some(title));
    heading.add_css_class("detail");
    heading.set_xalign(0.0);
    heading.set_margin_start(8);
    heading.set_margin_top(2);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 4);
    content.append(&heading);
    content.append(&scroll);

    let popover = gtk::Popover::new();
    popover.add_css_class("tileide-completion");
    popover.add_css_class("tileide-pick");
    popover.set_has_arrow(false);
    popover.set_autohide(false);
    popover.set_can_focus(false);
    popover.set_position(gtk::PositionType::Bottom);
    popover.set_child(Some(&content));
    // Unparented when it closes; `lsp::release_popovers` covers a view closing first.
    popover.set_parent(view);
    let buffer = view.buffer();
    popover.set_pointing_to(Some(&crate::lsp::cursor_rect(view, &buffer.iter_at_mark(&buffer.get_insert()))));

    let on_pick = Rc::new(on_pick);
    let done = Rc::new(Cell::new(false));
    let pick = {
        let (popover, done, on_pick, shown) = (popover.downgrade(), done.clone(), on_pick.clone(), shown.clone());
        move |index: i32| {
            let Some(i) = usize::try_from(index).ok().and_then(|i| shown.borrow().get(i).copied()) else { return };
            if done.replace(true) {
                return;
            }
            if let Some(p) = popover.upgrade() {
                p.popdown();
            }
            on_pick(i);
        }
    };
    let query = Rc::new(RefCell::new(String::new()));
    let refilter: Refilter = {
        let (query, h, title) = (query.clone(), heading.downgrade(), title.to_string());
        Rc::new(move |q: &str| {
            *query.borrow_mut() = q.to_string();
            if let Some(h) = h.upgrade() {
                h.set_text(&if q.is_empty() { title.clone() } else { format!("{title} · {q}") });
            }
            fill(q)
        })
    };
    if filtered {
        let pick = pick.clone();
        TEST.with_borrow_mut(|t| *t = Some((refilter.clone(), Rc::new(pick))));
    }

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let (l, s, p) = (list.downgrade(), scroll.downgrade(), popover.downgrade());
    let pick_key = pick.clone();
    keys.connect_key_pressed(move |_, key, _, state| {
        let (Some(list), Some(scroll), Some(popover)) = (l.upgrade(), s.upgrade(), p.upgrade()) else { return glib::Propagation::Proceed };
        let current = list.selected_row().map_or(0, |r| r.index());
        let count = list.observe_children().n_items() as i32;
        let step = |by: i32| {
            if let Some(row) = list.row_at_index((current + by).rem_euclid(count.max(1))) {
                list.select_row(Some(&row));
                if let Some(b) = row.compute_bounds(&list) {
                    scroll.vadjustment().clamp_page(b.y() as f64, (b.y() + b.height()) as f64);
                }
            }
        };
        if filtered {
            let typed = key.to_unicode().filter(|c| !c.is_control() && !state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK | gdk::ModifierType::SUPER_MASK));
            let mut q = query.borrow().clone();
            if let Some(c) = typed {
                q.push(c);
            } else if key == gdk::Key::BackSpace && q.pop().is_none() {
                return glib::Propagation::Stop;
            }
            if typed.is_some() || key == gdk::Key::BackSpace {
                refilter(&q);
                return glib::Propagation::Stop;
            }
        }
        match key {
            gdk::Key::Down => step(1),
            gdk::Key::Up => step(-1),
            gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::Tab => pick_key(current),
            gdk::Key::Escape => popover.popdown(),
            gdk::Key::Shift_L | gdk::Key::Shift_R | gdk::Key::Control_L | gdk::Key::Control_R | gdk::Key::Alt_L | gdk::Key::Alt_R | gdk::Key::Super_L | gdk::Key::Super_R => return glib::Propagation::Proceed,
            // Any other key closes the list and goes on to the editor.
            _ => {
                popover.popdown();
                return glib::Propagation::Proceed;
            }
        }
        glib::Propagation::Stop
    });
    let focus = gtk::EventControllerFocus::new();
    let p = popover.downgrade();
    focus.connect_leave(move |_| {
        if let Some(p) = p.upgrade() {
            p.popdown();
        }
    });
    let pick_row = pick.clone();
    list.connect_row_activated(move |_, row| pick_row(row.index()));

    let controllers: [gtk::EventController; 2] = [keys.upcast(), focus.upcast()];
    for c in &controllers {
        view.add_controller(c.clone());
    }
    let v = view.downgrade();
    popover.connect_closed(move |p| {
        if filtered {
            TEST.with_borrow_mut(|t| *t = None);
        }
        let (p, v, controllers) = (p.clone(), v.clone(), controllers.clone());
        // After the close finishes, never during it.
        glib::idle_add_local_once(move || {
            if let Some(view) = v.upgrade() {
                for c in &controllers {
                    view.remove_controller(c);
                }
            }
            if p.parent().is_some() {
                p.unparent();
            }
        });
    });
    popover.popup();
}
