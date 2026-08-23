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
        "linux" | "macos" => BindSupport::Supported,
        // Everything else — Windows, FreeBSD, Android, iOS — either has no
        // `BindDevice` implementation in librqbit or is untested here, so it
        // gets the honest weaker answer rather than a guarantee we cannot
        // keep.
        _ => BindSupport::Unsupported,
    }
}

pub fn bind_support() -> BindSupport {
    bind_support_for(std::env::consts::OS)
}

pub fn protection_summary(os: &str) -> &'static str {
    match bind_support_for(os) {
        BindSupport::Supported => {
            "This platform can bind torrent traffic to the VPN interface, so it cannot leave by \
             another route. No download session is running yet, so nothing is bound right now."
        }
        BindSupport::Unsupported => {
            "This platform cannot bind traffic to an interface, so swarmling watches the VPN and \
             stops every torrent if it drops, closing the whole session rather than pausing it: \
             restarting a download re-checks what is already on disk. That is weaker in both \
             directions: traffic can still leak in the moments before a drop is noticed, and \
             nothing resumes by itself afterwards."
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
        let linux = protection_summary("linux");
        assert!(
            linux.contains("can bind"),
            "must be capability tense: {linux}"
        );
        assert!(
            !linux.contains("is bound to"),
            "must not claim a live binding that does not exist: {linux}"
        );
        let windows = protection_summary("windows");
        assert!(
            windows.contains("cannot"),
            "must not overstate Windows protection: {windows}"
        );
        assert!(
            windows.contains("stops every torrent"),
            "must describe the stop that actually happens: {windows}"
        );
        assert!(
            !windows.contains("pause"),
            "swarmling closes the session rather than pausing it, and a user who \
             expects to resume will instead watch everything re-check: {windows}"
        );
    }

    #[test]
    fn an_untested_platform_does_not_claim_bind_support() {
        assert_eq!(bind_support_for("freebsd"), BindSupport::Unsupported);
        assert_eq!(bind_support_for("android"), BindSupport::Unsupported);
    }

    #[test]
    fn the_live_platform_agrees_with_the_table() {
        assert_eq!(bind_support(), bind_support_for(std::env::consts::OS));
    }
}
