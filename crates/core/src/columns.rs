// SPDX-License-Identifier: GPL-3.0-or-later
//! Scrolling columns (niri-style): tiles are columns on a horizontal strip that can
//! be wider than the code region; the view scrolls to keep the focused column in
//! sight. A column stacks its tiles vertically, sharing its height equally.

use crate::tiles::{Direction, Rect, TileId};
use crate::token::MIN_COLUMN_WIDTH;

#[derive(Debug, Clone, PartialEq)]
struct Column {
    tiles: Vec<TileId>,
    /// Fraction of the code region's width.
    width: f64,
}

#[derive(Debug, Default)]
pub struct Columns {
    columns: Vec<Column>,
    focused: Option<TileId>,
    /// Horizontal scroll of the strip in px (0 = first column at the left edge).
    scroll: f64,
    /// How much of a neighbouring column stays in sight beside the focused one (px), so
    /// you can tell more columns are there (`tiling.peek`).
    pub peek: f64,
}

const MIN_WIDTH_FRACTION: f64 = 0.2;

impl Columns {
    /// One column per tile, in the given order.
    pub fn from_ids(ids: &[TileId], focused: Option<TileId>, width: f64) -> Columns {
        let columns = ids.iter().map(|&id| Column { tiles: vec![id], width }).collect();
        Columns { columns, focused: focused.or(ids.first().copied()), scroll: 0.0, peek: 0.0 }
    }

    pub fn focused(&self) -> Option<TileId> {
        self.focused
    }

    pub fn focus(&mut self, id: TileId) {
        if self.position(id).is_some() {
            self.focused = Some(id);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    pub fn scroll(&self) -> f64 {
        self.scroll
    }

    /// Column and row of a tile.
    fn position(&self, id: TileId) -> Option<(usize, usize)> {
        self.columns.iter().enumerate().find_map(|(c, col)| col.tiles.iter().position(|&t| t == id).map(|r| (c, r)))
    }

    /// Adds `id` as a new column right of the focused one, or (`stack`) below the
    /// focused tile in its column. Focuses it.
    pub fn insert(&mut self, id: TileId, stack: bool, width: f64) {
        match self.focused.and_then(|f| self.position(f)) {
            Some((c, r)) if stack => self.columns[c].tiles.insert(r + 1, id),
            Some((c, _)) => self.columns.insert(c + 1, Column { tiles: vec![id], width }),
            None => self.columns.push(Column { tiles: vec![id], width }),
        }
        self.focused = Some(id);
    }

    /// Removes a tile; an emptied column disappears. If it had focus, focus moves to
    /// the tile above it, else the column to the left, else the right.
    pub fn close(&mut self, id: TileId) -> bool {
        let Some((c, r)) = self.position(id) else { return false };
        self.columns[c].tiles.remove(r);
        let column_removed = self.columns[c].tiles.is_empty();
        if column_removed {
            self.columns.remove(c);
        }
        if self.focused == Some(id) {
            self.focused = if column_removed {
                // The column to the left; at the left end, the one that moved into slot 0.
                self.columns.get(c.saturating_sub(1)).map(|col| col.tiles[0])
            } else {
                let tiles = &self.columns[c].tiles;
                Some(tiles[r.saturating_sub(1).min(tiles.len() - 1)])
            };
        }
        true
    }

    /// Tiles in reading order: column by column, top to bottom.
    pub fn ids(&self) -> Vec<TileId> {
        self.columns.iter().flat_map(|c| c.tiles.iter().copied()).collect()
    }

    /// Never so wide that a column between two others hides both neighbours' peeks.
    fn column_width(&self, col: &Column, area: Rect, gap: f64) -> f64 {
        let widest = if self.peek > 0.0 { (area.w - 2.0 * (self.peek + gap)).max(area.w / 2.0) } else { area.w };
        (col.width * area.w).max(MIN_COLUMN_WIDTH.min(area.w)).min(widest)
    }

    /// Left edge of each column relative to the strip's start, and its width. A lone
    /// column fills the region; column widths apply from two columns on.
    fn spans(&self, area: Rect, gap: f64) -> Vec<(f64, f64)> {
        let alone = self.columns.len() == 1;
        let mut x = 0.0;
        self.columns
            .iter()
            .map(|col| {
                let w = if alone { area.w } else { self.column_width(col, area, gap) };
                let span = (x, w);
                x += w + gap;
                span
            })
            .collect()
    }

    /// Rects for every tile at scroll offset `scroll` (pass `scroll()` or an animated
    /// value). Tiles outside `area` keep their off-screen rects; the caller hides them.
    pub fn layout(&self, area: Rect, gap: f64, scroll: f64) -> Vec<(TileId, Rect)> {
        let mut out = Vec::new();
        for (col, (x, w)) in self.columns.iter().zip(self.spans(area, gap)) {
            let n = col.tiles.len() as f64;
            let h = ((area.h - gap * (n - 1.0)) / n).max(0.0);
            for (i, &id) in col.tiles.iter().enumerate() {
                out.push((id, Rect { x: area.x + x - scroll, y: area.y + i as f64 * (h + gap), w, h }));
            }
        }
        out
    }

    /// Scrolls the least amount that shows the whole focused column, plus `peek` of
    /// each neighbour it has, never past the ends.
    pub fn ensure_visible(&mut self, area: Rect, gap: f64) {
        let spans = self.spans(area, gap);
        let total = spans.last().map_or(0.0, |(x, w)| x + w);
        if let Some((c, _)) = self.focused.and_then(|f| self.position(f)) {
            let (x, w) = spans[c];
            let side = |has: bool| if has && self.peek > 0.0 { self.peek + gap } else { 0.0 };
            let (left, right) = (x - side(c > 0), x + w + side(c + 1 < spans.len()));
            if left < self.scroll {
                self.scroll = left;
            } else if right > self.scroll + area.w {
                self.scroll = right - area.w;
            }
        }
        self.scroll = self.scroll.clamp(0.0, (total - area.w).max(0.0));
    }

    /// Left/right: the neighbouring column (same row where it has one). Up/down: within the column.
    pub fn focus_direction(&mut self, direction: Direction) {
        let Some((c, r)) = self.focused.and_then(|f| self.position(f)) else { return };
        let target = match direction {
            Direction::Left if c > 0 => Some((c - 1, r)),
            Direction::Right if c + 1 < self.columns.len() => Some((c + 1, r)),
            Direction::Up if r > 0 => Some((c, r - 1)),
            Direction::Down if r + 1 < self.columns[c].tiles.len() => Some((c, r + 1)),
            _ => None,
        };
        if let Some((c, r)) = target {
            let col = &self.columns[c].tiles;
            self.focused = Some(col[r.min(col.len() - 1)]);
        }
    }

    /// Right widens the focused column, left narrows it (by `amount` of the region's width).
    pub fn resize_focused(&mut self, direction: Direction, amount: f64) {
        let Some((c, _)) = self.focused.and_then(|f| self.position(f)) else { return };
        let delta = match direction {
            Direction::Right => amount,
            Direction::Left => -amount,
            Direction::Up | Direction::Down => return,
        };
        let col = &mut self.columns[c];
        col.width = (col.width + delta).clamp(MIN_WIDTH_FRACTION, 1.0);
    }

    /// Left/right moves the focused column; up/down moves the tile within its column.
    pub fn move_focused(&mut self, direction: Direction) {
        let Some((c, r)) = self.focused.and_then(|f| self.position(f)) else { return };
        match direction {
            Direction::Left if c > 0 => self.columns.swap(c, c - 1),
            Direction::Right if c + 1 < self.columns.len() => self.columns.swap(c, c + 1),
            Direction::Up if r > 0 => self.columns[c].tiles.swap(r, r - 1),
            Direction::Down if r + 1 < self.columns[c].tiles.len() => self.columns[c].tiles.swap(r, r + 1),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect { x: 100.0, y: 20.0, w: 1000.0, h: 600.0 };

    fn three() -> Columns {
        let mut c = Columns::default();
        for id in 0..3 {
            c.insert(id, false, 0.5);
        }
        c
    }

    #[test]
    fn columns_sit_side_by_side_past_the_edge() {
        let c = three();
        let rects = c.layout(AREA, 10.0, 0.0);
        assert_eq!(rects[0].1, Rect { x: 100.0, y: 20.0, w: 500.0, h: 600.0 });
        assert_eq!(rects[1].1.x, 610.0);
        assert_eq!(rects[2].1.x, 1120.0, "third column starts off-screen");
    }

    #[test]
    fn neighbours_peek_in_beside_the_focused_column() {
        let area = Rect { x: 0.0, y: 0.0, w: 1000.0, h: 600.0 };
        let mut c = Columns { peek: 40.0, ..Columns::default() };
        for id in 0..4 {
            c.insert(id, false, 0.95);
            c.ensure_visible(area, 10.0);
        }
        let shown = |c: &Columns, id| c.layout(area, 10.0, c.scroll()).into_iter().find(|(i, _)| *i == id).unwrap().1;
        // Focused: the last column, fully shown, with 40 px of the one before it.
        let last = shown(&c, 3);
        assert!(last.x >= 50.0 - 1e-6 && last.x + last.w <= 1000.0 + 1e-6, "{last:?}");
        assert!(shown(&c, 2).x + shown(&c, 2).w >= 40.0 - 1e-6, "the previous column peeks in");
        // A middle column shows both neighbours' edges.
        c.focus(1);
        c.ensure_visible(area, 10.0);
        let middle = shown(&c, 1);
        assert!(middle.x >= 50.0 - 1e-6 && middle.x + middle.w <= 950.0 + 1e-6, "{middle:?}");
    }

    #[test]
    fn scroll_shows_the_focused_column_and_stops_at_the_ends() {
        let mut c = three();
        c.ensure_visible(AREA, 10.0);
        assert_eq!(c.scroll(), 520.0, "column 2 (1020..1520) ends at the right edge");
        c.focus(0);
        c.ensure_visible(AREA, 10.0);
        assert_eq!(c.scroll(), 0.0);
        c.focus(1);
        c.ensure_visible(AREA, 10.0);
        assert_eq!(c.scroll(), 10.0, "column 1 (510..1010) needs 10 px to fit");
        c.focus(0);
        c.ensure_visible(AREA, 10.0);
        c.scroll = 5.0;
        c.ensure_visible(AREA, 10.0);
        assert_eq!(c.scroll(), 0.0, "column 0 starts at 0, so scroll returns there");
    }

    #[test]
    fn stacking_splits_the_column_height() {
        let mut c = three();
        c.focus(1);
        c.insert(9, true, 0.5);
        let rects = c.layout(AREA, 10.0, 0.0);
        let r1 = rects.iter().find(|r| r.0 == 1).unwrap().1;
        let r9 = rects.iter().find(|r| r.0 == 9).unwrap().1;
        assert_eq!((r1.x, r1.h), (r9.x, 295.0));
        assert_eq!(r9.y, 20.0 + 305.0);
        assert_eq!(c.ids(), vec![0, 1, 9, 2]);
    }

    #[test]
    fn focus_moves_between_and_within_columns() {
        let mut c = three();
        c.focus(1);
        c.insert(9, true, 0.5);
        c.focus_direction(Direction::Up);
        assert_eq!(c.focused(), Some(1));
        c.focus_direction(Direction::Right);
        assert_eq!(c.focused(), Some(2));
        c.focus_direction(Direction::Right);
        assert_eq!(c.focused(), Some(2), "no column further right");
        c.focus_direction(Direction::Left);
        assert_eq!(c.focused(), Some(1));
    }

    #[test]
    fn close_removes_empty_columns_and_moves_focus_left() {
        let mut c = three();
        assert!(c.close(2));
        assert_eq!(c.focused(), Some(1));
        assert_eq!(c.ids(), vec![0, 1]);
        c.focus(0);
        assert!(c.close(0));
        assert_eq!(c.focused(), Some(1));
        assert!(c.close(1));
        assert!(c.is_empty());
        assert_eq!(c.focused(), None);
    }

    #[test]
    fn closing_a_stacked_tile_focuses_its_neighbour_in_the_column() {
        let mut c = three();
        c.focus(1);
        c.insert(9, true, 0.5);
        c.close(9);
        assert_eq!(c.focused(), Some(1));
    }

    #[test]
    fn a_lone_column_fills_the_region() {
        let area = Rect { x: 0.0, y: 0.0, w: 1600.0, h: 900.0 };
        let mut c = Columns::from_ids(&[1], Some(1), 0.5);
        assert_eq!(c.layout(area, 10.0, c.scroll())[0].1, area, "Alt+V on one tile keeps it full size");
        c.insert(2, false, 0.5);
        assert!(c.layout(area, 10.0, c.scroll()).iter().all(|(_, r)| r.w == 800.0), "two columns use their widths");
    }

    #[test]
    fn resize_and_move() {
        let mut c = three();
        c.focus(0);
        c.resize_focused(Direction::Right, 0.25);
        assert_eq!(c.layout(AREA, 10.0, 0.0)[0].1.w, 750.0);
        c.resize_focused(Direction::Left, 5.0);
        assert_eq!(c.layout(AREA, 10.0, 0.0)[0].1.w, MIN_COLUMN_WIDTH, "never narrower than the minimum");
        c.move_focused(Direction::Right);
        assert_eq!(c.ids(), vec![1, 0, 2]);
        assert_eq!(c.focused(), Some(0));
    }
}
