use super::adapter::VpnStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardState {
    Unprotected,
    Protected { device: String },
    Lost { previous_device: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardAction {
    None,
    Bind { device: String },
    PauseAll { reason: String },
    Rebind { device: String },
}

/// The kill switch, as a pure state machine: observations in, instructions
/// out, no I/O. This is the piece that has to be exactly right, so it is kept
/// where it can be exhaustively tested.
pub struct Guard {
    state: GuardState,
}

impl Default for Guard {
    fn default() -> Self {
        Self::new()
    }
}

impl Guard {
    pub fn new() -> Self {
        Self {
            state: GuardState::Unprotected,
        }
    }

    pub fn state(&self) -> &GuardState {
        &self.state
    }

    pub fn observe(&mut self, status: &VpnStatus) -> GuardAction {
        match (&self.state, status) {
            (GuardState::Unprotected, VpnStatus::Connected { interface }) => {
                self.state = GuardState::Protected {
                    device: interface.name.clone(),
                };
                GuardAction::Bind {
                    device: interface.name.clone(),
                }
            }
            (GuardState::Unprotected, _) => GuardAction::None,

            (GuardState::Protected { device }, VpnStatus::Connected { interface }) => {
                if *device == interface.name {
                    GuardAction::None
                } else {
                    self.state = GuardState::Protected {
                        device: interface.name.clone(),
                    };
                    GuardAction::Rebind {
                        device: interface.name.clone(),
                    }
                }
            }
            // Disconnected and Unknown are handled identically on purpose: if
            // we cannot confirm the tunnel is up, we stop. Fail closed.
            (GuardState::Protected { device }, status) => {
                let reason = match status {
                    VpnStatus::Disconnected => "the VPN disconnected".to_string(),
                    VpnStatus::Unknown(msg) => {
                        format!("the VPN state could not be confirmed: {msg}")
                    }
                    VpnStatus::Connected { .. } => unreachable!("handled above"),
                };
                self.state = GuardState::Lost {
                    previous_device: device.clone(),
                };
                GuardAction::PauseAll { reason }
            }

            (GuardState::Lost { .. }, VpnStatus::Connected { interface }) => {
                self.state = GuardState::Protected {
                    device: interface.name.clone(),
                };
                GuardAction::Rebind {
                    device: interface.name.clone(),
                }
            }
            (GuardState::Lost { .. }, _) => GuardAction::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vpn::interfaces::Interface;
    use std::net::{IpAddr, Ipv4Addr};

    fn connected(name: &str) -> VpnStatus {
        VpnStatus::Connected {
            interface: Interface {
                name: name.into(),
                ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
            },
        }
    }

    #[test]
    fn binds_when_a_vpn_appears() {
        let mut g = Guard::new();
        assert_eq!(
            g.observe(&connected("wg0")),
            GuardAction::Bind {
                device: "wg0".into()
            }
        );
        assert_eq!(
            g.state(),
            &GuardState::Protected {
                device: "wg0".into()
            }
        );
    }

    #[test]
    fn stays_quiet_while_the_same_vpn_is_up() {
        let mut g = Guard::new();
        g.observe(&connected("wg0"));
        assert_eq!(g.observe(&connected("wg0")), GuardAction::None);
        assert_eq!(g.observe(&connected("wg0")), GuardAction::None);
    }

    #[test]
    fn pauses_everything_the_moment_the_tunnel_drops() {
        let mut g = Guard::new();
        g.observe(&connected("wg0"));
        match g.observe(&VpnStatus::Disconnected) {
            GuardAction::PauseAll { .. } => {}
            other => panic!("expected PauseAll, got {other:?}"),
        }
        assert_eq!(
            g.state(),
            &GuardState::Lost {
                previous_device: "wg0".into()
            }
        );
    }

    #[test]
    fn an_unknown_status_while_protected_also_pauses() {
        let mut g = Guard::new();
        g.observe(&connected("wg0"));
        match g.observe(&VpnStatus::Unknown("cannot tell".into())) {
            GuardAction::PauseAll { .. } => {}
            other => panic!("unknown must fail closed, got {other:?}"),
        }
    }

    #[test]
    fn a_changed_device_forces_a_rebind() {
        let mut g = Guard::new();
        g.observe(&connected("wg0"));
        assert_eq!(
            g.observe(&connected("wg1")),
            GuardAction::Rebind {
                device: "wg1".into()
            }
        );
        assert_eq!(
            g.state(),
            &GuardState::Protected {
                device: "wg1".into()
            }
        );
    }

    #[test]
    fn recovers_by_rebinding_when_the_vpn_returns() {
        let mut g = Guard::new();
        g.observe(&connected("wg0"));
        g.observe(&VpnStatus::Disconnected);
        assert_eq!(
            g.observe(&connected("wg9")),
            GuardAction::Rebind {
                device: "wg9".into()
            }
        );
        assert_eq!(
            g.state(),
            &GuardState::Protected {
                device: "wg9".into()
            }
        );
    }

    #[test]
    fn never_binds_without_a_vpn() {
        let mut g = Guard::new();
        assert_eq!(g.observe(&VpnStatus::Disconnected), GuardAction::None);
        assert_eq!(
            g.observe(&VpnStatus::Unknown("?".into())),
            GuardAction::None
        );
        assert_eq!(g.state(), &GuardState::Unprotected);
    }

    #[test]
    fn stays_lost_until_the_vpn_actually_returns() {
        let mut g = Guard::new();
        g.observe(&connected("wg0"));
        g.observe(&VpnStatus::Disconnected);
        assert_eq!(g.observe(&VpnStatus::Disconnected), GuardAction::None);
        assert_eq!(
            g.observe(&VpnStatus::Unknown("?".into())),
            GuardAction::None
        );
        assert!(matches!(g.state(), GuardState::Lost { .. }));
    }
}
