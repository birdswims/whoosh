//! Extra listeners for interfaces the operating system keeps off the default route,
//! and the LAN address a phone can actually open.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV6};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceV4 {
    pub name: String,
    pub ip: Ipv4Addr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanAddress {
    pub interface: String,
    pub ip: Ipv4Addr,
}

/// Private IPv4 on a physical interface, preferring `en` / `eth` over other names.
pub fn lan_ipv4() -> Option<Ipv4Addr> {
    lan_address().map(|address| address.ip)
}

pub fn lan_address() -> Option<LanAddress> {
    let interfaces = if_addrs::get_if_addrs().ok()?;
    let candidates = interfaces
        .into_iter()
        .filter_map(|interface| match interface.addr {
            if_addrs::IfAddr::V4(address) => Some(InterfaceV4 {
                name: interface.name,
                ip: address.ip,
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    pick_lan_ipv4(&candidates).map(|chosen| LanAddress {
        interface: chosen.name.clone(),
        ip: chosen.ip,
    })
}

pub fn pick_lan_ipv4(candidates: &[InterfaceV4]) -> Option<&InterfaceV4> {
    candidates
        .iter()
        .filter(|candidate| usable_lan(candidate))
        .min_by_key(|candidate| {
            (
                address_rank(candidate.ip),
                interface_rank(&candidate.name),
                candidate.name.clone(),
                candidate.ip,
            )
        })
}

/// One line telling the operator which address a phone has to reach.
pub fn visibility_report() -> String {
    let Some(lan) = lan_address() else {
        return "no private IPv4 address. Quick Share needs the phone on a shared network. AirDrop from an iPhone uses AWDL and does not need that address.".into();
    };
    #[cfg(target_os = "macos")]
    {
        macos_visibility(&lan)
    }
    #[cfg(not(target_os = "macos"))]
    {
        format!(
            "lan address {} on {}. Quick Share needs the phone on a network that can reach it.",
            lan.ip, lan.interface
        )
    }
}

pub fn explain_visibility(
    lan: &LanAddress,
    wifi_interface: Option<&str>,
    ssid: Option<&str>,
) -> String {
    match (wifi_interface, ssid) {
        (Some(wifi), Some(ssid)) if wifi == lan.interface => format!(
            "lan address {} on {wifi} (Wi-Fi {ssid}). Put the phone on this Wi-Fi.",
            lan.ip
        ),
        (Some(wifi), Some(ssid)) => format!(
            "quick share address {} is on {}, and Wi-Fi {ssid} is on {wifi}. The phone must be able to reach {}.",
            lan.ip, lan.interface, lan.ip
        ),
        (Some(wifi), None) if wifi == lan.interface => format!(
            "lan address {} on {wifi}, the Wi-Fi interface. Put the phone on this same network.",
            lan.ip
        ),
        (Some(wifi), None) => format!(
            "lan address {} is on {}. Wi-Fi is {wifi}. Put the phone on the network that can reach {}.",
            lan.ip, lan.interface, lan.ip
        ),
        (None, _) => format!(
            "lan address {} on {}. Quick Share needs the phone on a network that can reach it. AirDrop from an iPhone uses AWDL.",
            lan.ip, lan.interface
        ),
    }
}

pub fn parse_wifi_device(hardware_ports: &str) -> Option<String> {
    let mut port = None;
    for line in hardware_ports.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Hardware Port:") {
            port = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("Device:") {
            let name = port.as_deref().unwrap_or("");
            if name.eq_ignore_ascii_case("Wi-Fi") || name.eq_ignore_ascii_case("AirPort") {
                let device = rest.trim();
                if !device.is_empty() {
                    return Some(device.to_string());
                }
            }
        }
    }
    None
}

pub fn parse_airport_ssid(output: &str) -> Option<String> {
    for line in output.lines() {
        let line = line.trim();
        let rest = line
            .strip_prefix("Current Wi-Fi Network:")
            .or_else(|| line.strip_prefix("Current Wi-Fi Network"))?;
        let ssid = rest.trim().trim_start_matches(':').trim();
        if !ssid.is_empty() {
            return Some(ssid.to_string());
        }
    }
    None
}

fn usable_lan(candidate: &InterfaceV4) -> bool {
    if skipped_interface(&candidate.name) {
        return false;
    }
    let ip = candidate.ip;
    !ip.is_unspecified()
        && !ip.is_loopback()
        && !ip.is_broadcast()
        && !ip.is_multicast()
        && !ip.is_link_local()
}

fn skipped_interface(name: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "lo",
        "utun",
        "awdl",
        "llw",
        "gif",
        "stf",
        "anpi",
        "pktap",
        "ipsec",
        "ppp",
        "tun",
        "tap",
        "wg",
        "docker",
        "vboxnet",
        "vmnet",
        "veth",
        "virbr",
        "br-",
        "zt",
        "tailscale",
        "nan",
    ];
    let name = name.to_ascii_lowercase();
    PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

fn address_rank(ip: Ipv4Addr) -> u8 {
    let [a, b, _, _] = ip.octets();
    if a == 10 || (a == 172 && (16..32).contains(&b)) || (a == 192 && b == 168) {
        0
    } else if a == 100 && (64..128).contains(&b) {
        1
    } else {
        2
    }
}

fn interface_rank(name: &str) -> u8 {
    let name = name.to_ascii_lowercase();
    if name.starts_with("en")
        || name.starts_with("eth")
        || name.starts_with("wlan")
        || name.starts_with("wl")
        || name.starts_with("wifi")
    {
        0
    } else if name.starts_with("ap") || name.starts_with("bridge") {
        1
    } else {
        2
    }
}

#[cfg(target_os = "macos")]
fn macos_visibility(lan: &LanAddress) -> String {
    let ports = std::process::Command::new("networksetup")
        .args(["-listallhardwareports"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned());
    let wifi = ports.as_deref().and_then(parse_wifi_device);
    let ssid = wifi.as_deref().and_then(|interface| {
        let output = std::process::Command::new("networksetup")
            .args(["-getairportnetwork", interface])
            .output()
            .ok()?;
        parse_airport_ssid(&String::from_utf8_lossy(&output.stdout))
    });
    explain_visibility(lan, wifi.as_deref(), ssid.as_deref())
}

/// IPv6 addresses currently assigned to `awdl0`, scoped to that interface.
///
/// AirDrop on an iPhone sends to the receiver over AWDL. A socket bound only to
/// `0.0.0.0` does not receive those packets.
#[cfg(target_os = "macos")]
pub fn awdl_listeners(port: u16) -> Vec<SocketAddr> {
    let Some(index) = interface_index("awdl0") else {
        return Vec::new();
    };
    let Ok(interfaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    interfaces
        .into_iter()
        .filter_map(|interface| {
            if interface.name != "awdl0" {
                return None;
            }
            match interface.addr {
                if_addrs::IfAddr::V6(address) => Some(SocketAddr::V6(SocketAddrV6::new(
                    address.ip, port, 0, index,
                ))),
                _ => None,
            }
        })
        .collect()
}

#[cfg(not(target_os = "macos"))]
pub fn awdl_listeners(_port: u16) -> Vec<SocketAddr> {
    Vec::new()
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn interface_index(name: &str) -> Option<u32> {
    let c_name = std::ffi::CString::new(name).ok()?;
    // `if_nametoindex` reads a NUL-terminated name and returns 0 when the
    // interface does not exist. The CString keeps that terminator.
    let index = unsafe { libc::if_nametoindex(c_name.as_ptr()) };
    if index == 0 {
        None
    } else {
        Some(index)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        explain_visibility, parse_airport_ssid, parse_wifi_device, pick_lan_ipv4, InterfaceV4,
        LanAddress,
    };
    use std::net::Ipv4Addr;

    fn iface(name: &str, ip: [u8; 4]) -> InterfaceV4 {
        InterfaceV4 {
            name: name.into(),
            ip: Ipv4Addr::new(ip[0], ip[1], ip[2], ip[3]),
        }
    }

    #[test]
    fn lan_pick_skips_tunnel_and_prefers_ethernet() {
        let candidates = vec![
            iface("utun4", [10, 0, 0, 2]),
            iface("lo0", [127, 0, 0, 1]),
            iface("awdl0", [169, 254, 1, 1]),
            iface("bridge0", [192, 168, 2, 1]),
            iface("en0", [192, 168, 1, 11]),
            iface("docker0", [172, 17, 0, 1]),
        ];
        let chosen = pick_lan_ipv4(&candidates).unwrap();
        assert_eq!(chosen.name, "en0");
        assert_eq!(chosen.ip, Ipv4Addr::new(192, 168, 1, 11));
    }

    #[test]
    fn lan_pick_uses_shared_address_when_that_is_all_there_is() {
        let candidates = vec![iface("eth0", [100, 64, 0, 8])];
        assert_eq!(
            pick_lan_ipv4(&candidates).unwrap().ip,
            Ipv4Addr::new(100, 64, 0, 8)
        );
    }

    #[test]
    fn wifi_port_and_ssid_parse() {
        let ports = "\
Hardware Port: Ethernet
Device: en8

Hardware Port: Wi-Fi
Device: en0
Ethernet Address: aa
";
        assert_eq!(parse_wifi_device(ports).as_deref(), Some("en0"));
        assert_eq!(
            parse_airport_ssid("Current Wi-Fi Network: Harry Net").as_deref(),
            Some("Harry Net")
        );
        assert_eq!(
            parse_airport_ssid("You are not associated with an AirPort network."),
            None
        );
    }

    #[test]
    fn visibility_text_covers_ethernet_and_wifi() {
        let lan = LanAddress {
            interface: "en0".into(),
            ip: Ipv4Addr::new(192, 168, 1, 11),
        };
        let joined = explain_visibility(&lan, Some("en0"), Some("Harry Net"));
        assert!(joined.contains("Harry Net"));
        let ethernet = explain_visibility(&lan, Some("en0"), None);
        assert!(ethernet.contains("Wi-Fi interface"));
        assert!(ethernet.contains("192.168.1.11"));
        assert!(!ethernet.contains("not joined"));
        let split = explain_visibility(&lan, Some("en1"), Some("Phone Net"));
        assert!(split.contains("en1"));
        assert!(split.contains("Phone Net"));
    }
}
