//! Validation of the local address outgoing download connections are bound to.

use std::net::IpAddr;

/// Resolves the configured bind address to a usable local address.
///
/// `configured` is `None` when binding is disabled. Returns `None` when binding is disabled,
/// the value is blank or not an IP address, or no network interface that is up owns it.
pub fn resolve_bind_address(configured: Option<&str>) -> Option<IpAddr> {
    let address: IpAddr = configured?.trim().parse().ok()?;
    is_local_unicast_address(address).then_some(address)
}

/// Whether `address` is a unicast address assigned to a network interface that is up.
pub fn is_local_unicast_address(address: IpAddr) -> bool {
    match if_addrs::get_if_addrs() {
        Ok(interfaces) => interfaces
            .iter()
            .any(|interface| is_up(interface) && interface.ip() == address),
        Err(error) => {
            tracing::warn!("Unable to list the network interfaces: {error}");
            false
        }
    }
}

/// Loopback interfaces count as up even where the platform reports no operational status.
fn is_up(interface: &if_addrs::Interface) -> bool {
    interface.is_oper_up() || interface.is_loopback()
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    #[test]
    fn disabled_binding_resolves_to_none() {
        assert_eq!(resolve_bind_address(None), None);
    }

    #[test]
    fn blank_or_invalid_addresses_resolve_to_none() {
        for value in ["", "   ", "not-an-ip"] {
            assert_eq!(resolve_bind_address(Some(value)), None, "value {value:?}");
        }
    }

    #[test]
    fn address_of_no_local_interface_resolves_to_none() {
        assert_eq!(resolve_bind_address(Some("8.8.8.8")), None);
    }

    #[test]
    fn loopback_resolves_to_loopback() {
        assert_eq!(
            resolve_bind_address(Some("127.0.0.1")),
            Some(IpAddr::V4(Ipv4Addr::LOCALHOST))
        );
    }

    #[test]
    fn address_of_an_active_interface_resolves_to_itself() {
        let interfaces = if_addrs::get_if_addrs().expect("interfaces");
        let Some(local) = interfaces
            .iter()
            .filter(|interface| interface.is_oper_up() && !interface.is_loopback())
            .map(if_addrs::Interface::ip)
            .find(IpAddr::is_ipv4)
        else {
            return;
        };
        assert_eq!(resolve_bind_address(Some(&local.to_string())), Some(local));
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        assert_eq!(
            resolve_bind_address(Some(" 127.0.0.1 ")),
            Some(IpAddr::V4(Ipv4Addr::LOCALHOST))
        );
    }

    #[test]
    fn public_address_is_not_local_unicast() {
        assert!(!is_local_unicast_address(IpAddr::V4(Ipv4Addr::new(
            8, 8, 8, 8
        ))));
    }
}
