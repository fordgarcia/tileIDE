// SPDX-License-Identifier: GPL-3.0-or-later
//! The theme tray (`themes/theme-design.md` §1): the window dims, and a small tray of
//! colour strips, one per theme, sits above the bottom edge. ←/→ browse, Enter
//! applies and closes, Esc or a click on the dim closes without a change.

use std::cell::RefCell;
use std::f64::consts::{FRAC_PI_2, PI};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib, pango};
use gtk4 as gtk;
use tileide_core::theme::{Rgb, Theme};
use tileide_core::tiles::Rect;
use tileide_core::token;

const DIM: f64 = 0.6;
const STRIP_W: f64 = 30.0;
const SELECTED: (f64, f64) = (76.0, 120.0);
const STRIP_GAP: f64 = 6.0;
/// Caption pill: height, and its distance from the bottom edge.
const CAPTION_H: f64 = 34.0;
const BOTTOM: f64 = 28.0;
const CAPTION_GAP: f64 = 16.0;

struct State {
    /// (file stem, theme), in tray order.
    themes: Vec<(String, Theme)>,
    selected: usize,
    /// The app's current theme, for the caption.
    current: Rc<Theme>,
}

pub struct Tray {
    area: gtk::DrawingArea,
    state: Rc<RefCell<State>>,
}

/// Where each strip goes for a window of `width` × `height`: (rect, distance from the
/// selected strip, side −1/0/+1). Shared by drawing and clicking.
fn strips(count: usize, selected: usize, width: f64, height: f64) -> Vec<(Rect, usize, f64)> {
    let bottom = height - BOTTOM - CAPTION_H - CAPTION_GAP;
    let widths: Vec<f64> = (0..count).map(|i| if i == selected { SELECTED.0 } else { STRIP_W }).collect();
    let total = widths.iter().sum::<f64>() + STRIP_GAP * count.saturating_sub(1) as f64;
    let mut x = (width - total) / 2.0;
    (0..count)
        .map(|i| {
            let distance = i.abs_diff(selected);
            // Neighbours shrink with distance, like a coverflow seen at an angle.
            let h = if distance == 0 { SELECTED.1 } else { (SELECTED.1 * (0.78 - 0.03 * distance as f64)).max(56.0) };
            let rect = Rect { x, y: bottom - (SELECTED.1 + h) / 2.0, w: widths[i], h };
            x += widths[i] + STRIP_GAP;
            let side = (i as f64 - selected as f64).signum();
            (rect, distance, side)
        })
        .collect()
}

fn caption_rect(width: f64, height: f64, text_w: f64) -> Rect {
    let w = text_w + 32.0;
    Rect { x: (width - w) / 2.0, y: height - BOTTOM - CAPTION_H, w, h: CAPTION_H }
}

impl Tray {
    /// `on_apply` gets the chosen theme's file stem; `on_close` runs on every close.
    pub fn new(overlay: &gtk::Overlay, on_apply: Rc<dyn Fn(&str)>, on_close: Rc<dyn Fn()>) -> Tray {
        let area = gtk::DrawingArea::new();
        area.set_visible(false);
        area.set_focusable(true);
        area.set_can_target(true);
        overlay.add_overlay(&area);
        let state = Rc::new(RefCell::new(State { themes: Vec::new(), selected: 0, current: Rc::new(Theme::builtin()) }));

        let s = state.clone();
        area.set_draw_func(move |_, cr, w, h| {
            if let Err(e) = draw(cr, &s.borrow(), w as f64, h as f64) {
                eprintln!("tray: draw failed: {e}");
            }
        });

        let close = {
            let (area, on_close) = (area.clone(), on_close.clone());
            Rc::new(move || {
                crate::motion::fade_out(&area);
                on_close();
            })
        };
        let apply = {
            let (s, close) = (state.clone(), close.clone());
            Rc::new(move || {
                let key = s.borrow().themes.get(s.borrow().selected).map(|(k, _)| k.clone());
                close();
                if let Some(key) = key {
                    on_apply(&key);
                }
            })
        };

        let keys = gtk::EventControllerKey::new();
        let (s, a, apply2, close2) = (state.clone(), area.clone(), apply.clone(), close.clone());
        keys.connect_key_pressed(move |_, key, _, _| {
            match key {
                gdk::Key::Left | gdk::Key::Right => {
                    let mut st = s.borrow_mut();
                    let last = st.themes.len().saturating_sub(1);
                    st.selected = if key == gdk::Key::Left { st.selected.saturating_sub(1) } else { (st.selected + 1).min(last) };
                    a.queue_draw();
                }
                gdk::Key::Return | gdk::Key::KP_Enter => apply2(),
                gdk::Key::Escape => close2(),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        area.add_controller(keys);

        // A click on a strip selects it (a second click applies); anywhere else closes.
        let click = gtk::GestureClick::new();
        let (s, a) = (state.clone(), area.clone());
        click.connect_pressed(move |_, _, x, y| {
            let (w, h) = (a.width() as f64, a.height() as f64);
            let hit = {
                let st = s.borrow();
                strips(st.themes.len(), st.selected, w, h).iter().position(|(r, _, _)| x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h)
            };
            match hit {
                Some(i) if i == s.borrow().selected => apply(),
                Some(i) => {
                    s.borrow_mut().selected = i;
                    a.queue_draw();
                }
                None => close(),
            }
        });
        area.add_controller(click);
        Tray { area, state }
    }

    pub fn is_open(&self) -> bool {
        crate::motion::shown(&self.area)
    }

    /// Opens on `selected` (the theme in use) and takes keyboard focus.
    pub fn open(&self, themes: Vec<(String, Theme)>, selected: &str, current: Rc<Theme>) {
        {
            let mut st = self.state.borrow_mut();
            st.selected = themes.iter().position(|(k, _)| k == selected).unwrap_or(0);
            st.themes = themes;
            st.current = current;
        }
        crate::motion::fade_in(&self.area);
        self.area.queue_draw();
        self.area.grab_focus();
    }

    pub fn close(&self) {
        crate::motion::fade_out(&self.area);
    }
}

fn draw(cr: &gtk::cairo::Context, st: &State, w: f64, h: f64) -> Result<(), gtk::cairo::Error> {
    cr.set_source_rgba(0.0, 0.0, 0.0, DIM);
    cr.paint()?;
    for ((rect, distance, side), (_, theme)) in strips(st.themes.len(), st.selected, w, h).into_iter().zip(&st.themes) {
        cr.save()?;
        let alpha = if distance == 0 { 1.0 } else { (0.9 - 0.08 * distance as f64).max(0.35) };
        if distance > 0 {
            // A strip turned away from the viewer: its far edge drops (a shear about its centre).
            let (cx, cy) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
            cr.translate(cx, cy);
            cr.transform(gtk::cairo::Matrix::new(1.0, side * 0.12, 0.0, 1.0, 0.0, 0.0));
            cr.translate(-cx, -cy);
        }
        let radius = if distance == 0 { 10.0 } else { rect.w / 2.0 };
        rounded(cr, rect, radius);
        cr.clip();
        let band = rect.h / 3.0;
        for (i, colour) in theme.strip.iter().enumerate() {
            set(cr, *colour, alpha);
            cr.rectangle(rect.x, rect.y + i as f64 * band, rect.w, band + 0.5);
            cr.fill()?;
        }
        cr.reset_clip();
        if distance == 0 {
            // 2px ring in the theme's own accent.
            rounded(cr, Rect { x: rect.x - 3.0, y: rect.y - 3.0, w: rect.w + 6.0, h: rect.h + 6.0 }, radius + 3.0);
            set(cr, theme.accent, 1.0);
            cr.set_line_width(2.0);
            cr.stroke()?;
        }
        cr.restore()?;
    }
    draw_caption(cr, st, w, h)
}

/// Theme name, `n / total`, and key hints in boxes.
fn draw_caption(cr: &gtk::cairo::Context, st: &State, w: f64, h: f64) -> Result<(), gtk::cairo::Error> {
    let t = &*st.current;
    let Some((_, selected)) = st.themes.get(st.selected) else { return Ok(()) };
    let parts: Vec<(String, bool, bool)> = vec![
        (selected.name.clone(), true, false),
        (format!("{} / {}", st.selected + 1, st.themes.len()), false, false),
        ("← →".into(), false, true),
        ("browse".into(), false, false),
        ("Enter".into(), false, true),
        ("apply".into(), false, false),
        ("Esc".into(), false, true),
        ("close".into(), false, false),
    ];
    let layouts: Vec<(pango::Layout, bool)> = parts
        .iter()
        .map(|(text, bold, key)| {
            let layout = pangocairo::functions::create_layout(cr);
            let mut font = pango::FontDescription::new();
            font.set_family(&if *key { "monospace".to_string() } else { token::ui_font() });
            font.set_absolute_size(13.0 * if *key { 1.0 } else { token::ui_scale() } * pango::SCALE as f64);
            if *bold {
                font.set_weight(pango::Weight::Bold);
            }
            layout.set_font_description(Some(&font));
            layout.set_text(text);
            (layout, *key)
        })
        .collect();
    const SPACING: f64 = 10.0;
    const KEY_PAD: f64 = 6.0;
    let widths: Vec<f64> = layouts.iter().map(|(l, key)| l.pixel_size().0 as f64 + if *key { 2.0 * KEY_PAD } else { 0.0 }).collect();
    let text_w = widths.iter().sum::<f64>() + SPACING * (widths.len() - 1) as f64;
    let pill = caption_rect(w, h, text_w);
    rounded(cr, pill, pill.h / 2.0);
    set(cr, t.surface, 0.95);
    cr.fill()?;
    let mut x = pill.x + 16.0;
    for ((layout, key), width) in layouts.iter().zip(widths) {
        let (_, lh) = layout.pixel_size();
        let y = pill.y + (pill.h - lh as f64) / 2.0;
        if *key {
            rounded(cr, Rect { x, y: y - 2.0, w: width, h: lh as f64 + 4.0 }, 5.0);
            set(cr, t.muted, 0.6);
            cr.set_line_width(1.0);
            cr.stroke()?;
            set(cr, t.text, 1.0);
            cr.move_to(x + KEY_PAD, y);
        } else {
            set(cr, if x == pill.x + 16.0 { t.text } else { t.muted }, 1.0);
            cr.move_to(x, y);
        }
        pangocairo::functions::show_layout(cr, layout);
        x += width + SPACING;
    }
    Ok(())
}

fn rounded(cr: &gtk::cairo::Context, r: Rect, radius: f64) {
    let radius = radius.min(r.w / 2.0).min(r.h / 2.0);
    cr.new_sub_path();
    cr.arc(r.x + r.w - radius, r.y + radius, radius, -FRAC_PI_2, 0.0);
    cr.arc(r.x + r.w - radius, r.y + r.h - radius, radius, 0.0, FRAC_PI_2);
    cr.arc(r.x + radius, r.y + r.h - radius, radius, FRAC_PI_2, PI);
    cr.arc(r.x + radius, r.y + radius, radius, PI, PI + FRAC_PI_2);
    cr.close_path();
}

fn set(cr: &gtk::cairo::Context, (r, g, b): Rgb, alpha: f64) {
    cr.set_source_rgba(r, g, b, alpha);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_strip_is_large_and_the_row_is_centred() {
        let s = strips(5, 2, 1440.0, 810.0);
        assert_eq!((s[2].0.w, s[2].0.h), SELECTED);
        assert!(s.iter().enumerate().all(|(i, (r, _, _))| i == 2 || r.w == STRIP_W));
        let left = s[0].0.x;
        let right = s[4].0.x + s[4].0.w;
        assert!(((left + right) / 2.0 - 720.0).abs() < 1e-9);
        assert!(s[0].0.h < s[1].0.h, "further strips are smaller");
        assert!(s[2].0.y + s[2].0.h <= 810.0 - BOTTOM - CAPTION_H - CAPTION_GAP + 1e-9, "strips sit above the caption");
    }
}
