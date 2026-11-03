// SPDX-License-Identifier: GPL-3.0-or-later
//! Language server client without a display (v0.3 phase 6): `Content-Length` framing
//! (from spike 7), the built-in server table, protocol messages and position
//! conversion. The app owns the editors and decides what to show.

pub mod client;
pub mod protocol;
pub mod rpc;
pub mod servers;
pub mod text;

pub use serde_json::{Value, json};
