// SPDX-License-Identifier: GPL-3.0-or-later
//! The weight meter: resident memory and CPU of one extension process, read from
//! `/proc` (an extension is a process, so its cost is measured, not estimated). Child
//! processes an extension starts itself are not counted.

use std::time::Instant;

#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub rss_kb: u64,
    /// User + system time in clock ticks.
    pub cpu_ticks: u64,
    pub at: Instant,
}

pub fn sample(pid: u32) -> Option<Sample> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    Some(Sample { rss_kb: rss_kb(&status)?, cpu_ticks: cpu_ticks(&stat)?, at: Instant::now() })
}

/// `utime + stime` (fields 14 and 15); the name in field 2 may hold spaces or `)`.
fn cpu_ticks(stat: &str) -> Option<u64> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    Some(fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?)
}

fn rss_kb(status: &str) -> Option<u64> {
    status.lines().find_map(|l| l.strip_prefix("VmRSS:")).and_then(|v| v.split_whitespace().next()).and_then(|n| n.parse().ok())
}

fn ticks_per_second() -> f64 {
    // SAFETY: sysconf has no preconditions.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks > 0 { ticks as f64 } else { 100.0 }
}

/// CPU use between two samples, in percent of one core.
pub fn cpu_percent(before: &Sample, after: &Sample) -> f64 {
    let seconds = after.at.duration_since(before.at).as_secs_f64();
    if seconds <= 0.0 {
        return 0.0;
    }
    after.cpu_ticks.saturating_sub(before.cpu_ticks) as f64 / ticks_per_second() / seconds * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_files() {
        let stat = "4242 (odd ) name) S 1 4242 4242 0 -1 4194304 100 0 0 0 37 5 0 0 20 0 1 0 1 1 1";
        assert_eq!(cpu_ticks(stat), Some(42));
        assert_eq!(rss_kb("Name:\tx\nVmRSS:\t   6120 kB\n"), Some(6120));
    }

    #[test]
    fn samples_this_process() {
        let s = sample(std::process::id()).expect("own /proc entry");
        assert!(s.rss_kb > 0);
    }
}
