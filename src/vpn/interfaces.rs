use std::net::IpAddr;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    pub ip: IpAddr,
}

pub trait InterfaceProvider: Send + Sync {
    fn interfaces(&self) -> Vec<Interface>;
}

/// Device-name prefixes that indicate a tunnel. Provider-specific names come
/// first: when several tunnels are up, a device we can attribute to a known
/// VPN is a better guess than a generic one.
/// `tap` is deliberately absent: `tapN` is overwhelmingly a VM or container
/// bridge device (QEMU/KVM, libvirt, VirtualBox) bridged straight onto the
/// LAN, and is only a VPN device for OpenVPN in the rare bridged mode. Like
/// `ppp`, its false positives all point the unsafe way.
pub const VPN_PREFIXES: &[&str] = &["nordlynx", "proton", "tun", "wg", "utun", "ipsec"];

const PROVIDER_PREFIXES: &[&str] = &["nordlynx", "proton"];

/// Prefixes that start with a VPN indicator but are NOT actually VPNs.
/// This escape hatch handles known collisions where a name-based heuristic
/// would produce a false positive: e.g. `tunl0` is a Linux IPIP tunnel for
/// container networking (Flannel, kube-proxy), not a VPN. Loopback (`lo`,
/// `lo0`) lives here too so every known-not-a-VPN name is in one place.
const NOT_VPN_PREFIXES: &[&str] = &["tunl", "lo"];

/// Whether an address is one a tunnel could actually carry traffic on.
///
/// A device that only has a loopback or link-local address is not routing
/// anything anywhere: on macOS the system creates `utun` devices carrying
/// nothing but an `fe80::` address for iCloud Private Relay, Wi-Fi Calling
/// and Handoff, and a machine with no VPN at all can have most of a dozen of
/// them. Binding to one of those would put torrent traffic on the bare
/// connection while we reported protection, so a name alone is never enough.
///
/// Private ranges are deliberately NOT excluded: real tunnels routinely carry
/// CGNAT (`100.64.0.0/10`) or IPv6 ULA (`fd00::/8`) addresses, and filtering
/// those would discard the genuine VPN and keep the impostors.
pub fn is_routable(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_link_local() && !v4.is_unspecified(),
        IpAddr::V6(v6) => !v6.is_loopback() && !v6.is_unicast_link_local() && !v6.is_unspecified(),
    }
}

pub fn is_vpn_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    // Exclude known false positives before checking VPN prefixes.
    if NOT_VPN_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return false;
    }
    VPN_PREFIXES.iter().any(|p| name.starts_with(p))
}

fn is_provider_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    PROVIDER_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Pick the interface torrent traffic should be bound to.
///
/// This is a heuristic: an operating system does not label a device "VPN", and
/// macOS uses `utun` for unrelated things such as iCloud Private Relay. A
/// provider adapter that can answer authoritatively should be preferred over
/// this function.
///
/// The heuristic errs toward NOT detecting a VPN. A false positive would
/// claim protection swarmling is not actually providing — a critical safety
/// issue — so an interface is only returned when the heuristic is relatively
/// confident. No real interface name or IP from the system is used; all tests
/// employ synthetic data.
pub fn detect_vpn(provider: &dyn InterfaceProvider) -> Option<Interface> {
    // Enumeration yields one entry per address, so a device with several
    // addresses appears several times. Keeping only the routable entries
    // therefore drops any device whose addresses are *all* loopback or
    // link-local, and keeps a device that has at least one usable address.
    let all: Vec<Interface> = provider
        .interfaces()
        .into_iter()
        .filter(|i| is_routable(&i.ip))
        .collect();
    all.iter()
        .find(|i| is_provider_name(&i.name))
        .cloned()
        .or_else(|| all.iter().find(|i| is_vpn_name(&i.name)).cloned())
}

pub struct SystemInterfaces;

impl InterfaceProvider for SystemInterfaces {
    fn interfaces(&self) -> Vec<Interface> {
        // A failure to enumerate means "no interfaces known", never a panic:
        // the caller treats that as "no VPN", which is the safe direction.
        if_addrs::get_if_addrs()
            .map(|list| {
                list.into_iter()
                    .map(|i| Interface {
                        name: i.name,
                        ip: i.addr.ip(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[derive(Default)]
pub struct FakeInterfaces {
    list: Mutex<Vec<Interface>>,
}

impl FakeInterfaces {
    pub fn new(list: Vec<Interface>) -> Self {
        Self {
            list: Mutex::new(list),
        }
    }
    pub fn set(&self, list: Vec<Interface>) {
        *self.list.lock().unwrap_or_else(|e| e.into_inner()) = list;
    }
}

impl InterfaceProvider for FakeInterfaces {
    fn interfaces(&self) -> Vec<Interface> {
        self.list.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    fn iface(name: &str, last: u8) -> Interface {
        Interface {
            name: name.into(),
            ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, last)),
        }
    }

    #[test]
    fn recognises_tunnel_device_names() {
        for name in ["tun0", "wg0", "utun3", "ipsec1", "nordlynx", "proton0"] {
            assert!(is_vpn_name(name), "{name} should count as a tunnel");
        }
    }

    #[test]
    fn rejects_ordinary_and_loopback_devices() {
        for name in [
            "eth0",
            "en0",
            "wlan0",
            "wlp3s0",
            "lo",
            "lo0",
            "docker0",
            "bridge100",
        ] {
            assert!(!is_vpn_name(name), "{name} must not count as a tunnel");
        }
    }

    #[test]
    fn detects_the_tunnel_among_ordinary_devices() {
        let p = FakeInterfaces::new(vec![iface("lo0", 1), iface("en0", 2), iface("utun4", 3)]);
        assert_eq!(detect_vpn(&p).unwrap().name, "utun4");
    }

    #[test]
    fn returns_none_when_no_tunnel_is_up() {
        let p = FakeInterfaces::new(vec![iface("lo0", 1), iface("en0", 2)]);
        assert!(detect_vpn(&p).is_none());
    }

    #[test]
    fn prefers_a_provider_specific_device_over_a_generic_one() {
        let p = FakeInterfaces::new(vec![iface("utun0", 1), iface("nordlynx", 2)]);
        assert_eq!(detect_vpn(&p).unwrap().name, "nordlynx");
    }

    #[test]
    fn detection_is_deterministic_when_several_generic_tunnels_exist() {
        let p = FakeInterfaces::new(vec![iface("utun0", 1), iface("utun1", 2)]);
        let first = detect_vpn(&p).unwrap().name;
        for _ in 0..10 {
            assert_eq!(detect_vpn(&p).unwrap().name, first);
        }
        assert_eq!(first, "utun0");
    }

    #[test]
    fn a_changed_interface_list_is_observed() {
        let p = FakeInterfaces::new(vec![iface("en0", 2)]);
        assert!(detect_vpn(&p).is_none());
        p.set(vec![iface("en0", 2), iface("wg0", 5)]);
        assert_eq!(detect_vpn(&p).unwrap().name, "wg0");
    }

    #[test]
    fn rejects_ppp0_which_is_often_a_dsl_connection() {
        assert!(!is_vpn_name("ppp0"), "ppp0 must not count as a VPN");
    }

    #[test]
    fn rejects_pppoe_wan_which_is_often_a_dsl_connection() {
        assert!(
            !is_vpn_name("pppoe-wan"),
            "pppoe-wan must not count as a VPN"
        );
    }

    #[test]
    fn rejects_tap0_which_is_usually_a_vm_bridge() {
        assert!(!is_vpn_name("tap0"), "tap0 must not count as a VPN");
        assert!(!is_vpn_name("tap-vm1"), "tap-vm1 must not count as a VPN");
    }

    #[test]
    fn rejects_tunl0_which_is_a_container_tunnel() {
        assert!(!is_vpn_name("tunl0"), "tunl0 must not count as a VPN");
    }

    #[test]
    fn confirms_real_vpn_prefixes_still_detected() {
        for name in ["tun0", "wg0", "utun3", "ipsec0", "nordlynx"] {
            assert!(
                is_vpn_name(name),
                "{name} should still be detected as a VPN"
            );
        }
    }

    fn iface_ip(name: &str, ip: IpAddr) -> Interface {
        Interface {
            name: name.into(),
            ip,
        }
    }

    #[test]
    fn ignores_tunnels_that_only_carry_a_link_local_address() {
        // Synthetic stand-in for the macOS system `utun` devices created by
        // Private Relay, Wi-Fi Calling and Handoff: tunnel-shaped names
        // carrying nothing but `fe80::`. Only the last device is a real VPN.
        let link_local = |n: u16| IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, n));
        let p = FakeInterfaces::new(vec![
            iface_ip("utun0", link_local(1)),
            iface_ip("utun1", link_local(2)),
            iface_ip("utun2", link_local(3)),
            iface_ip("en0", IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20))),
            iface_ip("utun7", IpAddr::V4(Ipv4Addr::new(100, 90, 12, 34))),
            iface_ip(
                "utun7",
                IpAddr::V6(Ipv6Addr::new(0xfd99, 0x1111, 0x2222, 0, 0, 0, 0, 1)),
            ),
        ]);
        assert_eq!(detect_vpn(&p).unwrap().name, "utun7");
    }

    #[test]
    fn a_link_local_only_machine_reports_no_vpn() {
        let p = FakeInterfaces::new(vec![
            iface_ip(
                "utun0",
                IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)),
            ),
            iface_ip("utun1", IpAddr::V4(Ipv4Addr::new(169, 254, 3, 4))),
            iface_ip("lo0", IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))),
        ]);
        assert!(detect_vpn(&p).is_none());
    }

    #[test]
    fn a_tunnel_with_a_private_or_cgnat_address_is_still_a_vpn() {
        for ip in [
            IpAddr::V4(Ipv4Addr::new(100, 90, 12, 34)),
            IpAddr::V4(Ipv4Addr::new(10, 8, 0, 6)),
            IpAddr::V6(Ipv6Addr::new(0xfd99, 0, 0, 0, 0, 0, 0, 2)),
        ] {
            let p = FakeInterfaces::new(vec![iface_ip("wg0", ip)]);
            assert_eq!(detect_vpn(&p).unwrap().name, "wg0", "{ip} should count");
        }
    }

    #[test]
    fn returns_none_when_only_dsl_is_available() {
        let p = FakeInterfaces::new(vec![iface("ppp0", 1), iface("en0", 2)]);
        assert!(detect_vpn(&p).is_none());
    }
}
