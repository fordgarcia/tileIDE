// SPDX-License-Identifier: GPL-3.0-or-later
//! TileIDE's own confirmation dialog (save changes, replace a terminal, quit), in the
//! quick-open card's style instead of the GTK theme's: a rounded card over a dimmed
//! window, a title, a muted detail line and pill buttons. Enter picks the default
//! button, Esc or a click on the dim cancels, Tab and ←/→ move between buttons.
//! One dialog at a time; `install` puts its layer on the window's overlay.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4 as gtk;
use tileide_core::theme::{self, Theme};

/// How a button reads: cancelling, losing something, or the safe default.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Role {
    Cancel,
    Destructive,
    Primary,
}

pub fn css(t: &Theme) -> String {
    let c = theme::css;
    format!(
        ".tileide-dialog {{ background-color: rgba(0, 0, 0, 0.45); }}
.tileide-dialog .card {{ background-color: {surface}; color: {text}; border-radius: 22px; border: 1px solid {edge}; padding: 22px 22px 18px 22px; box-shadow: 0 18px 48px rgba(0, 0, 0, 0.35); }}
.tileide-dialog .title {{ font-size: 17px; font-weight: 600; }}
.tileide-dialog .detail {{ font-size: 13px; color: {muted}; }}
.tileide-dialog button {{ border-radius: 999px; padding: 6px 18px; min-height: 0; font-weight: 500; box-shadow: none; border: 1px solid transparent; background: {quiet}; color: {text}; }}
.tileide-dialog button:hover {{ background: {quiet_hover}; }}
.tileide-dialog button:focus-visible {{ outline: 2px solid {accent}; outline-offset: 2px; }}
.tileide-dialog button.destructive {{ color: {error}; border-color: {error_edge}; background: transparent; }}
.tileide-dialog button.destructive:hover {{ background: {error_hover}; }}
.tileide-dialog button.primary {{ background: {accent}; color: {on_accent}; }}
.tileide-dialog button.primary:hover {{ background: {accent_hover}; }}",
        surface = c(t.surface, 0.98),
        text = c(t.text, 1.0),
        muted = c(t.muted, 1.0),
        edge = c(t.text, 0.10),
        quiet = c(t.text, 0.08),
        quiet_hover = c(t.text, 0.14),
        error = c(t.error, 1.0),
        error_edge = c(t.error, 0.5),
        error_hover = c(t.error, 0.12),
        accent = c(t.accent, 1.0),
        accent_hover = c(t.accent, 0.85),
        on_accent = c(t.on_accent, 1.0),
    )
}

type Answer = Box<dyn FnOnce(usize)>;

struct Host {
    layer: gtk::Box,
    title: gtk::Label,
    detail: gtk::Label,
    buttons: gtk::Box,
    /// The open dialog's answer; taken by the first choice, so it runs once.
    pending: RefCell<Option<Answer>>,
    /// Focus to give back when the dialog closes.
    restore: RefCell<Option<gtk::Widget>>,
}

thread_local! {
    static HOST: RefCell<Option<Rc<Host>>> = const { RefCell::new(None) };
}

/// Adds the (hidden) dialog layer on top of everything else in `overlay`.
pub fn install(overlay: &gtk::Overlay) {
    let layer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    layer.add_css_class("tileide-dialog");
    layer.set_visible(false);
    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.add_css_class("card");
    card.set_halign(gtk::Align::Center);
    card.set_valign(gtk::Align::Center);
    card.set_vexpand(true);
    card.set_size_request(420, -1);
    let title = gtk::Label::new(None);
    title.add_css_class("title");
    title.set_xalign(0.0);
    title.set_wrap(true);
    let detail = gtk::Label::new(None);
    detail.add_css_class("detail");
    detail.set_xalign(0.0);
    detail.set_wrap(true);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    buttons.set_margin_top(14);
    card.append(&title);
    card.append(&detail);
    card.append(&buttons);
    layer.append(&card);
    overlay.add_overlay(&layer);

    let host = Rc::new(Host { layer, title, detail, buttons, pending: RefCell::new(None), restore: RefCell::new(None) });

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let h = Rc::downgrade(&host);
    keys.connect_key_pressed(move |_, key, _, _| {
        let Some(h) = h.upgrade() else { return glib::Propagation::Proceed };
        let focus = |forward: bool| {
            h.buttons.child_focus(if forward { gtk::DirectionType::TabForward } else { gtk::DirectionType::TabBackward });
        };
        match key {
            gdk::Key::Escape => answer(&h, 0),
            gdk::Key::Right => focus(true),
            gdk::Key::Left => focus(false),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    host.layer.add_controller(keys);

    let click = gtk::GestureClick::new();
    let h = Rc::downgrade(&host);
    click.connect_released(move |gesture, _, x, y| {
        let Some(h) = h.upgrade() else { return };
        if h.layer.pick(x, y, gtk::PickFlags::DEFAULT).is_some_and(|w| w == h.layer) {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            answer(&h, 0);
        }
    });
    host.layer.add_controller(click);

    HOST.set(Some(host));
}

pub fn is_open() -> bool {
    HOST.with_borrow(|h| h.as_ref().is_some_and(|h| crate::motion::shown(&h.layer)))
}

/// Asks `title`. `buttons[0]` must be the cancel button (Esc and the dim pick it);
/// `default` gets keyboard focus, so Enter picks it. `on_choice` gets the button's
/// index, exactly once. Without an installed host it answers "cancel" at once.
pub fn ask(title: &str, detail: &str, buttons: &[(&str, Role)], default: usize, on_choice: impl FnOnce(usize) + 'static) {
    let Some(host) = HOST.with_borrow(|h| h.clone()) else {
        on_choice(0);
        return;
    };
    // A dialog that is still open is cancelled first, so its owner hears back.
    if host.pending.borrow().is_some() {
        answer(&host, 0);
    }
    host.title.set_text(title);
    host.detail.set_text(detail);
    host.detail.set_visible(!detail.is_empty());
    while let Some(child) = host.buttons.first_child() {
        host.buttons.remove(&child);
    }
    let mut focus = None;
    for (i, (label, role)) in buttons.iter().enumerate() {
        let button = gtk::Button::with_label(label);
        match role {
            Role::Cancel => {}
            Role::Destructive => button.add_css_class("destructive"),
            Role::Primary => button.add_css_class("primary"),
        }
        let h = Rc::downgrade(&host);
        button.connect_clicked(move |_| {
            if let Some(h) = h.upgrade() {
                answer(&h, i);
            }
        });
        host.buttons.append(&button);
        if i == default {
            focus = Some(button);
        }
    }
    *host.pending.borrow_mut() = Some(Box::new(on_choice));
    *host.restore.borrow_mut() = host.layer.root().and_then(|r| r.focus());
    crate::motion::fade_in(&host.layer);
    if let Some(button) = focus {
        button.grab_focus();
    }
}

fn answer(host: &Host, index: usize) {
    crate::motion::fade_out(&host.layer);
    if let Some(widget) = host.restore.take() {
        widget.grab_focus();
    }
    // Taken before calling, so the answer can open another dialog.
    let pending = host.pending.take();
    if let Some(on_choice) = pending {
        on_choice(index);
    }
}
