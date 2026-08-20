use std::sync::Arc;

use super::adapter::{VpnAdapter, VpnError, VpnStatus};
use super::interfaces::{detect_vpn, Interface, InterfaceProvider};
use crate::util::command::CommandRunner;

const CLI: &str = "nordvpn";

/// Read the connection state out of `nordvpn status`. Output decoration varies
/// between versions, so match the words rather than the exact line.
pub fn parse_status(stdout: &str) -> Option<bool> {
    let lower = stdout.to_ascii_lowercase();
    if lower.contains("disconnected") {
        return Some(false);
    }
    if lower.contains("connected") {
        return Some(true);
    }
    None
}

/// Wraps the NordVPN CLI where it exists (Linux), and falls back to plain
/// interface detection where it does not (the macOS and Windows apps ship no
/// CLI). Credentials never pass through swarmling.
pub struct NordVpnAdapter {
    runner: Arc<dyn CommandRunner>,
    provider: Arc<dyn InterfaceProvider>,
}

impl NordVpnAdapter {
    pub fn new(runner: Arc<dyn CommandRunner>, provider: Arc<dyn InterfaceProvider>) -> Self {
        Self { runner, provider }
    }

    fn detected(&self) -> Option<Interface> {
        detect_vpn(self.provider.as_ref())
    }
}

#[async_trait::async_trait]
impl VpnAdapter for NordVpnAdapter {
    fn name(&self) -> &'static str {
        "nordvpn"
    }

    async fn status(&self) -> VpnStatus {
        let reported = match self.runner.run(CLI, &["status"]).await {
            Ok(out) => parse_status(&out),
            // No CLI on this platform (macOS and Windows ship none):
            // interface detection is all we have.
            Err(e) if e.is_absent() => None,
            // The CLI exists and failed — a dead `nordvpnd`, or the user not
            // in the right group. The authoritative source is unavailable,
            // so we must not let the name heuristic promote a stray tunnel
            // to "connected". Unknown is fail-closed for the guard.
            Err(e) => {
                return VpnStatus::Unknown(format!(
                    "`{CLI} status` could not be read: {}",
                    e.message()
                ))
            }
        };

        match reported {
            Some(false) => VpnStatus::Disconnected,
            Some(true) => match self.detected() {
                Some(interface) => VpnStatus::Connected { interface },
                // Connected but unnameable: we cannot bind to a tunnel we
                // cannot identify, so do not claim protection we can't give.
                None => VpnStatus::Unknown(
                    "nordvpn reports connected, but no tunnel interface was found".into(),
                ),
            },
            None => match self.detected() {
                Some(interface) => VpnStatus::Connected { interface },
                None => VpnStatus::Disconnected,
            },
        }
    }

    async fn connect(&self) -> Result<(), VpnError> {
        self.runner
            .run(CLI, &["connect"])
            .await
            .map(|_| ())
            .map_err(|e| {
                let message = format!("could not run `{CLI} connect`: {}", e.message());
                if e.is_absent() {
                    VpnError::Unsupported(message)
                } else {
                    VpnError::Control(message)
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::command::FakeCommands;
    use crate::vpn::interfaces::FakeInterfaces;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;

    fn iface(name: &str) -> Interface {
        Interface {
            name: name.into(),
            ip: IpAddr::V4(Ipv4Addr::new(10, 5, 0, 2)),
        }
    }

    #[test]
    fn parses_the_status_line_in_either_direction() {
        assert_eq!(parse_status("Status: Connected\nServer: x"), Some(true));
        assert_eq!(parse_status("  status: disconnected  "), Some(false));
        assert_eq!(parse_status("something else entirely"), None);
    }

    #[tokio::test]
    async fn connected_status_is_paired_with_the_interface() {
        let cmds = Arc::new(FakeCommands::new());
        cmds.respond("nordvpn", "Status: Connected\n");
        let ifaces = Arc::new(FakeInterfaces::new(vec![iface("en0"), iface("nordlynx")]));
        let adapter = NordVpnAdapter::new(cmds, ifaces);
        match adapter.status().await {
            VpnStatus::Connected { interface } => assert_eq!(interface.name, "nordlynx"),
            other => panic!("expected Connected, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn connected_without_a_nameable_interface_is_unknown_not_connected() {
        let cmds = Arc::new(FakeCommands::new());
        cmds.respond("nordvpn", "Status: Connected\n");
        let ifaces = Arc::new(FakeInterfaces::new(vec![iface("en0")]));
        let adapter = NordVpnAdapter::new(cmds, ifaces);
        assert!(matches!(adapter.status().await, VpnStatus::Unknown(_)));
    }

    #[tokio::test]
    async fn disconnected_is_reported_even_if_a_stray_tunnel_exists() {
        let cmds = Arc::new(FakeCommands::new());
        cmds.respond("nordvpn", "Status: Disconnected\n");
        let ifaces = Arc::new(FakeInterfaces::new(vec![iface("utun0")]));
        let adapter = NordVpnAdapter::new(cmds, ifaces);
        assert_eq!(adapter.status().await, VpnStatus::Disconnected);
    }

    #[tokio::test]
    async fn a_missing_cli_falls_back_to_interface_detection() {
        let cmds = Arc::new(FakeCommands::new());
        cmds.absent("nordvpn");
        let ifaces = Arc::new(FakeInterfaces::new(vec![iface("utun2")]));
        let adapter = NordVpnAdapter::new(cmds, ifaces);
        match adapter.status().await {
            VpnStatus::Connected { interface } => assert_eq!(interface.name, "utun2"),
            other => panic!("expected fallback Connected, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_cli_that_exists_but_fails_is_unknown_not_a_heuristic_guess() {
        // `nordvpnd` dead, or the user not in the `nordvpn` group. A stray
        // tunnel device must not be promoted to "connected" here.
        let cmds = Arc::new(FakeCommands::new());
        cmds.fail("nordvpn", "daemon is not running");
        let ifaces = Arc::new(FakeInterfaces::new(vec![iface("utun2")]));
        match NordVpnAdapter::new(cmds, ifaces).status().await {
            VpnStatus::Unknown(reason) => {
                assert!(reason.contains("daemon is not running"), "{reason}")
            }
            other => panic!("a failing CLI must fail closed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn connect_invokes_the_cli_once() {
        let cmds = Arc::new(FakeCommands::new());
        cmds.respond("nordvpn", "Connecting...\n");
        let ifaces = Arc::new(FakeInterfaces::new(vec![]));
        NordVpnAdapter::new(cmds.clone(), ifaces)
            .connect()
            .await
            .unwrap();
        assert_eq!(
            cmds.calls(),
            vec![("nordvpn".to_string(), vec!["connect".to_string()])]
        );
    }

    #[tokio::test]
    async fn connect_without_a_cli_is_unsupported() {
        let cmds = Arc::new(FakeCommands::new());
        cmds.absent("nordvpn");
        let ifaces = Arc::new(FakeInterfaces::new(vec![]));
        let err = NordVpnAdapter::new(cmds, ifaces)
            .connect()
            .await
            .unwrap_err();
        assert!(matches!(err, VpnError::Unsupported(_)));
    }

    #[tokio::test]
    async fn connect_with_a_failing_cli_is_a_control_error() {
        let cmds = Arc::new(FakeCommands::new());
        cmds.fail("nordvpn", "you are not logged in");
        let ifaces = Arc::new(FakeInterfaces::new(vec![]));
        let err = NordVpnAdapter::new(cmds, ifaces)
            .connect()
            .await
            .unwrap_err();
        assert!(matches!(err, VpnError::Control(_)), "got {err:?}");
    }
}
