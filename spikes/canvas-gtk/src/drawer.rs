//! Drawer state: open/close animation (frame-clock driven) and its user-set width.

use crate::draw::token;

/// design.md §8: drawer expand/collapse 180–220 ms, ease-out, interruptible.
pub const DURATION_US: f64 = 200_000.0;

pub struct Drawer {
    /// Linear animation time 0..1; drawing eases it.
    pub progress: f64,
    from: f64,
    pub target: f64,
    start_us: Option<i64>,
    /// Width when fully open; dragged with the mouse.
    pub width: f64,
}

impl Default for Drawer {
    fn default() -> Self {
        Drawer { progress: 0.0, from: 0.0, target: 0.0, start_us: None, width: token::DRAWER_WIDTH }
    }
}

impl Drawer {
    pub fn toggle(&mut self) {
        self.set_target(if self.target > 0.5 { 0.0 } else { 1.0 });
    }

    /// Restarts from the current position, so reversing mid-animation never jumps.
    fn set_target(&mut self, target: f64) {
        self.from = self.progress;
        self.target = target;
        self.start_us = None;
    }

    pub fn animating(&self) -> bool {
        self.progress != self.target
    }

    pub fn is_open(&self) -> bool {
        self.target > 0.5 && !self.animating()
    }

    pub fn tick(&mut self, now_us: i64) {
        let start = *self.start_us.get_or_insert(now_us);
        // A partial reverse takes proportionally less time.
        let span = (self.target - self.from).abs().max(f64::EPSILON);
        let t = ((now_us - start) as f64 / (DURATION_US * span)).min(1.0);
        self.progress = self.from + (self.target - self.from) * t;
    }

    /// Current extension in px (ease-out cubic).
    pub fn extension(&self) -> f64 {
        self.width * (1.0 - (1.0 - self.progress).powi(3))
    }

    pub fn set_width(&mut self, width: f64) {
        self.width = width.clamp(token::DRAWER_MIN_WIDTH, token::DRAWER_MAX_WIDTH);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reaches_target_after_duration() {
        let mut d = Drawer::default();
        d.toggle();
        d.tick(0);
        d.tick(DURATION_US as i64);
        assert_eq!(d.progress, 1.0);
        assert!(d.is_open());
        assert_eq!(d.extension(), token::DRAWER_WIDTH);
    }

    #[test]
    fn reversing_mid_animation_is_continuous_and_shorter() {
        let mut d = Drawer::default();
        d.toggle();
        d.tick(0);
        d.tick(100_000);
        let at_reverse = d.progress;
        assert!((at_reverse - 0.5).abs() < 1e-9);
        d.toggle();
        d.tick(100_000);
        assert_eq!(d.progress, at_reverse, "no jump when reversing");
        d.tick(200_000);
        assert_eq!(d.progress, 0.0, "half the distance takes half the time");
    }

    #[test]
    fn width_is_clamped() {
        let mut d = Drawer::default();
        d.set_width(50.0);
        assert_eq!(d.width, token::DRAWER_MIN_WIDTH);
        d.set_width(5000.0);
        assert_eq!(d.width, token::DRAWER_MAX_WIDTH);
    }
}
