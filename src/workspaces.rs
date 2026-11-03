// SPDX-License-Identifier: GPL-3.0-or-later
//! Workspaces 1–10 (`Alt+1…9, 0`) plus the optional Config workspace (`Alt+C`).
//! Each one owns a `TileView`; only the active one is visible, and the others keep
//! running (terminals too). A new TileIDE starts with workspace 1 and no tiles
//! (spec §6.2). `tileide.workspace(n, { … })` gives a workspace its name, folder,
//! layout, theme and background.
//!
//! Folders (v0.3 phase 5): only the launch workspace gets the opened folder; other new
//! workspaces start without one until the folder search (`Alt+D`) sets it. Leaving a
//! workspace with no tiles and no folder removes it, unless it is the last one or
//! `tileide.lua` defines it. Numbers never shift: removing 3 leaves 1, 2, 4.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk4 as gtk;
use tileide_config::model::{Tiling, Workspace};
use tileide_core::layout::LayoutKind;
use tileide_core::theme::Theme;
use tileide_core::tiles::Rect;
use tileide_core::token::Rice;

use crate::tile_view::{Kind, TileView};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Active {
    Number(u8),
    Config,
}

pub const MAX: u8 = 10;

/// `Alt+0` is workspace 10, shown as "0".
pub fn display(n: u8) -> u8 {
    n % 10
}

/// What the app needs after a switch or a config change, so its callback never has to
/// borrow `Workspaces` again (it runs inside a `Workspaces` method).
pub struct Switch {
    /// `None`: a workspace without a folder.
    pub root: Option<PathBuf>,
    pub theme: Option<String>,
    pub background: Option<String>,
    /// A workspace just created without a folder: the app opens the folder search.
    pub ask_folder: bool,
    /// The active numbered workspace (`None` in Config), for per-workspace picks.
    pub number: Option<u8>,
}

/// Gets the new state; returns a theme when the active theme changed.
pub type OnSwitch = Rc<dyn Fn(&Switch) -> Option<Rc<Theme>>>;

/// One line of the overview.
pub struct Summary {
    pub number: u8,
    pub name: Option<String>,
    pub folder: Option<PathBuf>,
    pub tiles: usize,
    pub layout: LayoutKind,
    pub active: bool,
}

pub struct Workspaces {
    numbered: BTreeMap<u8, Rc<RefCell<TileView>>>,
    config: Option<Rc<RefCell<TileView>>>,
    config_path: PathBuf,
    /// The folder TileIDE was opened on.
    folder: PathBuf,
    /// The workspace that shows `folder` until it is removed.
    launch: Option<u8>,
    /// Folders set with the folder search; they win over the config's.
    picked: BTreeMap<u8, PathBuf>,
    /// Set when `go` creates a workspace without a folder; taken by the next switch notice.
    fresh: bool,
    active: Active,
    last_number: u8,
    /// The tile layer (below the drawer); each workspace adds its view to it.
    overlay: gtk::Overlay,
    area: Rect,
    stage: Rect,
    gap: f64,
    tiling: Tiling,
    settings: BTreeMap<u8, Workspace>,
    theme: Rc<Theme>,
    on_change: Rc<dyn Fn()>,
    /// Called after every switch (re-roots the Files drawer, applies overrides).
    on_switch: Option<OnSwitch>,
    /// Alt+Z, shared by every editor in every workspace.
    wrap: bool,
    /// `appearance.rice`: the Wikipedia rice stacks every workspace's tiles.
    rice: Rice,
}

/// `~/x` → `$HOME/x`.
pub fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => Path::new(&home).join(rest),
        (None, Some(home)) if path == "~" => PathBuf::from(home),
        _ => PathBuf::from(path),
    }
}

impl Workspaces {
    pub fn new(overlay: gtk::Overlay, config_path: PathBuf, folder: PathBuf, tiling: Tiling, theme: Rc<Theme>, on_change: Rc<dyn Fn()>) -> Workspaces {
        let mut w = Workspaces {
            numbered: BTreeMap::new(),
            config: None,
            config_path,
            folder,
            launch: Some(1),
            picked: BTreeMap::new(),
            fresh: false,
            active: Active::Number(1),
            last_number: 1,
            overlay,
            area: Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 },
            stage: Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 },
            gap: tiling.gaps_in as f64,
            tiling,
            settings: BTreeMap::new(),
            theme,
            on_change,
            on_switch: None,
            wrap: false,
            rice: Rice::Default,
        };
        w.go(1);
        w
    }

    pub fn active(&self) -> Active {
        self.active
    }

    pub fn set_on_switch(&mut self, on_switch: OnSwitch) {
        self.on_switch = Some(on_switch);
    }

    pub fn switch_info(&self) -> Switch {
        let (theme, background) = self.overrides();
        let number = match self.active {
            Active::Number(n) => Some(n),
            Active::Config => None,
        };
        Switch { root: self.folder(self.active), theme, background, ask_folder: false, number }
    }

    fn notify_switch(&mut self) {
        let Some(on_switch) = self.on_switch.clone() else { return };
        let info = Switch { ask_folder: std::mem::take(&mut self.fresh), ..self.switch_info() };
        if let Some(theme) = on_switch(&info) {
            self.set_theme(theme);
        }
    }

    /// Applies `tileide.tiling` and `tileide.workspace(…)` after a (re)load. Existing
    /// workspaces keep their layout; new ones start with the configured one.
    pub fn configure(&mut self, tiling: Tiling, settings: BTreeMap<u8, Workspace>) {
        self.gap = tiling.gaps_in as f64;
        self.settings = settings;
        for (&n, view) in &self.numbered {
            let mut view = view.borrow_mut();
            view.set_gap(self.gap);
            view.set_column_width(tiling.column_width);
            view.set_max_tiles(tiling.max_tiles);
            view.set_peek(tiling.peek);
            view.set_folder(self.folder(Active::Number(n)));
        }
        if let Some(view) = &self.config {
            view.borrow_mut().set_gap(self.gap);
            view.borrow_mut().set_max_tiles(tiling.max_tiles);
            view.borrow_mut().set_peek(tiling.peek);
        }
        self.tiling = tiling;
        self.notify_switch();
        (self.on_change)();
    }

    /// The folder the Files drawer shows for workspace `which`: the picked one, the
    /// config's, or the opened folder (launch workspace only). `None` = no folder yet.
    pub fn folder(&self, which: Active) -> Option<PathBuf> {
        match which {
            Active::Number(n) => self
                .picked
                .get(&n)
                .cloned()
                .or_else(|| self.settings.get(&n).and_then(|w| w.folder.as_deref()).map(expand_home).filter(|p| p.is_dir()))
                .or_else(|| (self.launch == Some(n)).then(|| self.folder.clone())),
            Active::Config => Some(self.config_dir()),
        }
    }

    /// Where workspace `which`'s terminals start: its folder, else home.
    pub fn root(&self, which: Active) -> PathBuf {
        self.folder(which).unwrap_or_else(|| expand_home("~"))
    }

    /// The folder search's pick for the active workspace (the Config workspace keeps its own).
    pub fn set_folder(&mut self, folder: PathBuf) {
        let Active::Number(n) = self.active else { return };
        self.picked.insert(n, folder);
        self.current().borrow_mut().set_folder(self.folder(self.active));
        crate::lsp::retain_roots(&self.folders());
        self.notify_switch();
        (self.on_change)();
    }

    /// Unsaved editors and open terminals in workspace `n` (what removing it would end).
    pub fn work(&self, n: u8) -> (usize, usize) {
        self.numbered.get(&n).map_or((0, 0), |v| (v.borrow().dirty_count(), v.borrow().terminal_count()))
    }

    /// Removes workspace `n`. The active one hands over to its nearest neighbour (the
    /// lower one first); removing the last one leaves a fresh, empty workspace.
    pub fn remove(&mut self, n: u8) {
        if !self.numbered.contains_key(&n) {
            return;
        }
        self.discard(n);
        if self.active != Active::Number(n) {
            (self.on_change)();
            return;
        }
        let next = self.numbered.range(..n).next_back().or_else(|| self.numbered.range(n..).next()).map(|(&k, _)| k);
        match next {
            Some(k) => self.show(Active::Number(k)),
            None => self.create_next(),
        }
    }

    /// Drops workspace `n`'s view: it fades out, then leaves the tile layer.
    fn discard(&mut self, n: u8) {
        let Some(view) = self.numbered.remove(&n) else { return };
        self.picked.remove(&n);
        if self.launch == Some(n) {
            self.launch = None;
        }
        view.borrow().release();
        let widget = view.borrow().widget().clone();
        let (overlay, w) = (self.overlay.clone(), widget.clone());
        crate::motion::fade_out_then(&widget, move || overlay.remove_overlay(&w));
        drop(view);
        crate::lsp::retain_roots(&self.folders());
    }

    fn config_dir(&self) -> PathBuf {
        self.config_path.parent().map_or_else(|| self.folder.clone(), Path::to_path_buf)
    }

    pub fn name(&self, n: u8) -> Option<String> {
        self.settings.get(&n).and_then(|w| w.name.clone())
    }

    /// The active workspace's theme and background overrides.
    pub fn overrides(&self) -> (Option<String>, Option<String>) {
        match self.active {
            Active::Number(n) => self.settings.get(&n).map_or((None, None), |w| (w.theme.clone(), w.background.clone())),
            Active::Config => (None, None),
        }
    }

    /// Alt+Z: toggles word wrap in all editors, including ones opened later.
    pub fn toggle_wrap(&mut self) {
        self.wrap = !self.wrap;
        for view in self.views() {
            view.borrow_mut().set_wrap(self.wrap);
        }
    }

    pub fn set_theme(&mut self, theme: Rc<Theme>) {
        crate::lsp::set_theme(theme.clone());
        for view in self.views() {
            view.borrow_mut().set_theme(theme.clone());
        }
        self.theme = theme;
    }

    fn views(&self) -> impl Iterator<Item = &Rc<RefCell<TileView>>> {
        self.numbered.values().chain(self.config.iter())
    }

    /// Unsaved editors across every workspace.
    pub fn dirty_count(&self) -> usize {
        self.views().map(|v| v.borrow().dirty_count()).sum()
    }

    pub fn numbers(&self) -> Vec<u8> {
        self.numbered.keys().copied().collect()
    }

    pub fn summaries(&self) -> Vec<Summary> {
        self.numbered
            .iter()
            .map(|(&n, view)| {
                let view = view.borrow();
                Summary {
                    number: n,
                    name: self.name(n),
                    folder: self.folder(Active::Number(n)),
                    tiles: view.tile_count(),
                    layout: view.layout_kind(),
                    active: self.active == Active::Number(n),
                }
            })
            .collect()
    }

    pub fn current(&self) -> Rc<RefCell<TileView>> {
        match self.active {
            Active::Config => self.config.clone().expect("config workspace exists while active"),
            Active::Number(n) => self.numbered[&n].clone(),
        }
    }

    /// Every workspace's folder (language servers for other folders can stop).
    fn folders(&self) -> Vec<PathBuf> {
        self.numbered.keys().map(|&n| Active::Number(n)).chain(self.config.iter().map(|_| Active::Config)).filter_map(|w| self.folder(w)).collect()
    }

    /// The layout workspace `n` (`None` = Config) starts with in the current rice.
    fn layout_for(&self, n: Option<u8>) -> LayoutKind {
        match (self.rice, n) {
            (Rice::Wikipedia, _) => LayoutKind::Stack,
            (Rice::Default, Some(n)) => self.settings.get(&n).and_then(|w| w.layout).unwrap_or(self.tiling.layout),
            (Rice::Default, None) => LayoutKind::Dwindle,
        }
    }

    fn apply_rice(&self, view: &Rc<RefCell<TileView>>, n: Option<u8>) {
        let wiki = self.rice == Rice::Wikipedia;
        let scrolling = if wiki { LayoutKind::Stack } else { LayoutKind::Columns };
        view.borrow_mut().set_rice(scrolling, self.layout_for(n), wiki);
    }

    /// Switching rice puts every workspace in that rice's layout (the stack, or back to
    /// the configured one).
    pub fn set_rice(&mut self, rice: Rice) {
        if rice == self.rice {
            return;
        }
        self.rice = rice;
        for (&n, view) in &self.numbered {
            self.apply_rice(view, Some(n));
        }
        if let Some(view) = &self.config {
            self.apply_rice(view, None);
        }
        (self.on_change)();
    }

    fn new_view(&self, layout: LayoutKind, folder: Option<PathBuf>) -> Rc<RefCell<TileView>> {
        let view = TileView::new(layout, self.tiling.column_width, self.gap, self.theme.clone(), folder, self.on_change.clone());
        view.borrow_mut().set_area(self.area, self.stage);
        view.borrow_mut().set_wrap(self.wrap);
        view.borrow_mut().set_max_tiles(self.tiling.max_tiles);
        view.borrow_mut().set_peek(self.tiling.peek);
        self.overlay.add_overlay(view.borrow().widget());
        view
    }

    /// Goes to workspace `n`, creating it (empty) if it doesn't exist yet.
    pub fn go(&mut self, n: u8) {
        if !(1..=MAX).contains(&n) {
            return;
        }
        if !self.numbered.contains_key(&n) {
            let view = self.new_view(self.layout_for(Some(n)), self.folder(Active::Number(n)));
            self.apply_rice(&view, Some(n));
            self.numbered.insert(n, view);
            self.fresh = self.folder(Active::Number(n)).is_none();
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
            let view = self.new_view(self.layout_for(None), Some(self.config_dir()));
            self.apply_rice(&view, None);
            view.borrow_mut().add(Kind::Editor(Some(&self.config_path)));
            self.config = Some(view);
        }
        self.show(Active::Config);
    }

    fn show(&mut self, active: Active) {
        let left = self.active;
        if let Active::Number(n) = active {
            self.last_number = n;
        }
        self.active = active;
        // Leaving an empty workspace removes it (not on the way to Config: Alt+C returns).
        // A folder counts as work: a workspace with one but no tiles stays.
        if let (Active::Number(p), Active::Number(_)) = (left, active)
            && left != active
            && self.numbered.len() > 1
            && !self.settings.contains_key(&p)
            && self.folder(left).is_none()
            && self.numbered.get(&p).is_some_and(|v| v.borrow().tile_count() == 0)
        {
            self.discard(p);
        }
        let current = self.current();
        for view in self.views() {
            if !Rc::ptr_eq(view, &current) {
                view.borrow().set_visible(false);
            }
        }
        current.borrow().set_visible(true);
        self.notify_switch();
        (self.on_change)();
    }

    /// Every workspace gets the same code region, so switching never has to relayout.
    pub fn set_area(&mut self, area: Rect, stage: Rect) {
        self.area = area;
        self.stage = stage;
        for view in self.views() {
            view.borrow_mut().set_area(area, stage);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_is_expanded() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(expand_home("~/notes"), Path::new(&home).join("notes"));
        assert_eq!(expand_home("/abs"), PathBuf::from("/abs"));
        assert_eq!(expand_home("~"), PathBuf::from(home));
    }
}
