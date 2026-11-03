// SPDX-License-Identifier: GPL-3.0-or-later
//! The workspace overview (Alt+O): the window dims and shows one card per workspace
//! (number, name, folder, tile count, layout). A card, its digit or Enter switches;
//! Esc or a click on the dim closes.

use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4 as gtk;
use tileide_core::layout::LayoutKind;
use tileide_core::theme::{self, Theme};

use crate::workspaces::{self, Summary};

pub fn css(t: &Theme) -> String {
    let c = theme::css;
    format!(
        ".overview {{ background-color: rgba(0, 0, 0, 0.6); }}
.overview flowbox, .overview flowboxchild {{ background: transparent; padding: 0; }}
.overview button.card {{ background-color: {surface}; color: {text}; border-radius: 14px; border: 2px solid {edge}; padding: 14px 18px; min-width: 180px; box-shadow: none; }}
.overview button.card:hover, .overview button.card:focus {{ border-color: {accent}; }}
.overview button.card.active {{ border-color: {accent}; }}
.overview .number {{ font-size: 22px; font-weight: 600; color: {accent}; }}
.overview .name {{ font-size: 15px; font-weight: 500; }}
.overview .detail {{ font-size: 12px; color: {muted}; }}
.overview button.remove {{ min-width: 24px; min-height: 24px; padding: 0; margin: 8px; border-radius: 12px; border: none; box-shadow: none; background: transparent; color: {muted}; }}
.overview button.remove:hover {{ background-color: {remove_hover}; color: {error}; }}",
        surface = c(t.surface, 0.96),
        text = c(t.text, 1.0),
        muted = c(t.muted, 1.0),
        edge = c(t.text, 0.1),
        accent = c(t.accent, 1.0),
        error = c(t.error, 1.0),
        remove_hover = c(t.error, 0.14),
    )
}

pub struct Overview {
    root: gtk::Box,
    cards: gtk::FlowBox,
}

impl Overview {
    /// `on_pick` gets the chosen workspace number; `on_close` runs on every close.
    pub fn new(overlay: &gtk::Overlay, on_pick: Rc<dyn Fn(u8)>, on_close: Rc<dyn Fn()>) -> Rc<Overview> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("overview");
        root.set_visible(false);
        let cards = gtk::FlowBox::new();
        cards.set_selection_mode(gtk::SelectionMode::None);
        cards.set_halign(gtk::Align::Center);
        cards.set_valign(gtk::Align::Center);
        cards.set_vexpand(true);
        cards.set_max_children_per_line(5);
        cards.set_column_spacing(16);
        cards.set_row_spacing(16);
        root.append(&cards);
        overlay.add_overlay(&root);
        let view = Rc::new(Overview { root, cards });

        let keys = gtk::EventControllerKey::new();
        let (v, pick, close) = (Rc::downgrade(&view), on_pick.clone(), on_close.clone());
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(v) = v.upgrade() else { return glib::Propagation::Proceed };
            if key == gdk::Key::Escape {
                v.close();
                close();
                return glib::Propagation::Stop;
            }
            match key.to_unicode().and_then(|c| c.to_digit(10)) {
                Some(d) => {
                    v.close();
                    pick(if d == 0 { 10 } else { d as u8 });
                    glib::Propagation::Stop
                }
                None => glib::Propagation::Proceed,
            }
        });
        view.root.add_controller(keys);

        // Buttons claim their own clicks, so this only sees clicks on the dim.
        let click = gtk::GestureClick::new();
        let (v, close) = (Rc::downgrade(&view), on_close.clone());
        click.connect_released(move |gesture, _, x, y| {
            let Some(v) = v.upgrade() else { return };
            if v.root.pick(x, y, gtk::PickFlags::DEFAULT).is_some_and(|w| w.ancestor(gtk::Button::static_type()).is_none()) {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                v.close();
                close();
            }
        });
        view.root.add_controller(click);

        view
    }

    pub fn is_open(&self) -> bool {
        crate::motion::shown(&self.root)
    }

    /// `on_remove` runs from a card's × button (the app asks, removes, then reopens this).
    pub fn open(&self, summaries: &[Summary], on_pick: Rc<dyn Fn(u8)>, on_remove: Rc<dyn Fn(u8)>) {
        while let Some(child) = self.cards.first_child() {
            self.cards.remove(&child);
        }
        let mut focus = None;
        for s in summaries {
            let card = gtk::Button::new();
            card.add_css_class("card");
            if s.active {
                card.add_css_class("active");
            }
            let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
            let number = gtk::Label::new(Some(&workspaces::display(s.number).to_string()));
            number.add_css_class("number");
            number.set_xalign(0.0);
            let folder_name = match &s.folder {
                Some(f) => f.file_name().map_or_else(|| "/".into(), |n| n.to_string_lossy().into_owned()),
                None => "No folder".into(),
            };
            let name = gtk::Label::new(Some(s.name.as_deref().unwrap_or(&folder_name)));
            name.add_css_class("name");
            name.set_xalign(0.0);
            let tiles = match s.tiles {
                0 => "empty".to_string(),
                1 => "1 tile".to_string(),
                n => format!("{n} tiles"),
            };
            let layout = match s.layout {
                LayoutKind::Dwindle => "dwindle",
                LayoutKind::Columns => "columns",
                LayoutKind::Stack => "stack",
            };
            let folder = s.folder.as_ref().map_or_else(|| "Alt+D picks one".into(), |f| f.display().to_string());
            let detail = gtk::Label::new(Some(&format!("{tiles} · {layout}\n{folder}")));
            detail.add_css_class("detail");
            detail.set_xalign(0.0);
            detail.set_ellipsize(gtk::pango::EllipsizeMode::Start);
            detail.set_max_width_chars(28);
            column.append(&number);
            column.append(&name);
            column.append(&detail);
            card.set_child(Some(&column));
            let (pick, n, root) = (on_pick.clone(), s.number, self.root.clone());
            card.connect_clicked(move |_| {
                crate::motion::fade_out(&root);
                pick(n);
            });
            // A sibling over the card's corner: a button can't hold another button.
            let remove = gtk::Button::from_icon_name("window-close-symbolic");
            remove.add_css_class("remove");
            remove.set_tooltip_text(Some("Remove workspace (Alt+Shift+W)"));
            remove.set_halign(gtk::Align::End);
            remove.set_valign(gtk::Align::Start);
            let (on_remove, n) = (on_remove.clone(), s.number);
            remove.connect_clicked(move |_| on_remove(n));
            let corner = gtk::Overlay::new();
            corner.set_child(Some(&card));
            corner.add_overlay(&remove);
            self.cards.append(&corner);
            if s.active {
                focus = Some(card);
            }
        }
        crate::motion::fade_in(&self.root);
        match focus {
            Some(card) => card.grab_focus(),
            None => self.root.grab_focus(),
        };
    }

    pub fn close(&self) {
        crate::motion::fade_out(&self.root);
    }
}
