//! Modal overlays. Each submodule owns one popup. Anything reused across
//! popups (the dangerous-flag toggle row shared by spawn + picker) lives in
//! [`dangerous`].

pub(super) mod confirm_detach;
pub(super) mod daemon_lost;
pub(super) mod dangerous;
pub(super) mod help;
pub(super) mod picker;
pub(super) mod rename;
pub(super) mod spawn;
