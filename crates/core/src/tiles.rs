// SPDX-License-Identifier: GPL-3.0-or-later
//! The code region's tile tree (spec §4.3), independent of GTK so it can be unit-tested.
//! A binary split tree: every split divides its rectangle between two children by `ratio`.

pub type TileId = u32;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn centre(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// `Row` puts children side by side (a vertical divider); `Column` stacks them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Axis {
    Row,
    Column,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    fn axis(self) -> Axis {
        match self {
            Direction::Left | Direction::Right => Axis::Row,
            Direction::Up | Direction::Down => Axis::Column,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Leaf(TileId),
    Split { axis: Axis, ratio: f64, first: Box<Node>, second: Box<Node> },
}

#[derive(Debug, Default)]
pub struct TileTree {
    root: Option<Node>,
    focused: Option<TileId>,
}

const MIN_RATIO: f64 = 0.1;

impl TileTree {
    pub fn focused(&self) -> Option<TileId> {
        self.focused
    }

    pub fn focus(&mut self, id: TileId) {
        if self.contains(id) {
            self.focused = Some(id);
        }
    }

    fn contains(&self, id: TileId) -> bool {
        fn walk(node: &Node, id: TileId) -> bool {
            match node {
                Node::Leaf(leaf) => *leaf == id,
                Node::Split { first, second, .. } => walk(first, id) || walk(second, id),
            }
        }
        self.root.as_ref().is_some_and(|n| walk(n, id))
    }

    /// Adds tile `id` next to the focused one and focuses it. Dwindle: when `axis` is
    /// `None`, the focused tile is split along its longer side (needs its current rect).
    pub fn split(&mut self, id: TileId, axis: Option<Axis>, focused_rect: Option<Rect>) {
        let axis = axis.unwrap_or(match focused_rect {
            Some(r) if r.h > r.w => Axis::Column,
            _ => Axis::Row,
        });
        match (self.root.take(), self.focused) {
            (None, _) | (_, None) => self.root = Some(Node::Leaf(id)),
            (Some(mut root), Some(target)) => {
                replace_leaf(&mut root, target, &mut |leaf| Node::Split {
                    axis,
                    ratio: 0.5,
                    first: Box::new(leaf),
                    second: Box::new(Node::Leaf(id)),
                });
                self.root = Some(root);
            }
        }
        self.focused = Some(id);
    }

    /// Builds a dwindle tree from tiles in reading order (used when leaving columns).
    pub fn from_ids(ids: &[TileId], focused: Option<TileId>, area: Rect, gap: f64) -> TileTree {
        let mut tree = TileTree::default();
        for &id in ids {
            let rect = tree.layout(area, gap).into_iter().find(|(i, _)| Some(*i) == tree.focused).map(|(_, r)| r);
            tree.split(id, None, rect);
        }
        if let Some(id) = focused {
            tree.focus(id);
        }
        tree
    }

    /// Tiles in layout order: left to right, top to bottom.
    pub fn ids(&self) -> Vec<TileId> {
        fn walk(node: &Node, out: &mut Vec<TileId>) {
            match node {
                Node::Leaf(id) => out.push(*id),
                Node::Split { first, second, .. } => {
                    walk(first, out);
                    walk(second, out);
                }
            }
        }
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            walk(root, &mut out);
        }
        out
    }

    /// Swaps the focused tile with its neighbour in `direction`; focus follows the tile.
    pub fn move_focused(&mut self, direction: Direction, area: Rect, gap: f64) {
        let Some(from) = self.focused else { return };
        self.focus_direction(direction, area, gap);
        let Some(to) = self.focused.filter(|&to| to != from) else { return };
        fn swap(node: &mut Node, a: TileId, b: TileId) {
            match node {
                Node::Leaf(id) if *id == a => *id = b,
                Node::Leaf(id) if *id == b => *id = a,
                Node::Leaf(_) => {}
                Node::Split { first, second, .. } => {
                    swap(first, a, b);
                    swap(second, a, b);
                }
            }
        }
        if let Some(root) = self.root.as_mut() {
            swap(root, from, to);
        }
        self.focused = Some(from);
    }

    /// Removes a tile; its sibling takes the space. Focus moves to the sibling side only
    /// if the closed tile had it (e.g. a terminal whose shell exited keeps focus elsewhere).
    pub fn close(&mut self, id: TileId) -> bool {
        if !self.contains(id) {
            return false;
        }
        self.root = self.root.take().and_then(|root| remove_leaf(root, id));
        if self.focused == Some(id) {
            self.focused = self.root.as_ref().map(first_leaf);
        }
        true
    }

    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    pub fn layout(&self, area: Rect, gap: f64) -> Vec<(TileId, Rect)> {
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            layout_node(root, area, gap, &mut out);
        }
        out
    }

    /// Moves focus to the nearest tile whose centre lies in `direction`.
    pub fn focus_direction(&mut self, direction: Direction, area: Rect, gap: f64) {
        let rects = self.layout(area, gap);
        let Some(&(_, current)) = rects.iter().find(|(id, _)| Some(*id) == self.focused) else { return };
        let (cx, cy) = current.centre();
        let best = rects
            .iter()
            .filter(|(id, _)| Some(*id) != self.focused)
            .filter_map(|(id, r)| {
                let (x, y) = r.centre();
                let (along, across) = match direction {
                    Direction::Left => (cx - x, (y - cy).abs()),
                    Direction::Right => (x - cx, (y - cy).abs()),
                    Direction::Up => (cy - y, (x - cx).abs()),
                    Direction::Down => (y - cy, (x - cx).abs()),
                };
                (along > 0.0).then_some((id, along + 2.0 * across))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((id, _)) = best {
            self.focused = Some(*id);
        }
    }

    /// Grows the focused tile towards `direction` by `amount` (fraction of the parent split).
    pub fn resize_focused(&mut self, direction: Direction, amount: f64) {
        let (Some(target), Some(root)) = (self.focused, self.root.as_mut()) else { return };
        resize(root, target, direction, amount);
    }
}

fn first_leaf(node: &Node) -> TileId {
    match node {
        Node::Leaf(id) => *id,
        Node::Split { first, .. } => first_leaf(first),
    }
}

fn replace_leaf(node: &mut Node, target: TileId, make: &mut dyn FnMut(Node) -> Node) -> bool {
    match node {
        Node::Leaf(id) if *id == target => {
            let leaf = std::mem::replace(node, Node::Leaf(target));
            *node = make(leaf);
            true
        }
        Node::Leaf(_) => false,
        Node::Split { first, second, .. } => replace_leaf(first, target, make) || replace_leaf(second, target, make),
    }
}

fn remove_leaf(node: Node, target: TileId) -> Option<Node> {
    match node {
        Node::Leaf(id) if id == target => None,
        Node::Leaf(_) => Some(node),
        Node::Split { axis, ratio, first, second } => match (remove_leaf(*first, target), remove_leaf(*second, target)) {
            (Some(a), Some(b)) => Some(Node::Split { axis, ratio, first: Box::new(a), second: Box::new(b) }),
            (Some(only), None) | (None, Some(only)) => Some(only),
            (None, None) => None,
        },
    }
}

fn layout_node(node: &Node, r: Rect, gap: f64, out: &mut Vec<(TileId, Rect)>) {
    match node {
        Node::Leaf(id) => out.push((*id, r)),
        Node::Split { axis, ratio, first, second } => {
            let (a, b) = match axis {
                Axis::Row => {
                    let w = ((r.w - gap) * ratio).max(0.0);
                    (Rect { w, ..r }, Rect { x: r.x + w + gap, w: (r.w - gap - w).max(0.0), ..r })
                }
                Axis::Column => {
                    let h = ((r.h - gap) * ratio).max(0.0);
                    (Rect { h, ..r }, Rect { y: r.y + h + gap, h: (r.h - gap - h).max(0.0), ..r })
                }
            };
            layout_node(first, a, gap, out);
            layout_node(second, b, gap, out);
        }
    }
}

/// Returns whether `target` is inside `node`; adjusts the deepest matching-axis split
/// where moving the divider in `direction` grows the target.
fn resize(node: &mut Node, target: TileId, direction: Direction, amount: f64) -> bool {
    let Node::Split { axis, ratio, first, second } = node else {
        return matches!(node, Node::Leaf(id) if *id == target);
    };
    let in_first = resize(first, target, direction, amount);
    let in_second = !in_first && resize(second, target, direction, amount);
    if !in_first && !in_second {
        return false;
    }
    // Only act once: at the deepest split on this axis whose divider is on the right side.
    let grows = match direction {
        Direction::Right | Direction::Down => in_first,
        Direction::Left | Direction::Up => in_second,
    };
    if *axis == direction.axis() && grows && amount != 0.0 {
        let delta = if in_first { amount } else { -amount };
        *ratio = (*ratio + delta).clamp(MIN_RATIO, 1.0 - MIN_RATIO);
        return false; // consumed: parents must not resize again
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect { x: 0.0, y: 0.0, w: 1000.0, h: 600.0 };

    fn three() -> TileTree {
        // main | (config / terminal), like the reference layout
        let mut t = TileTree::default();
        t.split(0, None, None);
        t.split(1, Some(Axis::Row), None);
        t.split(2, Some(Axis::Column), None);
        t
    }

    #[test]
    fn layout_respects_gaps() {
        let t = three();
        let rects = t.layout(AREA, 8.0);
        assert_eq!(rects.len(), 3);
        assert_eq!(rects[0].1, Rect { x: 0.0, y: 0.0, w: 496.0, h: 600.0 });
        assert_eq!(rects[1].1, Rect { x: 504.0, y: 0.0, w: 496.0, h: 296.0 });
        assert_eq!(rects[2].1, Rect { x: 504.0, y: 304.0, w: 496.0, h: 296.0 });
    }

    #[test]
    fn dwindle_splits_the_longer_side() {
        let mut t = TileTree::default();
        t.split(0, None, None);
        t.split(1, None, Some(Rect { w: 300.0, h: 800.0, ..AREA }));
        let rects = t.layout(AREA, 0.0);
        assert_eq!(rects[0].1.w, 1000.0, "tall tile splits into a column");
    }

    #[test]
    fn close_gives_space_and_focus_to_sibling() {
        let mut t = three();
        assert!(t.close(2));
        let rects = t.layout(AREA, 8.0);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[1].1.h, 600.0, "tile 1 takes the full column");
        assert_eq!(t.focused(), Some(0));
        while let Some(id) = t.focused() {
            t.close(id);
        }
        assert!(t.layout(AREA, 8.0).is_empty());
        assert_eq!(t.focused(), None);
    }

    #[test]
    fn closing_an_unfocused_tile_keeps_focus() {
        let mut t = three();
        assert!(t.close(1));
        assert_eq!(t.focused(), Some(2));
        assert!(!t.close(1), "already gone");
        assert_eq!(t.layout(AREA, 0.0).len(), 2);
    }

    #[test]
    fn focus_moves_by_direction() {
        let mut t = three();
        t.focus_direction(Direction::Up, AREA, 8.0);
        assert_eq!(t.focused(), Some(1));
        t.focus_direction(Direction::Left, AREA, 8.0);
        assert_eq!(t.focused(), Some(0));
        t.focus_direction(Direction::Left, AREA, 8.0);
        assert_eq!(t.focused(), Some(0), "nothing further left");
        t.focus_direction(Direction::Right, AREA, 8.0);
        assert!(matches!(t.focused(), Some(1 | 2)));
    }

    #[test]
    fn resize_moves_the_right_divider_once() {
        let mut t = three();
        t.focus(0);
        t.resize_focused(Direction::Right, 0.1);
        let rects = t.layout(AREA, 0.0);
        assert!((rects[0].1.w - 600.0).abs() < 1e-9);
        t.focus(2);
        t.resize_focused(Direction::Up, 0.1);
        let rects = t.layout(AREA, 0.0);
        assert!((rects[2].1.h - 360.0).abs() < 1e-9, "bottom tile grows upward");
        assert!((rects[0].1.w - 600.0).abs() < 1e-9, "row split untouched");
        t.resize_focused(Direction::Left, 0.1);
        let rects = t.layout(AREA, 0.0);
        assert!((rects[0].1.w - 500.0).abs() < 1e-9, "left edge of tile 2 is the row divider");
    }

    #[test]
    fn move_swaps_with_the_neighbour() {
        let mut t = three();
        t.focus(0);
        t.move_focused(Direction::Right, AREA, 0.0);
        assert_eq!(t.focused(), Some(0), "focus follows the moved tile");
        assert_eq!(t.ids(), vec![1, 0, 2]);
    }

    #[test]
    fn rebuild_keeps_order_and_focus() {
        let t = TileTree::from_ids(&[4, 7, 9], Some(7), AREA, 0.0);
        assert_eq!(t.ids(), vec![4, 7, 9]);
        assert_eq!(t.focused(), Some(7));
    }

    #[test]
    fn ratio_is_clamped() {
        let mut t = three();
        t.focus(0);
        for _ in 0..20 {
            t.resize_focused(Direction::Right, 0.1);
        }
        assert!((t.layout(AREA, 0.0)[0].1.w - 900.0).abs() < 1e-9);
    }
}
