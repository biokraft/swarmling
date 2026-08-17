use std::sync::Arc;

use super::interfaces::{detect_vpn, Interface, InterfaceProvider};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VpnStatus {
    Connected { interface: Interface },
    Disconnected,
    Unknown(String),
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum VpnError {
    #[error("this VPN adapter cannot connect for you: {0}")]
    Unsupported(String),
    #[error("VPN control failed: {0}")]
    Control(String),
}

#[async_trait::async_trait]
pub trait VpnAdapter: Send + Sync {
    fn name(&self) -> &'static str;
    async fn status(&self) -> VpnStatus;
    async fn connect(&self) -> Result<(), VpnError>;
}

/// Works with every VPN, because it only looks at network interfaces. This is
/// the fallback whenever no provider-specific adapter applies.
pub struct GenericAdapter {
    provider: Arc<dyn InterfaceProvider>,
}

impl GenericAdapter {
    pub fn new(provider: Arc<dyn InterfaceProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait::async_trait]
impl VpnAdapter for GenericAdapter {
    fn name(&self) -> &'static str {
        "generic"
    }

    async fn status(&self) -> VpnStatus {
        match detect_vpn(self.provider.as_ref()) {
            Some(interface) => VpnStatus::Connected { interface },
            None => VpnStatus::Disconnected,
        }
    }

    async fn connect(&self) -> Result<(), VpnError> {
        Err(VpnError::Unsupported(
            "the generic adapter can see a VPN but cannot start one; connect with your VPN client"
                .into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vpn::interfaces::FakeInterfaces;
    use std::net::{IpAddr, Ipv4Addr};

    fn iface(name: &str) -> Interface {
        Interface {
            name: name.into(),
            ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
        }
    }

    #[tokio::test]
    async fn reports_connected_when_a_tunnel_is_present() {
        let p = Arc::new(FakeInterfaces::new(vec![iface("en0"), iface("wg0")]));
        let adapter = GenericAdapter::new(p);
        match adapter.status().await {
            VpnStatus::Connected { interface } => assert_eq!(interface.name, "wg0"),
            other => panic!("expected Connected, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn reports_disconnected_with_no_tunnel() {
        let p = Arc::new(FakeInterfaces::new(vec![iface("en0")]));
        assert_eq!(
            GenericAdapter::new(p).status().await,
            VpnStatus::Disconnected
        );
    }

    #[tokio::test]
    async fn generic_adapter_cannot_connect_and_says_so() {
        let p = Arc::new(FakeInterfaces::new(vec![]));
        let err = GenericAdapter::new(p).connect().await.unwrap_err();
        assert!(matches!(err, VpnError::Unsupported(_)));
    }

    #[tokio::test]
    async fn status_follows_the_interface_list() {
        let p = Arc::new(FakeInterfaces::new(vec![iface("en0")]));
        let adapter = GenericAdapter::new(p.clone());
        assert_eq!(adapter.status().await, VpnStatus::Disconnected);
        p.set(vec![iface("en0"), iface("nordlynx")]);
        assert!(matches!(
            adapter.status().await,
            VpnStatus::Connected { .. }
        ));
    }
}
