//! mDNS advertisements for the native protocol, Quick Share, and AirDrop.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

use crate::error::{Error, Result};

pub struct Advertisement {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Advertisement {
    pub fn start(
        service_type: &str,
        instance: &str,
        port: u16,
        txt: &[(&str, &str)],
    ) -> Result<Self> {
        let daemon = ServiceDaemon::new().map_err(|error| Error::protocol(error.to_string()))?;
        let host = mdns_hostname();
        let info = ServiceInfo::new(service_type, instance, &host, "", port, txt)
            .map_err(|error| Error::protocol(error.to_string()))?
            .enable_addr_auto();
        let fullname = info.get_fullname().to_string();
        daemon
            .register(info)
            .map_err(|error| Error::protocol(error.to_string()))?;
        Ok(Self { daemon, fullname })
    }

    pub fn shutdown(self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

#[derive(Debug, Clone)]
pub struct FoundPeer {
    pub instance: String,
    pub addr: SocketAddr,
    pub txt: HashMap<String, String>,
}

pub async fn browse(service_type: &str, wait: Duration) -> Result<Vec<FoundPeer>> {
    let daemon = ServiceDaemon::new().map_err(|error| Error::protocol(error.to_string()))?;
    let receiver = daemon
        .browse(service_type)
        .map_err(|error| Error::protocol(error.to_string()))?;
    let deadline = tokio::time::Instant::now() + wait;
    let mut peers = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, receiver.recv_async()).await {
            Ok(Ok(ServiceEvent::ServiceResolved(info))) => {
                if let Some(peer) = convert(&info) {
                    if peers
                        .iter()
                        .any(|existing: &FoundPeer| existing.addr == peer.addr)
                    {
                        continue;
                    }
                    peers.push(peer);
                }
            }
            Ok(Ok(_)) => {}
            _ => break,
        }
    }
    let _ = daemon.shutdown();
    Ok(peers)
}

fn convert(info: &ServiceInfo) -> Option<FoundPeer> {
    let port = info.get_port();
    let addr = pick_addr(info.get_addresses(), port)?;
    let mut txt = HashMap::new();
    for key in ["n", "v", "fp", "flags"] {
        if let Some(value) = info.get_property_val_str(key) {
            txt.insert(key.to_string(), value.to_string());
        }
    }
    let instance = info
        .get_fullname()
        .split('.')
        .next()
        .unwrap_or("")
        .to_string();
    Some(FoundPeer {
        instance,
        addr,
        txt,
    })
}

fn pick_addr(addresses: &std::collections::HashSet<IpAddr>, port: u16) -> Option<SocketAddr> {
    let preferred = addresses
        .iter()
        .copied()
        .find(|addr| !addr.is_loopback() && !addr.is_multicast())
        .or_else(|| addresses.iter().copied().next())?;
    Some(SocketAddr::new(preferred, port))
}

fn mdns_hostname() -> String {
    let raw = hostname::get()
        .ok()
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| "whoosh".into());
    let mut label: String = raw
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
        .take(40)
        .collect();
    if label.is_empty() || label.starts_with('-') || label.ends_with('-') {
        label = "whoosh".into();
    }
    format!("{label}.local.")
}

pub fn device_name() -> String {
    hostname::get()
        .ok()
        .and_then(|value| value.into_string().ok())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Whoosh".into())
}
