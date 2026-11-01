// SPDX-License-Identifier: GPL-3.0-or-later
//! Motion timing (design.md §8). Things that move glide with ease-out cubic; things that
//! appear or leave fade with a gentler ease-out quad, so opacity never pops. Leaving is
//! quicker than arriving, so nothing waits on an exit.

use crate::tiles::Rect;

/// Tile glide: open, close, resize, move, `Alt+V`, columns scroll.
pub const MOVE_US: f64 = 220_000.0;
/// Fade-in: new tiles, workspace switch, overlays, theme crossfade.
pub const FADE_IN_US: f64 = 180_000.0;
/// Fade-out: closing tiles and overlays, the workspace being left.
pub const FADE_OUT_US: f64 = 120_000.0;

/// Ease-out cubic, for movement.
pub fn ease(t: f64) -> f64 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// Ease-out quad, for opacity: cubic reaches half opacity in a fifth of the time,
/// which reads as a pop rather than a fade.
pub fn ease_fade(t: f64) -> f64 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(2)
}

/// Eased glide progress of an animation started at `start_us`, and whether it has finished.
pub fn progress(start_us: i64, now_us: i64) -> (f64, bool) {
    let t = (now_us - start_us) as f64 / MOVE_US;
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
        Fade::with_duration(from, to, if to >= from { FADE_IN_US } else { FADE_OUT_US })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eases_out_and_ends_exactly() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        assert!(ease(0.5) > 0.5, "ease-out moves fast first");
        assert_eq!(progress(0, MOVE_US as i64 * 2), (1.0, true), "clamped past the end");
        assert!(!progress(0, 1).1);
    }

    #[test]
    fn fades_are_gentler_than_moves() {
        assert!(ease_fade(0.2) < ease(0.2));
        assert_eq!(ease_fade(1.0), 1.0);
    }

    #[test]
    fn fade_out_is_quicker_than_fade_in() {
        let quarter = (FADE_OUT_US / 4.0) as i64;
        assert!(Fade::new(1.0, 0.0).at(quarter).0 < 1.0 - Fade::new(0.0, 1.0).at(quarter).0);
        assert_eq!(Fade::new(1.0, 0.0).at(FADE_OUT_US as i64), (0.0, true));
        assert_eq!(Fade::new(0.0, 1.0).at(FADE_IN_US as i64), (1.0, true));
    }

    #[test]
    fn interrupted_fade_continues_from_where_it_is() {
        let fade = Fade::new(0.4, 1.0);
        assert_eq!(fade.at(0), (0.4, false));
        assert!(fade.at((FADE_IN_US * 0.6) as i64).1, "only the remaining 60% is played");
        assert_eq!(Fade::new(1.0, 1.0).at(0), (1.0, true), "nothing to do");
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
