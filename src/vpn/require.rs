use std::sync::Arc;

use crate::config::settings::Settings;
use crate::util::command::SystemCommands;
use crate::vpn::adapter::{GenericAdapter, VpnAdapter, VpnStatus};
use crate::vpn::interfaces::SystemInterfaces;
use crate::vpn::nordvpn::NordVpnAdapter;

/// Whether a download may proceed, given the user's setting and what the VPN
/// currently looks like. Pure so the policy can be tested without touching a
/// network interface or a process.
///
/// Only `Connected` permits the download: `Unknown` means we could not
/// confirm the tunnel, and a safety check that passes when it cannot tell is
/// not a safety check.
pub fn refusal_reason(vpn_required: bool, status: &VpnStatus) -> Option<String> {
    if !vpn_required {
        return None;
    }
    match status {
        VpnStatus::Connected { .. } => None,
        VpnStatus::Disconnected => Some(
            "vpn_required is on and no VPN is up. Connect your VPN, or run \
             `swarmling vpn require off` to drop the requirement."
                .to_string(),
        ),
        VpnStatus::Unknown(reason) => Some(format!(
            "vpn_required is on and the VPN state could not be confirmed ({reason}). \
             Refusing rather than guessing."
        )),
    }
}

/// Build the adapter the settings ask for. Read-only: no `connect()` call,
/// and no torrent session is ever constructed here.
pub fn adapter_for(settings: &Settings) -> Box<dyn VpnAdapter> {
    let provider = Arc::new(SystemInterfaces);
    if settings.vpn_adapter == "nordvpn" {
        Box::new(NordVpnAdapter::new(Arc::new(SystemCommands), provider))
    } else {
        Box::new(GenericAdapter::new(provider))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vpn::interfaces::Interface;
    use std::net::{IpAddr, Ipv4Addr};

    fn connected() -> VpnStatus {
        VpnStatus::Connected {
            interface: Interface {
                name: "wg0".into(),
                ip: IpAddr::V4(Ipv4Addr::new(10, 8, 0, 2)),
            },
        }
    }

    #[test]
    fn nothing_is_refused_when_the_requirement_is_off() {
        for status in [
            connected(),
            VpnStatus::Disconnected,
            VpnStatus::Unknown("?".into()),
        ] {
            assert_eq!(refusal_reason(false, &status), None);
        }
    }

    #[test]
    fn a_live_vpn_permits_the_download() {
        assert_eq!(refusal_reason(true, &connected()), None);
    }

    #[test]
    fn no_vpn_refuses_the_download() {
        let reason = refusal_reason(true, &VpnStatus::Disconnected).expect("must refuse");
        assert!(reason.contains("vpn_required"), "{reason}");
    }

    #[test]
    fn an_unconfirmable_vpn_refuses_rather_than_guessing() {
        let reason =
            refusal_reason(true, &VpnStatus::Unknown("daemon dead".into())).expect("must refuse");
        assert!(reason.contains("daemon dead"), "{reason}");
    }
}
