/// Whether torrent traffic can be pinned to one network device on this OS.
///
/// librqbit implements `bind_device_name` with `SO_BINDTODEVICE` (Linux) and
/// `IP(V6)_BOUND_IF` (macOS). On Windows the underlying
/// `BindDevice::new_from_name` returns `BindDeviceNotSupported` unconditionally
/// and session construction propagates that error — so setting the option
/// there does not merely fail to protect, it stops the session starting at
/// all. Windows gets detect-and-pause instead, and we say so rather than
/// implying a guarantee we cannot make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindSupport {
    Supported,
    Unsupported,
}

pub fn bind_support_for(os: &str) -> BindSupport {
    match os {
        "windows" => BindSupport::Unsupported,
        _ => BindSupport::Supported,
    }
}

pub fn bind_support() -> BindSupport {
    bind_support_for(std::env::consts::OS)
}

pub fn protection_summary(os: &str) -> &'static str {
    match bind_support_for(os) {
        BindSupport::Supported => {
            "Torrent traffic is bound to the VPN interface and cannot leave by another route."
        }
        BindSupport::Unsupported => {
            "This platform cannot bind traffic to an interface, so swarmling watches the VPN and \
             will pause every torrent if it drops. That is weaker: traffic can still leak in the \
             moments before a drop is noticed."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_platforms_support_binding() {
        assert_eq!(bind_support_for("linux"), BindSupport::Supported);
        assert_eq!(bind_support_for("macos"), BindSupport::Supported);
    }

    #[test]
    fn windows_does_not_support_binding() {
        assert_eq!(bind_support_for("windows"), BindSupport::Unsupported);
    }

    #[test]
    fn the_summary_is_honest_about_what_each_platform_gets() {
        assert!(protection_summary("linux").contains("bound"));
        let windows = protection_summary("windows");
        assert!(
            windows.contains("cannot"),
            "must not overstate Windows protection: {windows}"
        );
        assert!(windows.contains("pause"));
    }

    #[test]
    fn the_live_platform_agrees_with_the_table() {
        assert_eq!(bind_support(), bind_support_for(std::env::consts::OS));
    }
}
