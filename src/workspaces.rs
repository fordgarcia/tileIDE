// SPDX-License-Identifier: GPL-3.0-or-later
//! Workspaces 1–10 (`Alt+1…9, 0`) plus the optional Config workspace (`Alt+C`).
//! Each one owns a `TileView`; only the active one is visible. A new TileIDE starts
//! with workspace 1 and no tiles (spec §6.2); nothing is restored from past sessions.
//! Each workspace has a root folder for the Files drawer: the opened folder for the
//! numbered ones, the config directory for Config.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk4 as gtk;

use crate::tile_view::{Kind, TileView};
use crate::tiles::Rect;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Active {
    Number(u8),
    Config,
}

pub const MAX: u8 = 10;

/// `Alt+0` is workspace 10, shown as "0".
pub fn from_digit(digit: u32) -> Option<u8> {
    match digit {
        0 => Some(10),
        1..=9 => Some(digit as u8),
        _ => None,
    }
}

pub fn display(n: u8) -> u8 {
    n % 10
}

pub struct Workspaces {
    numbered: BTreeMap<u8, Rc<RefCell<TileView>>>,
    config: Option<Rc<RefCell<TileView>>>,
    config_path: PathBuf,
    folder: PathBuf,
    active: Active,
    last_number: u8,
    /// The tile layer (below the drawer); each workspace adds its view to it.
    overlay: gtk::Overlay,
    area: Rect,
    on_change: Rc<dyn Fn()>,
    /// Called with the new workspace's root after every switch (re-roots the Files drawer).
    on_switch: Option<Rc<dyn Fn(&Path)>>,
    /// Alt+Z, shared by every editor in every workspace.
    wrap: bool,
}

impl Workspaces {
    pub fn new(overlay: gtk::Overlay, config_path: PathBuf, folder: PathBuf, on_change: Rc<dyn Fn()>) -> Workspaces {
        let mut w = Workspaces {
            numbered: BTreeMap::new(),
            config: None,
            config_path,
            folder,
            active: Active::Number(1),
            last_number: 1,
            overlay,
            area: Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 },
            on_change,
            on_switch: None,
            wrap: false,
        };
        w.go(1);
        w
    }

    pub fn active(&self) -> Active {
        self.active
    }

    pub fn set_on_switch(&mut self, on_switch: Rc<dyn Fn(&Path)>) {
        self.on_switch = Some(on_switch);
    }

    /// The folder the Files drawer shows for workspace `which`.
    pub fn root(&self, which: Active) -> PathBuf {
        match which {
            Active::Number(_) => self.folder.clone(),
            Active::Config => self.config_path.parent().map_or_else(|| self.folder.clone(), Path::to_path_buf),
        }
    }

    /// Alt+Z: toggles word wrap in all editors, including ones opened later.
    pub fn toggle_wrap(&mut self) {
        self.wrap = !self.wrap;
        for view in self.numbered.values().chain(self.config.iter()) {
            view.borrow_mut().set_wrap(self.wrap);
        }
    }

    pub fn numbers(&self) -> Vec<u8> {
        self.numbered.keys().copied().collect()
    }

    pub fn current(&self) -> Rc<RefCell<TileView>> {
        match self.active {
            Active::Config => self.config.clone().expect("config workspace exists while active"),
            Active::Number(n) => self.numbered[&n].clone(),
        }
    }

    fn new_view(&self) -> Rc<RefCell<TileView>> {
        let view = TileView::new(self.on_change.clone());
        view.borrow_mut().set_area(self.area);
        view.borrow_mut().set_wrap(self.wrap);
        self.overlay.add_overlay(view.borrow().widget());
        view
    }

    /// Goes to workspace `n`, creating it (empty) if it doesn't exist yet.
    pub fn go(&mut self, n: u8) {
        if !(1..=MAX).contains(&n) {
            return;
        }
        if !self.numbered.contains_key(&n) {
            let view = self.new_view();
            self.numbered.insert(n, view);
        }
        self.show(Active::Number(n));
    }

    /// The + button: the lowest number not in use, if any is left.
    pub fn create_next(&mut self) {
        if let Some(n) = (1..=MAX).find(|n| !self.numbered.contains_key(n)) {
            self.go(n);
        }
    }

    /// Alt+C: opens the Config workspace (with the config file) or returns to the last one.
    pub fn toggle_config(&mut self) {
        if self.active == Active::Config {
            let n = self.last_number;
            self.go(n);
            return;
        }
        if self.config.is_none() {
            let view = self.new_view();
            view.borrow_mut().add(Kind::Editor(Some(&self.config_path)), None);
            self.config = Some(view);
        }
        self.show(Active::Config);
    }

    fn show(&mut self, active: Active) {
        if let Active::Number(n) = active {
            self.last_number = n;
        }
        self.active = active;
        let current = self.current();
        for view in self.numbered.values().chain(self.config.iter()) {
            if !Rc::ptr_eq(view, &current) {
                view.borrow().set_visible(false);
            }
        }
        current.borrow().set_visible(true);
        if let Some(on_switch) = &self.on_switch {
            on_switch(&self.root(active));
        }
        (self.on_change)();
    }

    /// Every workspace gets the same code region, so switching never has to relayout.
    pub fn set_area(&mut self, area: Rect) {
        self.area = area;
        for view in self.numbered.values().chain(self.config.iter()) {
            view.borrow_mut().set_area(area);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_map_to_workspaces() {
        assert_eq!(from_digit(1), Some(1));
        assert_eq!(from_digit(9), Some(9));
        assert_eq!(from_digit(0), Some(10));
        assert_eq!(display(10), 0);
        assert_eq!(from_digit(11), None);
    }
}
