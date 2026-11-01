// SPDX-License-Identifier: GPL-3.0-or-later
//! One workspace's layout: dwindle (`TileTree`) or scrolling columns (`Columns`).
//! Tile ids are allocated here, so switching between the two keeps every tile.

use crate::columns::Columns;
use crate::tiles::{Axis, Direction, Rect, TileId, TileTree};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutKind {
    Dwindle,
    Columns,
}

#[derive(Debug)]
enum Inner {
    Dwindle(TileTree),
    Columns(Columns),
}

#[derive(Debug)]
pub struct Layout {
    inner: Inner,
    next_id: TileId,
    /// Width of new columns, as a fraction of the code region.
    column_width: f64,
    /// Set by `split`: the axis the next new tile uses (`Column` stacks in columns).
    pub pending_axis: Option<Axis>,
}

/// Keyboard resize step: a fraction of the parent split (dwindle) or of the region (columns).
const RESIZE_STEP: f64 = 0.05;

impl Layout {
    pub fn new(kind: LayoutKind, column_width: f64) -> Layout {
        let inner = match kind {
            LayoutKind::Dwindle => Inner::Dwindle(TileTree::default()),
            LayoutKind::Columns => Inner::Columns(Columns::default()),
        };
        Layout { inner, next_id: 0, column_width, pending_axis: None }
    }

    pub fn kind(&self) -> LayoutKind {
        match self.inner {
            Inner::Dwindle(_) => LayoutKind::Dwindle,
            Inner::Columns(_) => LayoutKind::Columns,
        }
    }

    pub fn set_column_width(&mut self, width: f64) {
        self.column_width = width;
    }

    pub fn focused(&self) -> Option<TileId> {
        match &self.inner {
            Inner::Dwindle(t) => t.focused(),
            Inner::Columns(c) => c.focused(),
        }
    }

    pub fn focus(&mut self, id: TileId, area: Rect, gap: f64) {
        match &mut self.inner {
            Inner::Dwindle(t) => t.focus(id),
            Inner::Columns(c) => {
                c.focus(id);
                c.ensure_visible(area, gap);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        match &self.inner {
            Inner::Dwindle(t) => t.is_empty(),
            Inner::Columns(c) => c.is_empty(),
        }
    }

    /// The scroll offset the columns layout wants (0 for dwindle).
    pub fn scroll(&self) -> f64 {
        match &self.inner {
            Inner::Dwindle(_) => 0.0,
            Inner::Columns(c) => c.scroll(),
        }
    }

    /// After the code region changed size: keeps the focused column in sight.
    pub fn fit(&mut self, area: Rect, gap: f64) {
        if let Inner::Columns(c) = &mut self.inner {
            c.ensure_visible(area, gap);
        }
    }

    /// Adds a tile next to the focused one, focuses it and returns its id.
    pub fn add(&mut self, area: Rect, gap: f64) -> TileId {
        let id = self.next_id;
        self.next_id += 1;
        let axis = self.pending_axis.take();
        match &mut self.inner {
            Inner::Dwindle(t) => {
                let rect = t.layout(area, gap).into_iter().find(|(i, _)| Some(*i) == t.focused()).map(|(_, r)| r);
                t.split(id, axis, rect);
            }
            Inner::Columns(c) => {
                c.insert(id, axis == Some(Axis::Column), self.column_width);
                c.ensure_visible(area, gap);
            }
        }
        id
    }

    pub fn close(&mut self, id: TileId, area: Rect, gap: f64) -> bool {
        match &mut self.inner {
            Inner::Dwindle(t) => t.close(id),
            Inner::Columns(c) => {
                let closed = c.close(id);
                c.ensure_visible(area, gap);
                closed
            }
        }
    }

    /// Rects at scroll offset `scroll` (columns only; see `scroll()`).
    pub fn layout(&self, area: Rect, gap: f64, scroll: f64) -> Vec<(TileId, Rect)> {
        match &self.inner {
            Inner::Dwindle(t) => t.layout(area, gap),
            Inner::Columns(c) => c.layout(area, gap, scroll),
        }
    }

    pub fn focus_direction(&mut self, direction: Direction, area: Rect, gap: f64) {
        match &mut self.inner {
            Inner::Dwindle(t) => t.focus_direction(direction, area, gap),
            Inner::Columns(c) => {
                c.focus_direction(direction);
                c.ensure_visible(area, gap);
            }
        }
    }

    pub fn resize_focused(&mut self, direction: Direction, area: Rect, gap: f64) {
        match &mut self.inner {
            Inner::Dwindle(t) => t.resize_focused(direction, RESIZE_STEP),
            Inner::Columns(c) => {
                c.resize_focused(direction, RESIZE_STEP * 2.0);
                c.ensure_visible(area, gap);
            }
        }
    }

    pub fn move_focused(&mut self, direction: Direction, area: Rect, gap: f64) {
        match &mut self.inner {
            Inner::Dwindle(t) => t.move_focused(direction, area, gap),
            Inner::Columns(c) => {
                c.move_focused(direction);
                c.ensure_visible(area, gap);
            }
        }
    }

    /// Switches layout keeping every tile and the focus. Order: reading order of the
    /// current layout. Split ratios and column widths are not carried over.
    pub fn set_kind(&mut self, kind: LayoutKind, area: Rect, gap: f64) {
        if kind == self.kind() {
            return;
        }
        let ids = match &self.inner {
            Inner::Dwindle(t) => t.ids(),
            Inner::Columns(c) => c.ids(),
        };
        let focused = self.focused();
        self.inner = match kind {
            LayoutKind::Dwindle => Inner::Dwindle(TileTree::from_ids(&ids, focused, area, gap)),
            LayoutKind::Columns => {
                let mut c = Columns::from_ids(&ids, focused, self.column_width);
                c.ensure_visible(area, gap);
                Inner::Columns(c)
            }
        };
    }

    pub fn toggle_kind(&mut self, area: Rect, gap: f64) {
        let next = match self.kind() {
            LayoutKind::Dwindle => LayoutKind::Columns,
            LayoutKind::Columns => LayoutKind::Dwindle,
        };
        self.set_kind(next, area, gap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect { x: 0.0, y: 0.0, w: 1000.0, h: 600.0 };

    #[test]
    fn toggling_keeps_tiles_order_and_focus() {
        let mut l = Layout::new(LayoutKind::Dwindle, 0.5);
        let ids: Vec<_> = (0..3).map(|_| l.add(AREA, 10.0)).collect();
        l.focus(ids[1], AREA, 10.0);
        l.toggle_kind(AREA, 10.0);
        assert_eq!(l.kind(), LayoutKind::Columns);
        assert_eq!(l.focused(), Some(ids[1]));
        let order: Vec<_> = l.layout(AREA, 10.0, 0.0).iter().map(|r| r.0).collect();
        assert_eq!(order, ids, "dwindle reading order becomes column order");
        l.toggle_kind(AREA, 10.0);
        assert_eq!(l.kind(), LayoutKind::Dwindle);
        assert_eq!(l.layout(AREA, 10.0, 0.0).len(), 3);
        assert_eq!(l.focused(), Some(ids[1]));
    }

    #[test]
    fn narrowing_the_region_keeps_the_focused_column_visible() {
        let mut l = Layout::new(LayoutKind::Columns, 0.5);
        let wide = Rect { w: 2000.0, ..AREA };
        for _ in 0..3 {
            l.add(wide, 10.0);
        }
        let narrow = Rect { w: 300.0, ..AREA };
        l.fit(narrow, 10.0);
        let focused = l.focused().unwrap();
        let r = l.layout(narrow, 10.0, l.scroll()).into_iter().find(|(id, _)| *id == focused).unwrap().1;
        assert!(r.x >= narrow.x && r.x + r.w <= narrow.x + narrow.w, "{r:?} outside {narrow:?}");
    }

    #[test]
    fn ids_stay_unique_across_layouts() {
        let mut l = Layout::new(LayoutKind::Columns, 0.5);
        let a = l.add(AREA, 10.0);
        l.toggle_kind(AREA, 10.0);
        let b = l.add(AREA, 10.0);
        assert_ne!(a, b);
    }

    #[test]
    fn pending_vertical_split_stacks_in_columns() {
        let mut l = Layout::new(LayoutKind::Columns, 0.5);
        l.add(AREA, 10.0);
        l.pending_axis = Some(Axis::Column);
        l.add(AREA, 10.0);
        let rects = l.layout(AREA, 10.0, 0.0);
        assert_eq!(rects[0].1.x, rects[1].1.x, "same column");
        assert!(l.pending_axis.is_none(), "used once");
    }
}

