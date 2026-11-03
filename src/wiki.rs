// SPDX-License-Identifier: GPL-3.0-or-later
//! The Wikipedia rice's bars (v0.5 phase 3, `design/001/wikipedia-rice-design.png`):
//! a top bar (Files toggle, workspace title, file search, Settings and Keybinds links,
//! workspace-list toggle) and a workspace list on the right (one card per workspace,
//! New workspace, theme tray, Settings, clock). Files is the drawer on the left; the
//! frame (`draw`) leaves room for both via `token::top_inset` / `right_inset`.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk4 as gtk;
use tileide_core::theme::{self, Theme};
use tileide_core::token::{self, FRAME_BORDER, TOP_BAR, WORKSPACE_LIST_WIDTH};

use crate::workspaces::{self, Summary};

pub struct Callbacks {
    pub toggle_files: Rc<dyn Fn()>,
    pub search: Rc<dyn Fn()>,
    pub settings: Rc<dyn Fn()>,
    pub keybinds: Rc<dyn Fn()>,
    pub toggle_list: Rc<dyn Fn()>,
    pub go: Rc<dyn Fn(u8)>,
    pub new_workspace: Rc<dyn Fn()>,
    pub themes: Rc<dyn Fn()>,
}

pub struct Bars {
    top: gtk::Box,
    title: gtk::Label,
    list: gtk::Box,
    rows: gtk::Box,
    clock: gtk::Label,
    go: Rc<dyn Fn(u8)>,
    /// What the rows show now, so a redraw rebuilds them only on a change.
    shown: RefCell<Vec<(u8, String, usize, bool)>>,
}

pub fn css(t: &Theme) -> String {
    let c = theme::css;
    format!(
        "
.wiki-top, .wiki-list {{ font-family: \"{font}\"; font-size: 13px; color: {text}; }}
.wiki-top {{ padding: 0 {pad}px; }}
.wiki-top .title {{ font-size: 20px; font-weight: 600; margin: 0 18px 0 6px; }}
.wiki-top button, .wiki-list button {{ border: none; box-shadow: none; background: transparent; color: {muted}; border-radius: 8px; }}
.wiki-top button:hover, .wiki-list button:hover {{ background-color: {hover}; color: {text}; }}
.wiki-top button.search {{ min-width: 380px; padding: 6px 12px; border: 1px solid {edge}; background-color: {field}; color: {muted}; }}
.wiki-top button.link {{ color: {accent}; padding: 4px 8px; }}
.wiki-top .chip, .wiki-list .chip {{ font-size: 11px; color: {muted}; margin-left: 4px; }}
.wiki-list {{ padding: 16px 12px 12px 12px; }}
.wiki-list .heading {{ font-size: 17px; font-weight: 600; }}
.wiki-list button.pill {{ font-size: 11px; padding: 1px 8px; min-height: 0; border-radius: 9px; background-color: {hover}; }}
.wiki-list button.card {{ padding: 10px 12px; border: 1px solid {edge}; border-radius: 10px; color: {text}; }}
.wiki-list button.card.active {{ border-color: {accent}; background-color: {active}; }}
.wiki-list button.card .count {{ font-size: 11px; color: {muted}; }}
.wiki-list .swatch {{ min-width: 22px; min-height: 22px; border-radius: 5px; background-color: {swatch}; }}
.wiki-list button.card.active .swatch {{ background-color: {accent}; }}
.wiki-list button.new {{ padding: 8px 12px; border: 1px dashed {edge}; border-radius: 10px; }}
.wiki-list .clock {{ font-size: 15px; color: {text}; }}
",
        font = token::UI_FONT,
        pad = FRAME_BORDER,
        text = c(t.text, 1.0),
        muted = c(t.muted, 1.0),
        accent = c(t.accent, 1.0),
        hover = c(t.text, 0.07),
        edge = c(t.text, 0.12),
        field = c(t.text, 0.04),
        active = c(t.accent, 0.10),
        swatch = c(t.muted, 0.5),
    )
}

fn link(text: &str, keys: &str) -> gtk::Button {
    let b = gtk::Button::new();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.append(&gtk::Label::new(Some(text)));
    let chip = gtk::Label::new(Some(keys));
    chip.add_css_class("chip");
    row.append(&chip);
    b.set_child(Some(&row));
    b
}

fn icon(name: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(name);
    b.set_tooltip_text(Some(tooltip));
    b.set_valign(gtk::Align::Center);
    b
}

fn on_click(b: &gtk::Button, f: &Rc<dyn Fn()>) {
    let f = f.clone();
    b.connect_clicked(move |_| f());
}

impl Bars {
    pub fn new(overlay: &gtk::Overlay, cb: Callbacks) -> Rc<Bars> {
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        top.add_css_class("wiki-top");
        top.set_valign(gtk::Align::Start);
        top.set_size_request(-1, TOP_BAR as i32);
        let menu = icon("open-menu-symbolic", "Files (Alt+B)");
        on_click(&menu, &cb.toggle_files);
        top.append(&menu);
        let title = gtk::Label::new(None);
        title.add_css_class("title");
        top.append(&title);
        let search = gtk::Button::new();
        search.add_css_class("search");
        search.set_valign(gtk::Align::Center);
        let field = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        field.append(&gtk::Image::from_icon_name("system-search-symbolic"));
        let hint = gtk::Label::new(Some("Search files"));
        hint.set_hexpand(true);
        hint.set_xalign(0.0);
        field.append(&hint);
        let chip = gtk::Label::new(Some("Alt+E"));
        chip.add_css_class("chip");
        field.append(&chip);
        search.set_child(Some(&field));
        on_click(&search, &cb.search);
        top.append(&search);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        top.append(&spacer);
        for (b, f) in [(link("Settings", "Alt+,"), &cb.settings), (link("All keybinds", "Alt+K"), &cb.keybinds)] {
            b.add_css_class("link");
            b.set_valign(gtk::Align::Center);
            on_click(&b, f);
            top.append(&b);
        }
        let toggle = icon("sidebar-show-right-symbolic", "Workspaces");
        on_click(&toggle, &cb.toggle_list);
        top.append(&toggle);
        top.set_visible(false);
        overlay.add_overlay(&top);

        let list = gtk::Box::new(gtk::Orientation::Vertical, 8);
        list.add_css_class("wiki-list");
        list.set_halign(gtk::Align::End);
        list.set_size_request(WORKSPACE_LIST_WIDTH as i32, -1);
        list.set_margin_top(TOP_BAR as i32);
        list.set_margin_bottom(FRAME_BORDER as i32);
        list.set_margin_end(FRAME_BORDER as i32);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let heading = gtk::Label::new(Some("Workspaces"));
        heading.add_css_class("heading");
        head.append(&heading);
        let hide = gtk::Button::with_label("hide");
        hide.add_css_class("pill");
        hide.set_valign(gtk::Align::Center);
        on_click(&hide, &cb.toggle_list);
        head.append(&hide);
        list.append(&head);
        let rows = gtk::Box::new(gtk::Orientation::Vertical, 8);
        rows.set_margin_top(8);
        list.append(&rows);
        let add = gtk::Button::new();
        add.add_css_class("new");
        let add_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        add_row.append(&gtk::Label::new(Some("+")));
        add_row.append(&gtk::Label::new(Some("New workspace")));
        add.set_child(Some(&add_row));
        on_click(&add, &cb.new_workspace);
        list.append(&add);
        let fill = gtk::Box::new(gtk::Orientation::Vertical, 0);
        fill.set_vexpand(true);
        list.append(&fill);
        let foot = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let palette = icon("applications-graphics-symbolic", "Themes (Alt+T)");
        on_click(&palette, &cb.themes);
        foot.append(&palette);
        let gear = icon("emblem-system-symbolic", "Settings (Alt+,)");
        on_click(&gear, &cb.settings);
        foot.append(&gear);
        let clock = gtk::Label::new(None);
        clock.add_css_class("clock");
        clock.set_hexpand(true);
        clock.set_xalign(1.0);
        foot.append(&clock);
        list.append(&foot);
        list.set_visible(false);
        overlay.add_overlay(&list);

        Rc::new(Bars { top, title, list, rows, clock, go: cb.go, shown: RefCell::new(Vec::new()) })
    }

    /// Shows the bars (Wikipedia rice) or hides them; `list`: the workspace list too.
    pub fn show(&self, on: bool, list: bool) {
        self.top.set_visible(on);
        self.list.set_visible(on && list);
    }

    pub fn set_clock(&self, hours: &str, minutes: &str) {
        self.clock.set_text(&format!("{hours}:{minutes}"));
    }

    /// The title and the workspace cards; rebuilt only when something shown changed.
    pub fn refresh(&self, summaries: &[Summary], title: &str) {
        if self.title.text() != title {
            self.title.set_text(title);
        }
        let now: Vec<(u8, String, usize, bool)> = summaries.iter().map(|s| (s.number, name(s), s.tiles, s.active)).collect();
        if *self.shown.borrow() == now {
            return;
        }
        while let Some(child) = self.rows.first_child() {
            self.rows.remove(&child);
        }
        for (n, label, tiles, active) in &now {
            let card = gtk::Button::new();
            card.add_css_class("card");
            if *active {
                card.add_css_class("active");
            }
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let swatch = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            swatch.add_css_class("swatch");
            swatch.set_valign(gtk::Align::Center);
            row.append(&swatch);
            let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
            let title = gtk::Label::new(Some(label));
            title.set_xalign(0.0);
            title.set_ellipsize(gtk::pango::EllipsizeMode::End);
            text.append(&title);
            let count = gtk::Label::new(Some(&match tiles {
                1 => "1 tile".to_string(),
                n => format!("{n} tiles"),
            }));
            count.add_css_class("count");
            count.set_xalign(0.0);
            text.append(&count);
            row.append(&text);
            card.set_child(Some(&row));
            card.set_tooltip_text(Some(&format!("Alt+{}", workspaces::display(*n))));
            let (go, n) = (self.go.clone(), *n);
            card.connect_clicked(move |_| go(n));
            self.rows.append(&card);
        }
        *self.shown.borrow_mut() = now;
    }
}

/// A workspace card's title: its configured name, else "Workspace N".
pub fn name(s: &Summary) -> String {
    s.name.clone().unwrap_or_else(|| format!("Workspace {}", workspaces::display(s.number)))
}
