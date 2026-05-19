//! Wire types and framing for the cmux ↔ cmuxd protocol.
//!
//! See `DAEMON_PLAN.md` §6 for protocol semantics.
//!
//! Phase 1: skeleton only. Variants land in phase 2.

#![deny(unsafe_code)]

pub const PROTOCOL_VERSION: u32 = 1;

/// Placeholder until phase 2 fills in the real enum variants.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind")]
pub enum Request {
    Hello {
        client_version: String,
        want_protocol: u32,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind")]
pub enum Event {
    Welcome {
        server_version: String,
        protocol: u32,
        session_count: usize,
    },
}
