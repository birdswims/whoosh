//! mDNS advertisements for the native protocol, Quick Share, and AirDrop.
//!
//! On macOS the advertiser is `dns-sd`, which talks to mDNSResponder. That
//! process already has Local Network permission. AirDrop is registered with
//! `-includeAWDL` so the AWDL address stays in the answer. The iPhone share
//! sheet lists `_companion-link._tcp`. Whoosh registers that type on its own
//! host (`whoosh-<port>.local`) at the Wi-Fi address, so the row is separate
//! from the Mac's computer name. Quick Share is registered only on the LAN
//! interface: a proxy hostname is not a record Android Nearby will keep.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

use crate::error::{Error, Result};

pub struct Advertisement {
    backend: Backend,
    detail: String,
}

enum Backend {
    Daemon {
        daemon: ServiceDaemon,
        fullname: String,
    },
    #[cfg(target_os = "macos")]
    System(Option<std::process::Child>),
    Stopped,
}

impl Advertisement {
    pub fn start(
        service_type: &str,
        instance: &str,
        port: u16,
        txt: &[(&str, &str)],
    ) -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            match system_advertisement(service_type, instance, port, txt) {
                Ok(advert) => return Ok(advert),
                Err(SystemStart::Missing) => {
                    tracing::warn!("dns-sd is not installed; using userspace mDNS");
                }
                Err(SystemStart::Failed(error)) => return Err(error),
            }
        }
        userspace_advertisement(service_type, instance, port, txt)
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        let backend = std::mem::replace(&mut self.backend, Backend::Stopped);
        match backend {
            Backend::Daemon { daemon, fullname } => {
                let _ = daemon.unregister(&fullname);
                let _ = daemon.shutdown();
            }
            #[cfg(target_os = "macos")]
            Backend::System(Some(mut child)) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            Backend::Stopped => {}
            #[cfg(target_os = "macos")]
            Backend::System(None) => {}
        }
    }
}

impl Drop for Advertisement {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsSdPlan {
    pub args: Vec<String>,
    pub summary: String,
}

/// Arguments for `dns-sd`. AirDrop stays a normal registration so mDNSResponder
/// can publish the AWDL address. Companion Link is a proxy on the Wi-Fi
/// address with its own hostname: the iPhone treats one hostname as one
/// AirDrop row, and the Mac already publishes the computer name on its
/// hostname. Quick Share is a normal registration on the LAN interface.
pub fn dns_sd_plan(
    service_type: &str,
    instance: &str,
    port: u16,
    txt: &[(&str, &str)],
    lan: Option<(&str, Ipv4Addr)>,
) -> Result<DnsSdPlan> {
    if instance.is_empty() || instance.len() > 63 || instance.bytes().any(|byte| byte == 0) {
        return Err(Error::protocol("mDNS instance name must be 1 to 63 bytes"));
    }
    let reg_type = dns_sd_type(service_type)?;
    let mut txt_args = Vec::with_capacity(txt.len());
    for (key, value) in txt {
        if key.is_empty()
            || !key.is_ascii()
            || key
                .bytes()
                .any(|byte| byte == b'=' || byte.is_ascii_whitespace())
        {
            return Err(Error::protocol("mDNS txt key is invalid"));
        }
        if value
            .chars()
            .any(|ch| ch == '\n' || ch == '\r' || ch == '\0')
        {
            return Err(Error::protocol("mDNS txt value is invalid"));
        }
        txt_args.push(format!("{key}={value}"));
    }
    let port_text = port.to_string();
    let mut args = Vec::new();
    let summary = if service_type.contains("_companion-link") {
        let (interface, ip) = lan
            .ok_or_else(|| Error::protocol("companion-link needs the Wi-Fi address of this Mac"))?;
        check_interface(interface)?;
        let host = format!("whoosh-{port}.local");
        args.push("-i".into());
        args.push(interface.to_string());
        args.push("-P".into());
        args.push(instance.to_string());
        args.push(reg_type);
        args.push("local".into());
        args.push(port_text);
        args.push(host);
        args.push(ip.to_string());
        args.extend(txt_args);
        format!("iPhone list on {interface} at {ip} as whoosh-{port}.local")
    } else if keeps_interface_addresses(service_type) || lan.is_none() {
        args.push("-includeAWDL".into());
        args.push("-R".into());
        args.push(instance.to_string());
        args.push(reg_type);
        args.push("local".into());
        args.push(port_text);
        args.extend(txt_args);
        "macOS mDNS including AWDL".to_string()
    } else {
        let (interface, ip) = lan.expect("lan address checked above");
        check_interface(interface)?;
        // A normal registration on the Wi-Fi interface. Android drops a proxy
        // hostname that does not belong to that interface.
        args.push("-i".into());
        args.push(interface.to_string());
        args.push("-R".into());
        args.push(instance.to_string());
        args.push(reg_type);
        args.push("local".into());
        args.push(port_text);
        args.extend(txt_args);
        format!("macOS mDNS on {interface} at {ip}")
    };
    Ok(DnsSdPlan { args, summary })
}

pub fn registration_confirmed(output: &str) -> bool {
    output
        .to_ascii_lowercase()
        .contains("registered and active")
}

fn keeps_interface_addresses(service_type: &str) -> bool {
    service_type.contains("_airdrop")
}

fn check_interface(interface: &str) -> Result<()> {
    if interface.is_empty()
        || !interface
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(Error::protocol("LAN interface name is invalid"));
    }
    Ok(())
}

fn dns_sd_type(service_type: &str) -> Result<String> {
    let trimmed = service_type.trim_matches('.');
    let (name, _) = trimmed
        .rsplit_once('.')
        .ok_or_else(|| Error::protocol("mDNS service type is invalid"))?;
    if !name.starts_with('_') || !name.contains("._") {
        return Err(Error::protocol(format!(
            "mDNS service type {service_type} is invalid"
        )));
    }
    Ok(name.to_string())
}

fn userspace_advertisement(
    service_type: &str,
    instance: &str,
    port: u16,
    txt: &[(&str, &str)],
) -> Result<Advertisement> {
    let companion = service_type.contains("_companion-link");
    let pinned = if keeps_interface_addresses(service_type) {
        None
    } else {
        crate::net::lan_ipv4()
    };
    // Companion Link must not reuse the computer's hostname. The iPhone folds
    // every record on that name into the macOS AirDrop row.
    let host = if companion || pinned.is_some() {
        format!("whoosh-{port}.local.")
    } else {
        mdns_hostname()
    };
    let ip = pinned.map(|addr| addr.to_string()).unwrap_or_default();
    let mut info = ServiceInfo::new(service_type, instance, &host, ip.as_str(), port, txt)
        .map_err(|error| Error::protocol(error.to_string()))?;
    if pinned.is_none() {
        info = info.enable_addr_auto();
    }
    let fullname = info.get_fullname().to_string();
    let daemon = ServiceDaemon::new().map_err(|error| Error::protocol(error.to_string()))?;
    daemon
        .register(info)
        .map_err(|error| Error::protocol(error.to_string()))?;
    let detail = match pinned {
        Some(addr) => format!("userspace mDNS pinned at {addr}"),
        None => format!("userspace mDNS {fullname}"),
    };
    Ok(Advertisement {
        backend: Backend::Daemon { daemon, fullname },
        detail,
    })
}

#[cfg(target_os = "macos")]
enum SystemStart {
    Missing,
    Failed(Error),
}

#[cfg(target_os = "macos")]
fn system_advertisement(
    service_type: &str,
    instance: &str,
    port: u16,
    txt: &[(&str, &str)],
) -> std::result::Result<Advertisement, SystemStart> {
    let lan = if keeps_interface_addresses(service_type) {
        None
    } else {
        crate::net::lan_address()
    };
    let plan = dns_sd_plan(
        service_type,
        instance,
        port,
        txt,
        lan.as_ref()
            .map(|address| (address.interface.as_str(), address.ip)),
    )
    .map_err(SystemStart::Failed)?;
    let mut command = std::process::Command::new("dns-sd");
    command
        .args(&plan.args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(SystemStart::Missing);
        }
        Err(error) => return Err(SystemStart::Failed(error.into())),
    };
    if let Err(error) = confirm_registration(&mut child, Duration::from_secs(4), instance) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(SystemStart::Failed(error));
    }
    Ok(Advertisement {
        backend: Backend::System(Some(child)),
        detail: plan.summary,
    })
}

#[cfg(target_os = "macos")]
fn confirm_registration(
    child: &mut std::process::Child,
    timeout: Duration,
    instance: &str,
) -> Result<()> {
    use std::io::{BufRead, BufReader};
    use std::sync::{Arc, Mutex};

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::protocol("dns-sd did not return stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::protocol("dns-sd did not return stderr"))?;
    let captured = Arc::new(Mutex::new(String::new()));
    spawn_drain(stdout, Arc::clone(&captured));
    spawn_drain(stderr, Arc::clone(&captured));
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let text = captured
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        if registration_confirmed(&text) && text.contains(instance) {
            return Ok(());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(Error::protocol(format!("dns-sd exited {status}: {text}")));
            }
            Ok(None) => {}
            Err(error) => return Err(error.into()),
        }
        if std::time::Instant::now() >= deadline {
            return Err(Error::protocol(format!(
                "dns-sd did not register the service: {text}"
            )));
        }
        std::thread::sleep(Duration::from_millis(30));
    }

    fn spawn_drain<R>(reader: R, captured: Arc<Mutex<String>>)
    where
        R: std::io::Read + Send + 'static,
    {
        std::thread::spawn(move || {
            let mut lines = BufReader::new(reader);
            let mut line = String::new();
            loop {
                line.clear();
                match lines.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let mut guard =
                            captured.lock().unwrap_or_else(|poison| poison.into_inner());
                        if guard.len() < 4096 {
                            guard.push_str(&line);
                        }
                    }
                }
            }
        });
    }
}

#[derive(Debug, Clone)]
pub struct FoundPeer {
    pub instance: String,
    pub fullname: String,
    pub addr: SocketAddr,
    pub txt: HashMap<String, String>,
}

pub enum PeerUpdate {
    Resolved { via: &'static str, peer: FoundPeer },
    Removed { fullname: String },
}

/// One mDNS browser for the whole time the app is open.
///
/// A fresh browse every couple of seconds misses devices that are still
/// nearby, because the reply often arrives after the window closes.
pub struct PeerBrowser {
    daemon: ServiceDaemon,
    events: tokio::sync::mpsc::UnboundedReceiver<PeerUpdate>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl PeerBrowser {
    pub fn open(services: &[(&str, &'static str)]) -> Result<Self> {
        let daemon = ServiceDaemon::new().map_err(|error| Error::protocol(error.to_string()))?;
        let (tx, events) = tokio::sync::mpsc::unbounded_channel();
        let mut tasks = Vec::new();
        for (service_type, via) in services {
            let receiver = match daemon.browse(service_type) {
                Ok(receiver) => receiver,
                Err(error) => {
                    tracing::warn!(%error, service = service_type, "browse failed");
                    continue;
                }
            };
            let tx = tx.clone();
            let via = *via;
            tasks.push(tokio::spawn(async move {
                while let Ok(event) = receiver.recv_async().await {
                    let update = match event {
                        ServiceEvent::ServiceResolved(info) => {
                            convert(&info).map(|peer| PeerUpdate::Resolved { via, peer })
                        }
                        ServiceEvent::ServiceRemoved(_, fullname) => {
                            Some(PeerUpdate::Removed { fullname })
                        }
                        _ => None,
                    };
                    let Some(update) = update else { continue };
                    if tx.send(update).is_err() {
                        break;
                    }
                }
            }));
        }
        if tasks.is_empty() {
            let _ = daemon.shutdown();
            return Err(Error::protocol("mDNS browse did not start"));
        }
        Ok(Self {
            daemon,
            events,
            tasks,
        })
    }

    pub async fn recv(&mut self) -> Option<PeerUpdate> {
        self.events.recv().await
    }
}

impl Drop for PeerBrowser {
    fn drop(&mut self) {
        let _ = self.daemon.shutdown();
        for task in &self.tasks {
            task.abort();
        }
    }
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
                    upsert_peer(&mut peers, peer);
                }
            }
            Ok(Ok(_)) => {}
            _ => break,
        }
    }
    let _ = daemon.shutdown();
    Ok(peers)
}

fn upsert_peer(peers: &mut Vec<FoundPeer>, peer: FoundPeer) {
    let index = peers
        .iter()
        .position(|existing| same_instance(existing, &peer));
    let Some(index) = index else {
        peers.push(peer);
        return;
    };
    if addr_rank(peer.addr.ip()) <= addr_rank(peers[index].addr.ip()) {
        peers[index] = peer;
    }
}

fn same_instance(existing: &FoundPeer, peer: &FoundPeer) -> bool {
    let instance = peer.instance.trim();
    if !instance.is_empty() && existing.instance.eq_ignore_ascii_case(instance) {
        return true;
    }
    existing.addr == peer.addr
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
    let fullname = info.get_fullname().to_string();
    let instance = fullname.split('.').next().unwrap_or("").to_string();
    Some(FoundPeer {
        instance,
        fullname,
        addr,
        txt,
    })
}

/// Prefer a normal Wi-Fi address over a link-local one.
///
/// Address sets arrive in a `HashSet`, whose iteration order changes between
/// browses. Ranking the address keeps one device on one row.
pub(crate) fn addr_rank(addr: IpAddr) -> u8 {
    match addr {
        IpAddr::V4(ip) if is_lan_v4(ip) => 0,
        IpAddr::V4(ip) if is_other_unicast_v4(ip) => 1,
        IpAddr::V6(ip) if is_ula(ip) => 2,
        IpAddr::V6(ip) if is_global_v6(ip) => 3,
        IpAddr::V4(ip) if ip.is_link_local() => 4,
        IpAddr::V6(ip) if ip.is_unicast_link_local() => 5,
        _ => 6,
    }
}

fn is_lan_v4(ip: Ipv4Addr) -> bool {
    let [first, second, _, _] = ip.octets();
    first == 10
        || (first == 172 && (16..=31).contains(&second))
        || (first == 192 && second == 168)
        || (first == 100 && (64..=127).contains(&second))
}

fn is_other_unicast_v4(ip: Ipv4Addr) -> bool {
    !ip.is_unspecified()
        && !ip.is_loopback()
        && !ip.is_multicast()
        && !ip.is_broadcast()
        && !ip.is_link_local()
        && !is_lan_v4(ip)
}

fn is_ula(ip: Ipv6Addr) -> bool {
    ip.octets()[0] & 0xfe == 0xfc
}

fn is_global_v6(ip: Ipv6Addr) -> bool {
    !ip.is_unspecified()
        && !ip.is_loopback()
        && !ip.is_multicast()
        && !ip.is_unicast_link_local()
        && !is_ula(ip)
}

fn pick_addr(addresses: &std::collections::HashSet<IpAddr>, port: u16) -> Option<SocketAddr> {
    addresses
        .iter()
        .copied()
        .min_by_key(|addr| (addr_rank(*addr), *addr))
        .map(|addr| SocketAddr::new(addr, port))
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

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    use super::Advertisement;
    use super::{dns_sd_plan, registration_confirmed};
    use std::net::Ipv4Addr;

    #[test]
    fn airdrop_registration_includes_awdl_and_keeps_interface_addresses() {
        let plan = dns_sd_plan(
            "_airdrop._tcp.local.",
            "aabbccddeeff",
            8770,
            &[("flags", "136")],
            Some(("en0", Ipv4Addr::new(192, 168, 1, 11))),
        )
        .unwrap();
        assert_eq!(
            plan.args,
            vec![
                "-includeAWDL",
                "-R",
                "aabbccddeeff",
                "_airdrop._tcp",
                "local",
                "8770",
                "flags=136",
            ]
        );
        assert!(!plan.args.iter().any(|arg| arg == "-i"));
    }

    #[test]
    fn airdrop_alt_is_registered_on_awdl() {
        let plan = dns_sd_plan(
            "_airdrop-alt._tcp.local.",
            "Harry's Mac",
            9,
            &[("flags", "140")],
            Some(("en0", Ipv4Addr::new(192, 168, 1, 11))),
        )
        .unwrap();
        assert!(plan.args.iter().any(|arg| arg == "-includeAWDL"));
        assert!(plan.args.iter().any(|arg| arg == "_airdrop-alt._tcp"));
        assert!(plan.args.iter().any(|arg| arg == "Harry's Mac"));
        assert!(!plan.args.iter().any(|arg| arg == "-i"));
    }

    #[test]
    fn companion_link_is_a_separate_device_on_the_lan() {
        let plan = dns_sd_plan(
            "_companion-link._tcp.local.",
            "Harry's Mac",
            52628,
            &[("rpFl", "0x20000"), ("rpVr", "715.2")],
            Some(("en0", Ipv4Addr::new(192, 168, 1, 11))),
        )
        .unwrap();
        assert_eq!(
            plan.args,
            vec![
                "-i",
                "en0",
                "-P",
                "Harry's Mac",
                "_companion-link._tcp",
                "local",
                "52628",
                "whoosh-52628.local",
                "192.168.1.11",
                "rpFl=0x20000",
                "rpVr=715.2",
            ]
        );
        assert!(plan.summary.contains("iPhone list"));
        assert!(!plan.args.iter().any(|arg| arg == "-includeAWDL"));
        assert!(!plan.args.iter().any(|arg| arg.contains("MacBook")));
    }

    #[test]
    fn companion_link_without_a_lan_address_is_rejected() {
        let error = dns_sd_plan(
            "_companion-link._tcp.local.",
            "Harry's Mac",
            9,
            &[("rpFl", "0x20000")],
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("Wi-Fi"));
    }

    #[test]
    fn quickshare_registration_pins_the_lan_address() {
        let plan = dns_sd_plan(
            "_FC9F5ED42C8A._tcp.local.",
            "I1VYVzD8n14AAA",
            53521,
            &[("n", "e30")],
            Some(("en0", Ipv4Addr::new(192, 168, 1, 11))),
        )
        .unwrap();
        assert_eq!(
            plan.args,
            vec![
                "-i",
                "en0",
                "-R",
                "I1VYVzD8n14AAA",
                "_FC9F5ED42C8A._tcp",
                "local",
                "53521",
                "n=e30",
            ]
        );
        assert!(plan.summary.contains("en0"));
        assert!(plan.summary.contains("192.168.1.11"));
        assert!(!plan.args.iter().any(|arg| arg == "-P"));
    }

    #[test]
    fn missing_lan_address_registers_without_a_proxy() {
        let plan = dns_sd_plan(
            "_whoosh._udp.local.",
            "abcd",
            1,
            &[("n", "Harry's Mac"), ("v", "1")],
            None,
        )
        .unwrap();
        assert_eq!(plan.args[1], "-R");
        assert!(plan.args.iter().any(|arg| arg == "n=Harry's Mac"));
        assert!(!plan.args.iter().any(|arg| arg == "-P"));
    }

    #[test]
    fn registration_line_is_the_mdnsresponder_success_text() {
        assert!(registration_confirmed(
            "12:00:00.000  Name now registered and active\n"
        ));
        assert!(!registration_confirmed("Registering Service whoosh\n"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_system_mdns_lists_airdrop_on_awdl() {
        let instance = format!(
            "ws{:010x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                & 0xFFFF_FFFF_FF
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let advert =
            Advertisement::start("_airdrop._tcp.local.", &instance, port, &[("flags", "136")])
                .unwrap_or_else(|error| panic!("register: {error}"));
        let output = dns_sd_output(
            &["-t", "4", "-i", "awdl0", "-B", "_airdrop._tcp", "local"],
            std::time::Duration::from_secs(8),
        );
        let text = String::from_utf8_lossy(&output);
        assert!(
            text.contains(&instance),
            "awdl browse missed {instance}: {text}"
        );
        drop(listener);
        advert.shutdown();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_system_mdns_pins_quickshare() {
        let Some(lan) = crate::net::lan_address() else {
            return;
        };
        let instance = crate::quickshare::service_instance_name("Ab12").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let advert = Advertisement::start(
            "_FC9F5ED42C8A._tcp.local.",
            &instance,
            port,
            &[("n", "e30")],
        )
        .unwrap_or_else(|error| panic!("register: {error}"));
        let output = dns_sd_output(
            &[
                "-t",
                "4",
                "-i",
                &lan.interface,
                "-L",
                &instance,
                "_FC9F5ED42C8A._tcp",
                "local",
            ],
            std::time::Duration::from_secs(8),
        );
        let text = String::from_utf8_lossy(&output);
        assert!(
            text.contains(&instance) && text.contains("n=e30") && text.contains(&port.to_string()),
            "quick share lookup missed {instance} on {}: {text}",
            lan.interface
        );
        drop(listener);
        advert.shutdown();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_companion_link_stays_beside_the_mac() {
        let Some(lan) = crate::net::lan_address() else {
            return;
        };
        let instance = format!(
            "Whoosh{:04x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                & 0xFFFF
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let advert = Advertisement::start(
            "_companion-link._tcp.local.",
            &instance,
            port,
            &[("rpFl", "0x20000"), ("rpBA", "AA:BB:CC:DD:EE:FF")],
        )
        .unwrap_or_else(|error| panic!("register: {error}"));
        let output = dns_sd_output(
            &[
                "-t",
                "4",
                "-i",
                &lan.interface,
                "-L",
                &instance,
                "_companion-link._tcp",
                "local",
            ],
            std::time::Duration::from_secs(8),
        );
        let text = String::from_utf8_lossy(&output);
        let host = format!("whoosh-{port}.local");
        assert!(
            text.contains(&instance)
                && text.contains(&host)
                && text.contains("rpFl=0x20000")
                && text.contains(&port.to_string()),
            "companion-link lookup missed {instance} on {}: {text}",
            lan.interface
        );
        drop(listener);
        advert.shutdown();
    }

    #[cfg(target_os = "macos")]
    fn dns_sd_output(args: &[&str], timeout: std::time::Duration) -> Vec<u8> {
        let mut child = std::process::Command::new("dns-sd")
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        loop {
            if child.try_wait().ok().flatten().is_some() {
                break;
            }
            if started.elapsed() > timeout {
                let _ = child.kill();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        let output = child.wait_with_output().unwrap();
        let mut bytes = output.stdout;
        bytes.extend_from_slice(&output.stderr);
        bytes
    }

    #[test]
    fn pick_addr_prefers_the_lan_address() {
        use std::collections::HashSet;
        use std::net::{IpAddr, Ipv6Addr, SocketAddr};

        let mut addresses = HashSet::new();
        addresses.insert(IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)));
        addresses.insert(IpAddr::V4(Ipv4Addr::new(169, 254, 4, 5)));
        addresses.insert(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)));
        let picked = super::pick_addr(&addresses, 5353).unwrap();
        assert_eq!(picked, SocketAddr::from(([192, 168, 1, 20], 5353)));
    }

    #[test]
    fn one_instance_keeps_its_lan_address() {
        use super::FoundPeer;
        use std::collections::HashMap;

        let link_local = FoundPeer {
            instance: "Phone".into(),
            fullname: "Phone._airdrop._tcp.local.".into(),
            addr: "[fe80::1]:8770".parse().unwrap(),
            txt: HashMap::new(),
        };
        let lan = FoundPeer {
            instance: "phone".into(),
            fullname: "phone._airdrop._tcp.local.".into(),
            addr: "192.168.1.5:8770".parse().unwrap(),
            txt: HashMap::new(),
        };
        let mut peers = vec![link_local];
        super::upsert_peer(&mut peers, lan);
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].addr, "192.168.1.5:8770".parse().unwrap());
        super::upsert_peer(
            &mut peers,
            FoundPeer {
                instance: "phone".into(),
                fullname: "phone._airdrop._tcp.local.".into(),
                addr: "[fe80::2]:8770".parse().unwrap(),
                txt: HashMap::new(),
            },
        );
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].addr, "192.168.1.5:8770".parse().unwrap());
    }
}
