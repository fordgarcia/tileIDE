// SPDX-License-Identifier: GPL-3.0-or-later
//! Smooth animation on by default (`tileide.appearance { animations = false }` turns it
//! off, as does GTK's `gtk-enable-animations`). Timing: `tileide_core::motion`.

use std::cell::{Cell, RefCell};

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;
use tileide_core::motion::{self, Fade};

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(true) };
}

/// Object data key holding a widget's running fade, so a new fade replaces it.
const FADE_KEY: &str = "tileide-fade";

/// From the config; the desktop's own "no animations" setting also wins.
pub fn set_enabled(config: bool) {
    let desktop = gtk::Settings::default().is_none_or(|s| s.is_gtk_enable_animations());
    ENABLED.set(config && desktop);
}

pub fn enabled() -> bool {
    ENABLED.get()
}

/// Visible and not on its way out (`fade_out` keeps a widget visible while it fades).
pub fn shown(widget: &impl IsA<gtk::Widget>) -> bool {
    let widget = widget.as_ref();
    widget.is_visible() && widget.can_target()
}

/// Shows `widget` and fades it in, from where a running fade-out has left it.
pub fn fade_in(widget: &impl IsA<gtk::Widget>) {
    let widget = widget.as_ref();
    let from = if widget.is_visible() { widget.opacity() } else { 0.0 };
    widget.set_can_target(true);
    widget.set_visible(true);
    run(widget, Fade::new(from, 1.0), || {});
}

/// Fades `widget` out, then hides it. It stops taking clicks at once; callers move
/// keyboard focus themselves.
pub fn fade_out(widget: &impl IsA<gtk::Widget>) {
    let w = widget.as_ref().clone();
    fade_out_then(widget, move || w.set_visible(false));
}

/// Fades `widget` out, then runs `then` (which should hide or remove it).
pub fn fade_out_then(widget: &impl IsA<gtk::Widget>, then: impl FnOnce() + 'static) {
    let widget = widget.as_ref();
    if !widget.is_visible() {
        cancel(widget);
        then();
        return;
    }
    widget.set_can_target(false);
    run(widget, Fade::new(widget.opacity(), 0.0), then);
}

/// Theme changes: lays a still of the current frame over the whole window, then fades
/// it away, so the new colours dissolve in instead of snapping. `cover` is a hidden,
/// non-targetable picture on top of the window's overlay.
pub fn crossfade(cover: &gtk::Picture) {
    let Some(root) = cover.parent() else { return };
    if !enabled() || !root.is_mapped() {
        return;
    }
    let still = gtk::WidgetPaintable::new(Some(&root)).current_image();
    cover.set_paintable(Some(&still));
    cover.set_opacity(1.0);
    cover.set_visible(true);
    let c = cover.clone();
    run(cover.upcast_ref(), Fade::with_duration(1.0, 0.0, motion::FADE_IN_US), move || {
        c.set_visible(false);
        c.set_paintable(None::<&gtk::gdk::Paintable>);
    });
}

fn run(widget: &gtk::Widget, fade: Fade, then: impl FnOnce() + 'static) {
    cancel(widget);
    if !enabled() {
        widget.set_opacity(fade.at(i64::MAX).0);
        then();
        return;
    }
    widget.set_opacity(fade.at(0).0);
    let start = Cell::new(None);
    let then = RefCell::new(Some(then));
    let id = widget.add_tick_callback(move |w, clock| {
        let now = clock.frame_time();
        let begun = start.get().unwrap_or(now);
        start.set(Some(begun));
        let (opacity, done) = fade.at(now - begun);
        w.set_opacity(opacity);
        if !done {
            return glib::ControlFlow::Continue;
        }
        // SAFETY: FADE_KEY only ever holds a `TickCallbackId`; taken so `cancel` won't
        // remove a callback that is already finishing.
        let _ = unsafe { w.steal_data::<gtk::TickCallbackId>(FADE_KEY) };
        if let Some(then) = then.take() {
            then();
        }
        glib::ControlFlow::Break
    });
    // SAFETY: as above.
    unsafe { widget.set_data(FADE_KEY, id) };
}

/// Stops a running fade where it is (its `then` is dropped: the new fade decides).
fn cancel(widget: &gtk::Widget) {
    // SAFETY: FADE_KEY only ever holds a `TickCallbackId`.
    if let Some(id) = unsafe { widget.steal_data::<gtk::TickCallbackId>(FADE_KEY) } {
        id.remove();
    }
}
