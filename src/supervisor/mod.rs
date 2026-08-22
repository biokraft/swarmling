//! Deciding when a torrent session may exist, and what must be in it.
//!
//! The rules that keep swarmling from ever moving bytes without a VPN are the
//! ones that have to be exactly right, so they live here as a pure
//! reconciler — observations in, instructions out, no IO — exactly as
//! `vpn::guard` does. That is what makes them reachable by tests that cannot
//! possibly start a transfer.

pub mod driver;
pub mod factory;
pub mod state;

pub use factory::SessionFactory;
pub use state::{Input, Intent, SessionOp, SessionState, Supervisor, Wanted};
