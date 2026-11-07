//! Validation of the local address outgoing download connections are bound to.

use std::net::IpAddr;

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
    fn loopback_is_local() {
        assert!(is_local_unicast_address(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    }

    #[test]
    fn address_of_an_active_interface_is_local() {
        let interfaces = if_addrs::get_if_addrs().expect("interfaces");
        let Some(local) = interfaces
            .iter()
            .filter(|interface| interface.is_oper_up() && !interface.is_loopback())
            .map(if_addrs::Interface::ip)
            .find(IpAddr::is_ipv4)
        else {
            return;
        };
        assert!(is_local_unicast_address(local), "{local}");
    }

    #[test]
    fn public_address_is_not_local_unicast() {
        assert!(!is_local_unicast_address(IpAddr::V4(Ipv4Addr::new(
            8, 8, 8, 8
        ))));
    }
}
