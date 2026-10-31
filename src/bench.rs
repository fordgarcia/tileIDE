// SPDX-License-Identifier: GPL-3.0-or-later
//! Measurement helpers: drawer frame statistics, RSS, and the offscreen `render` command.

use std::time::Duration;

use gtk::prelude::*;
use gtk4 as gtk;
use gtk4::cairo;

use crate::draw;

/// Frame-to-frame intervals and draw cost while the drawer animates.
#[derive(Default)]
pub struct FrameStats {
    pub last_frame_us: Option<i64>,
    pub intervals_us: Vec<i64>,
    pub draw_times: Vec<Duration>,
}

pub fn print_stats(window: &gtk::ApplicationWindow, s: &FrameStats) {
    let refresh_mhz = window
        .surface()
        .and_then(|surface| WidgetExt::display(window).monitor_at_surface(&surface))
        .map(|m| m.refresh_rate())
        .filter(|r| *r > 0)
        .unwrap_or(60_000);
    let budget_us = 1_000_000_000 / refresh_mhz as i64;
    let mut intervals = s.intervals_us.clone();
    intervals.sort_unstable();
    let mut draws = s.draw_times.clone();
    draws.sort_unstable();
    if intervals.is_empty() || draws.is_empty() {
        println!("no animation frames recorded");
        return;
    }
    let ms = |us: i64| us as f64 / 1000.0;
    let dropped = intervals.iter().filter(|&&i| i as f64 > budget_us as f64 * 1.5).count();
    println!(
        "drawer animation @ {:.0} Hz (frame budget {:.2} ms): {} frames, interval median {:.2} ms, p95 {:.2} ms, max {:.2} ms, dropped (>1.5x budget) {} ({:.1}%)",
        refresh_mhz as f64 / 1000.0,
        ms(budget_us),
        intervals.len(),
        ms(intervals[intervals.len() / 2]),
        ms(intervals[intervals.len() * 95 / 100]),
        ms(*intervals.last().unwrap()),
        dropped,
        dropped as f64 * 100.0 / intervals.len() as f64,
    );
    println!(
        "draw callback (CPU): median {:.2} ms, max {:.2} ms",
        draws[draws.len() / 2].as_secs_f64() * 1000.0,
        draws.last().unwrap().as_secs_f64() * 1000.0
    );
}

pub fn rss() -> Option<String> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    Some(line.trim_start_matches("VmRSS:").trim().to_string())
}

/// Offscreen frame at 2x scale over a light checkerboard, so translucency and curves are visible.
pub fn render(args: &[String]) -> Result<(), String> {
    let usage = "usage: tileide render <out.png> <drawer 0..1> [--overlay] [background flags]";
    let out = args.first().ok_or(usage)?;
    let open: f64 = args.get(1).and_then(|p| p.parse().ok()).ok_or(usage)?;
    let (w, h, scale) = (1280.0, 720.0, 2.0);
    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, (w * scale) as i32, (h * scale) as i32).map_err(|e| e.to_string())?;
    let cr = cairo::Context::new(&surface).map_err(|e| e.to_string())?;
    cr.scale(scale, scale);
    let settings = crate::background::Settings::from_args(args)?;
    let tile_opacity = settings.tile_opacity;
    let mut backdrop = crate::background::Background::new(settings);
    if let Some(b) = backdrop.as_mut()
        && let Some(path) = b.image_path()
    {
        b.set_image(crate::background::decode(&path)?)?;
    }
    let background = backdrop.as_mut().and_then(|b| b.surface(w as i32, h as i32, scale).cloned());
    let scene = draw::Scene {
        width: w,
        height: h,
        solid: false,
        drawer: draw::token::DRAWER_WIDTH * open.clamp(0.0, 1.0),
        push: !args.iter().any(|a| a == "--overlay"),
        workspaces: &[1, 2, 3],
        active: Some(2),
        label: "tileIDE · 2",
        clock: ("21", "45"),
        empty: false,
        placeholder_tiles: true,
        background: background.as_ref(),
        tile_opacity,
    };
    cr.push_group();
    draw::draw(&cr, &scene).map_err(|e| e.to_string())?;
    let frame = cr.pop_group().map_err(|e| e.to_string())?;
    for y in 0..(h / 20.0) as i32 {
        for x in 0..(w / 20.0) as i32 {
            let shade = if (x + y) % 2 == 0 { 0.85 } else { 0.95 };
            cr.set_source_rgb(shade, shade, shade);
            cr.rectangle(x as f64 * 20.0, y as f64 * 20.0, 20.0, 20.0);
            cr.fill().map_err(|e| e.to_string())?;
        }
    }
    cr.set_source(&frame).map_err(|e| e.to_string())?;
    cr.paint().map_err(|e| e.to_string())?;
    drop(cr);
    let mut file = std::fs::File::create(out).map_err(|e| format!("{out}: {e}"))?;
    surface.write_to_png(&mut file).map_err(|e| e.to_string())
}
