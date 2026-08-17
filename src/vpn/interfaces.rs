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
pub const VPN_PREFIXES: &[&str] = &[
    "nordlynx", "proton", "tun", "tap", "wg", "utun", "ppp", "ipsec",
];

const PROVIDER_PREFIXES: &[&str] = &["nordlynx", "proton"];

pub fn is_vpn_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    if name == "lo" || name.starts_with("lo0") {
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
pub fn detect_vpn(provider: &dyn InterfaceProvider) -> Option<Interface> {
    let all = provider.interfaces();
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
    use std::net::{IpAddr, Ipv4Addr};

    fn iface(name: &str, last: u8) -> Interface {
        Interface {
            name: name.into(),
            ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, last)),
        }
    }

    #[test]
    fn recognises_tunnel_device_names() {
        for name in [
            "tun0", "tap0", "wg0", "utun3", "ppp0", "ipsec1", "nordlynx", "proton0",
        ] {
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
}
