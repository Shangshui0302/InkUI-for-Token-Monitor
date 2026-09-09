use if_addrs::{get_if_addrs, IfAddr};
use std::env;
use std::io;
use std::net::Ipv4Addr;

const INTERFACE_ENV: &str = "CODEX_MONITOR_DISCOVERY_INTERFACE";

/// Select the IPv4 address used for multicast discovery.
///
/// An explicit interface address or name is useful for diagnostics and for
/// hosts with unusual routing. Automatic selection skips loopback, point to
/// point, virtual, and reserved tunnel interfaces so a VPN/TUN route cannot
/// swallow LAN multicast.
pub fn discovery_interface_v4() -> io::Result<Ipv4Addr> {
    let interfaces = get_if_addrs()?;
    if let Ok(value) = env::var(INTERFACE_ENV) {
        return explicit_interface(&interfaces, &value);
    }

    interfaces
        .iter()
        .filter_map(|interface| {
            let IfAddr::V4(address) = &interface.addr else {
                return None;
            };
            if !interface.is_oper_up()
                || interface.is_loopback()
                || interface.is_p2p()
                || address.broadcast.is_none()
                || is_reserved_or_tunnel(address.ip)
                || is_virtual_name(&interface.name)
            {
                return None;
            }

            Some((interface_score(&interface.name, address.ip), address.ip))
        })
        .min_by_key(|(score, ip)| (*score, *ip))
        .map(|(_, ip)| ip)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "no usable LAN IPv4 interface for multicast discovery",
            )
        })
}

fn explicit_interface(interfaces: &[if_addrs::Interface], value: &str) -> io::Result<Ipv4Addr> {
    if let Ok(ip) = value.parse::<Ipv4Addr>() {
        return validate_explicit_ip(ip);
    }

    interfaces
        .iter()
        .filter(|interface| interface.name.eq_ignore_ascii_case(value))
        .find_map(|interface| match &interface.addr {
            IfAddr::V4(address) => Some(address.ip),
            IfAddr::V6(_) => None,
        })
        .map_or_else(
            || {
                Err(io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    format!("discovery interface '{value}' was not found"),
                ))
            },
            validate_explicit_ip,
        )
}

fn validate_explicit_ip(ip: Ipv4Addr) -> io::Result<Ipv4Addr> {
    if ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid multicast discovery interface address: {ip}"),
        ));
    }
    Ok(ip)
}

fn interface_score(name: &str, ip: Ipv4Addr) -> (u8, u8) {
    let private_score = u8::from(!ip.is_private());
    let familiar_name_score = u8::from(!is_familiar_lan_name(name));
    (private_score, familiar_name_score)
}

fn is_reserved_or_tunnel(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_multicast()
        || (octets[0] == 198 && octets[1] == 18)
}

fn is_virtual_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "docker",
        "hyper-v",
        "mihomo",
        "meta",
        "tap",
        "tailscale",
        "tun",
        "utun",
        "vbox",
        "veth",
        "vEthernet",
        "virbr",
        "vmnet",
        "wireguard",
        "wg",
    ]
    .iter()
    .any(|token| name.contains(&token.to_ascii_lowercase()))
}

fn is_familiar_lan_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["eth", "en", "lan", "wi-fi", "wifi", "wlan"]
        .iter()
        .any(|token| name == *token || name.starts_with(token))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_tunnel_addresses_are_not_auto_candidates() {
        assert!(is_reserved_or_tunnel(Ipv4Addr::new(198, 18, 0, 1)));
        assert!(is_reserved_or_tunnel(Ipv4Addr::new(127, 0, 0, 1)));
        assert!(!is_reserved_or_tunnel(Ipv4Addr::new(10, 223, 203, 165)));
    }

    #[test]
    fn virtual_interface_names_are_skipped() {
        assert!(is_virtual_name("Meta"));
        assert!(is_virtual_name("vEthernet (WLAN)"));
        assert!(!is_virtual_name("wlp2s0"));
    }

    #[test]
    fn familiar_private_lan_addresses_win() {
        assert!(
            interface_score("wlp2s0", Ipv4Addr::new(10, 223, 203, 165))
                < interface_score("enp1s0", Ipv4Addr::new(198, 51, 100, 4))
        );
    }
}
