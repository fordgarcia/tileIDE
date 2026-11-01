// SPDX-License-Identifier: GPL-3.0-or-later
//! Missing-tool alerts (v0.4 phase 5): language servers, debuggers, Jupyter, Python
//! modules. They pop up on the banner unless Settings → Alerts turns them off; the app
//! sets the hook that decides.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::glib;

type Hook = Rc<dyn Fn(String)>;

thread_local! {
    static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
}

pub fn set_hook(hook: Hook) {
    HOOK.with_borrow_mut(|h| *h = Some(hook));
}

/// Reports a missing tool (after the caller's borrows end: the app may redraw).
pub fn missing(message: String) {
    eprintln!("missing: {message}");
    if let Some(hook) = HOOK.with_borrow(Clone::clone) {
        glib::idle_add_local_once(move || hook(message));
    }
}
