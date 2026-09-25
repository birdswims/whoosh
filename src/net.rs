//! Extra listeners for interfaces the operating system keeps off the default route.

use std::net::{SocketAddr, SocketAddrV6};

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
