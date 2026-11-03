// SPDX-License-Identifier: GPL-3.0-or-later
//! Motion timing (design.md §8), in two styles chosen in Settings or with
//! `tileide.appearance { motion = "fluid" | "snappy", motion_speed = 1.0 }`:
//! - **Fluid** (default, Apple-like): things that move follow a critically damped spring
//!   that leaves already moving (no hesitation, no overshoot, a long soft settle); fades
//!   ease out, so what appears shows at once and what leaves goes at once.
//! - **Snappy**: shorter ease-out cubic moves and ease-out quad fades.
//!
//! In both, leaving is quicker than arriving, so nothing waits on an exit. The speed
//! multiplier divides every duration.

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use crate::tiles::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Curve {
    #[default]
    Fluid,
    Snappy,
}

impl Curve {
    pub const ALL: [Curve; 2] = [Curve::Fluid, Curve::Snappy];

    pub fn key(self) -> &'static str {
        match self {
            Curve::Fluid => "fluid",
            Curve::Snappy => "snappy",
        }
    }

    pub fn from_key(key: &str) -> Option<Curve> {
        Curve::ALL.into_iter().find(|c| c.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Curve::Fluid => "Fluid",
            Curve::Snappy => "Snappy",
        }
    }

    /// (move, fade in, fade out, drawer) in µs at speed 1.
    fn durations(self) -> [f64; 4] {
        match self {
            Curve::Fluid => [380_000.0, 240_000.0, 160_000.0, 340_000.0],
            Curve::Snappy => [220_000.0, 180_000.0, 120_000.0, 200_000.0],
        }
    }
}

pub const MIN_SPEED: f64 = 0.5;
pub const MAX_SPEED: f64 = 2.0;

static CURVE: AtomicU8 = AtomicU8::new(0);
static SPEED: AtomicU64 = AtomicU64::new(0x3FF0_0000_0000_0000); // 1.0

/// Sets the style for every animation from now on.
pub fn set_profile(curve: Curve, speed: f64) {
    CURVE.store(curve as u8, Ordering::Relaxed);
    SPEED.store(speed.clamp(MIN_SPEED, MAX_SPEED).to_bits(), Ordering::Relaxed);
}

pub fn curve() -> Curve {
    if CURVE.load(Ordering::Relaxed) == Curve::Snappy as u8 { Curve::Snappy } else { Curve::Fluid }
}

pub fn speed() -> f64 {
    f64::from_bits(SPEED.load(Ordering::Relaxed))
}

fn duration(i: usize) -> f64 {
    curve().durations()[i] / speed()
}

/// Tile glide: open, close, resize, move, `Alt+V`, columns scroll.
pub fn move_us() -> f64 {
    duration(0)
}

/// Fade-in: new tiles, workspace switch, overlays, theme crossfade.
pub fn fade_in_us() -> f64 {
    duration(1)
}

/// Fade-out: closing tiles and overlays, the workspace being left.
pub fn fade_out_us() -> f64 {
    duration(2)
}

/// Drawer expand/collapse.
pub fn drawer_us() -> f64 {
    duration(3)
}

/// Critically damped spring that starts at 70% of its natural speed (a spring from rest
/// hesitates first, which reads as lag), scaled to settle within 0.1% at t = 1. With a
/// start speed at most ω it never overshoots.
fn spring(t: f64) -> f64 {
    const OMEGA: f64 = 8.4;
    const START: f64 = 0.7 * OMEGA;
    if t >= 1.0 {
        return 1.0;
    }
    1.0 - (1.0 + (OMEGA - START) * t) * (-OMEGA * t).exp()
}

/// Movement curve of `curve`.
pub fn ease_with(curve: Curve, t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    match curve {
        Curve::Fluid => spring(t),
        Curve::Snappy => 1.0 - (1.0 - t).powi(3),
    }
}

/// Opacity curve of `curve`: ease-out, so a fade answers at once. Fluid uses sine (a
/// little softer than Snappy's quad over a longer time); cubic would reach half opacity
/// in a fifth of the time, which reads as a pop.
pub fn ease_fade_with(curve: Curve, t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    match curve {
        Curve::Fluid => (std::f64::consts::FRAC_PI_2 * t).sin(),
        Curve::Snappy => 1.0 - (1.0 - t).powi(2),
    }
}

pub fn ease(t: f64) -> f64 {
    ease_with(curve(), t)
}

pub fn ease_fade(t: f64) -> f64 {
    ease_fade_with(curve(), t)
}

/// Eased glide progress of an animation started at `start_us`, and whether it has finished.
pub fn progress(start_us: i64, now_us: i64) -> (f64, bool) {
    let t = (now_us - start_us) as f64 / move_us();
    (ease(t), t >= 1.0)
}

pub fn lerp_rect(from: Rect, to: Rect, k: f64) -> Rect {
    let mix = |a: f64, b: f64| a + (b - a) * k;
    Rect { x: mix(from.x, to.x), y: mix(from.y, to.y), w: mix(from.w, to.w), h: mix(from.h, to.h) }
}

/// An opacity fade. Starting from wherever the widget is now keeps an interrupted fade
/// continuous, and a partial fade takes proportionally less time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fade {
    from: f64,
    to: f64,
    duration_us: f64,
}

impl Fade {
    pub fn new(from: f64, to: f64) -> Fade {
        Fade::with_duration(from, to, if to >= from { fade_in_us() } else { fade_out_us() })
    }

    /// `duration_us` is for a full 0 ⇄ 1 fade.
    pub fn with_duration(from: f64, to: f64, duration_us: f64) -> Fade {
        Fade { from, to, duration_us: duration_us * (to - from).abs() }
    }

    /// Opacity `elapsed_us` into the fade, and whether it has finished.
    pub fn at(&self, elapsed_us: i64) -> (f64, bool) {
        if self.duration_us <= 0.0 {
            return (self.to, true);
        }
        let t = elapsed_us as f64 / self.duration_us;
        (self.from + (self.to - self.from) * ease_fade(t), t >= 1.0)
    }
}

/// Things that come and go from a list (the rail's workspace dots): each fades and
/// grows in when it appears, and shrinks out when it leaves.
#[derive(Debug, Default)]
pub struct Presence {
    /// Item, running fade and when it started (`None` = settled at `to`).
    items: Vec<(u8, Fade, Option<i64>)>,
    started: bool,
}

impl Presence {
    /// Follows `now_present` (sorted). The first call settles at once: nothing animates in at startup.
    pub fn sync(&mut self, now_present: &[u8], now_us: i64, animate: bool) {
        if !self.started || !animate {
            self.started = true;
            self.items = now_present.iter().map(|&n| (n, Fade::new(1.0, 1.0), None)).collect();
            return;
        }
        for &n in now_present {
            match self.items.iter_mut().find(|i| i.0 == n) {
                Some(item) if item.1.to < 1.0 => *item = (n, Fade::new(item.1.at(elapsed(item.2, now_us)).0, 1.0), Some(now_us)),
                Some(_) => {}
                None => self.items.push((n, Fade::new(0.0, 1.0), Some(now_us))),
            }
        }
        for item in self.items.iter_mut().filter(|i| !now_present.contains(&i.0) && i.1.to > 0.0) {
            *item = (item.0, Fade::new(item.1.at(elapsed(item.2, now_us)).0, 0.0), Some(now_us));
        }
        self.items.sort_by_key(|i| i.0);
    }

    /// Each item with its presence 0..1 at `now_us`; items that finished leaving are dropped.
    pub fn at(&mut self, now_us: i64) -> Vec<(u8, f64)> {
        self.items.retain(|i| !(i.1.to == 0.0 && i.1.at(elapsed(i.2, now_us)).1));
        self.items.iter().map(|i| (i.0, i.1.at(elapsed(i.2, now_us)).0)).collect()
    }

    pub fn animating(&self, now_us: i64) -> bool {
        self.items.iter().any(|i| !i.1.at(elapsed(i.2, now_us)).1)
    }
}

fn elapsed(start: Option<i64>, now_us: i64) -> i64 {
    start.map_or(i64::MAX, |s| now_us - s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eases_out_and_ends_exactly() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        assert!(ease(0.5) > 0.5, "ease-out moves fast first");
        assert_eq!(progress(0, move_us() as i64 * 2), (1.0, true), "clamped past the end");
        assert!(!progress(0, 1).1);
    }

    #[test]
    fn fades_are_gentler_than_moves() {
        for c in Curve::ALL {
            assert!(ease_fade_with(c, 0.2) < ease_with(c, 0.2), "{c:?}");
            assert_eq!(ease_fade_with(c, 1.0), 1.0);
            assert_eq!(ease_with(c, 0.0), 0.0);
            assert_eq!(ease_with(c, 1.0), 1.0);
        }
    }

    #[test]
    fn fluid_springs_without_overshoot() {
        let mut last = 0.0;
        for i in 1..=100 {
            let v = ease_with(Curve::Fluid, i as f64 / 100.0);
            assert!(v >= last && v <= 1.0, "monotonic, never past the target");
            last = v;
        }
        assert!(ease_with(Curve::Fluid, 0.08) > 0.3, "leaves at once (a spring from rest hesitated: 0.16 here)");
        assert!(ease_fade_with(Curve::Fluid, 0.15) > 0.2, "a new tile shows at once");
        assert!((ease_with(Curve::Fluid, 0.99) - 1.0).abs() < 0.002, "settled at the end");
        assert_eq!(Curve::from_key("fluid"), Some(Curve::Fluid));
        assert_eq!(Curve::from_key("bouncy"), None);
    }

    #[test]
    fn fade_out_is_quicker_than_fade_in() {
        let quarter = (fade_out_us() / 4.0) as i64;
        assert!(Fade::new(1.0, 0.0).at(quarter).0 < 1.0 - Fade::new(0.0, 1.0).at(quarter).0);
        assert_eq!(Fade::new(1.0, 0.0).at(fade_out_us() as i64), (0.0, true));
        assert_eq!(Fade::new(0.0, 1.0).at(fade_in_us() as i64), (1.0, true));
    }

    #[test]
    fn interrupted_fade_continues_from_where_it_is() {
        let fade = Fade::new(0.4, 1.0);
        assert_eq!(fade.at(0), (0.4, false));
        assert!(fade.at((fade_in_us() * 0.6) as i64).1, "only the remaining 60% is played");
        assert_eq!(Fade::new(1.0, 1.0).at(0), (1.0, true), "nothing to do");
    }

    #[test]
    fn presence_settles_at_start_then_fades_items_in_and_out() {
        let mut p = Presence::default();
        p.sync(&[1, 2], 0, true);
        assert_eq!(p.at(0), [(1, 1.0), (2, 1.0)], "no fade-in at startup");
        p.sync(&[1, 3], 1_000, true);
        let mid = p.at(1_000 + (fade_out_us() / 2.0) as i64);
        assert_eq!(mid.iter().map(|i| i.0).collect::<Vec<_>>(), [1, 2, 3], "2 is still leaving");
        assert!(mid[1].1 > 0.0 && mid[1].1 < 1.0 && mid[2].1 > 0.0 && mid[2].1 < 1.0);
        assert!(p.animating(1_000));
        let end = 1_000 + fade_in_us() as i64;
        assert_eq!(p.at(end), [(1, 1.0), (3, 1.0)], "2 is gone once faded");
        assert!(!p.animating(end));
    }

    #[test]
    fn presence_reverses_a_leaving_item_smoothly() {
        let mut p = Presence::default();
        p.sync(&[1, 2], 0, true);
        p.sync(&[1], 0, true);
        let t = (fade_out_us() / 2.0) as i64;
        let leaving = p.at(t)[1].1;
        p.sync(&[1, 2], t, true);
        assert_eq!(p.at(t)[1].1, leaving, "comes back from where it was");
    }

    #[test]
    fn rects_interpolate_every_edge() {
        let a = Rect { x: 0.0, y: 0.0, w: 100.0, h: 50.0 };
        let b = Rect { x: 100.0, y: 20.0, w: 300.0, h: 50.0 };
        assert_eq!(lerp_rect(a, b, 0.0), a);
        assert_eq!(lerp_rect(a, b, 1.0), b);
        assert_eq!(lerp_rect(a, b, 0.5), Rect { x: 50.0, y: 10.0, w: 200.0, h: 50.0 });
    }
}
