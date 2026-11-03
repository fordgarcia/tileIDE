// SPDX-License-Identifier: GPL-3.0-or-later
//! How an extension behaves in one workspace (v0.3 phase 8). Shared by the state file
//! (`tileide-config`) and the extension host (`tileide-extensions`).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Starts on its own activation events (a language, a command, startup).
    #[default]
    OnDemand,
    /// Starts as soon as the workspace is entered.
    On,
    /// Never starts for this workspace; its commands are hidden there.
    Off,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::OnDemand, Mode::On, Mode::Off];

    pub fn key(self) -> &'static str {
        match self {
            Mode::OnDemand => "on-demand",
            Mode::On => "on",
            Mode::Off => "off",
        }
    }

    pub fn from_key(key: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|m| m.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::OnDemand => "On demand",
            Mode::On => "Always here",
            Mode::Off => "Off here",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip() {
        for m in Mode::ALL {
            assert_eq!(Mode::from_key(m.key()), Some(m));
        }
        assert_eq!(Mode::from_key("sometimes"), None);
    }
}
