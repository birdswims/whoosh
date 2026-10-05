//! Remembered Whoosh peers.
//!
//! `~/.config/whoosh/known-peers` holds one peer per line:
//! `address fingerprint` or `address fingerprint name`.
//! The desktop apps and the command-line sender share the file.
//! A line whose address is `device:` plus the fingerprint records a computer
//! added from Settings before it has been seen on the network.
//! Trust checks the fingerprint, so a computer stays trusted when its port changes.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::native::{fingerprint_hex, Trust};

const DEVICE_PREFIX: &str = "device:";

/// How many other computers one advertisement names.
///
/// Each fingerprint is its own DNS-SD TXT string (`tf0`, `tf1`, …). A personal
/// LAN stays well under this. Past the limit, the extra computers do not see
/// this one as having added them until a slot frees.
const ANNOUNCED_TRUST_LIMIT: usize = 16;

pub fn config_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| Error::protocol("home directory is not set"))?;
    let dir = home.join(".config").join("whoosh");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn peers_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("known-peers"))
}

pub fn trust_for(addr: &SocketAddr, trust_first: bool) -> Result<Trust> {
    if let Some(fingerprint) = remembered(addr) {
        return Ok(Trust::Fingerprint(Some(fingerprint)));
    }
    if trust_first {
        return Ok(Trust::Fingerprint(None));
    }
    Err(Error::Untrusted(format!(
        "{addr} is not a remembered peer. Run again with --trust-first after checking the receiver's fingerprint."
    )))
}

pub fn remembered(addr: &SocketAddr) -> Option<[u8; 32]> {
    remembered_at(&peers_path().ok()?, addr)
}

/// True when this certificate was trusted from any address.
///
/// Clipboard access uses the fingerprint, not the current IP and port, so a
/// device stays yours after it picks a new port.
pub fn fingerprint_is_trusted(fingerprint: &[u8; 32]) -> bool {
    peers_path()
        .ok()
        .is_some_and(|path| fingerprint_is_trusted_at(&path, fingerprint))
}

pub fn fingerprint_is_trusted_at(path: &Path, fingerprint: &[u8; 32]) -> bool {
    load_peers(path)
        .iter()
        .any(|peer| same_fingerprint(&peer.fingerprint, fingerprint))
}

fn same_fingerprint(left: &[u8; 32], right: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

pub fn remembered_at(path: &Path, addr: &SocketAddr) -> Option<[u8; 32]> {
    let key = addr.to_string();
    load_peers(path)
        .into_iter()
        .find(|peer| peer.address == key)
        .map(|peer| peer.fingerprint)
}

pub fn remember(addr: &SocketAddr, fingerprint: &[u8; 32]) -> Result<()> {
    remember_named(addr, fingerprint, "")
}

pub fn remember_named(addr: &SocketAddr, fingerprint: &[u8; 32], name: &str) -> Result<()> {
    remember_at(&peers_path()?, addr, fingerprint, name)
}

pub fn remember_at(
    path: &Path,
    addr: &SocketAddr,
    fingerprint: &[u8; 32],
    name: &str,
) -> Result<()> {
    let mut peers = load_peers(path);
    let key = addr.to_string();
    let name = clean_trusted_name(name);
    if let Some(existing) = peers.iter_mut().find(|peer| peer.address == key) {
        existing.fingerprint = *fingerprint;
        if !name.is_empty() {
            existing.name = name;
        }
    } else {
        peers.push(SavedPeer {
            address: key,
            fingerprint: *fingerprint,
            name,
        });
    }
    write_peers(path, &peers)
}

/// One trusted computer, possibly remembered at several addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedDevice {
    pub fingerprint: String,
    pub name: String,
    pub addresses: Vec<String>,
}

pub fn list_trusted() -> Vec<TrustedDevice> {
    peers_path()
        .map(|path| list_trusted_at(&path))
        .unwrap_or_default()
}

pub fn list_trusted_at(path: &Path) -> Vec<TrustedDevice> {
    let peers = load_peers(path);
    let mut order = Vec::<[u8; 32]>::new();
    for peer in &peers {
        if !order
            .iter()
            .any(|saved| same_fingerprint(saved, &peer.fingerprint))
        {
            order.push(peer.fingerprint);
        }
    }
    order
        .into_iter()
        .map(|fingerprint| {
            let rows: Vec<_> = peers
                .iter()
                .filter(|peer| same_fingerprint(&peer.fingerprint, &fingerprint))
                .collect();
            let name = rows
                .iter()
                .rev()
                .find(|peer| is_manual_key(&peer.address) && !peer.name.is_empty())
                .or_else(|| rows.iter().rev().find(|peer| !peer.name.is_empty()))
                .map(|peer| peer.name.clone())
                .unwrap_or_default();
            let addresses = rows
                .iter()
                .filter(|peer| !is_manual_key(&peer.address))
                .map(|peer| peer.address.clone())
                .collect();
            TrustedDevice {
                fingerprint: fingerprint_hex(&fingerprint),
                name,
                addresses,
            }
        })
        .collect()
}

/// Trust a fingerprint that may not be on the network right now.
pub fn trust_fingerprint(fingerprint: &[u8; 32], name: &str) -> Result<()> {
    trust_fingerprint_at(&peers_path()?, fingerprint, name)
}

pub fn trust_fingerprint_at(path: &Path, fingerprint: &[u8; 32], name: &str) -> Result<()> {
    let mut peers = load_peers(path);
    let key = manual_key(fingerprint);
    let name = clean_trusted_name(name);
    if let Some(existing) = peers.iter_mut().find(|peer| peer.address == key) {
        if !name.is_empty() {
            existing.name = name;
        }
    } else {
        peers.push(SavedPeer {
            address: key,
            fingerprint: *fingerprint,
            name,
        });
    }
    write_peers(path, &peers)
}

/// Drop every saved address for this fingerprint.
pub fn forget_fingerprint(fingerprint: &[u8; 32]) -> Result<()> {
    forget_fingerprint_at(&peers_path()?, fingerprint)
}

pub fn forget_fingerprint_at(path: &Path, fingerprint: &[u8; 32]) -> Result<()> {
    let peers = load_peers(path);
    let kept: Vec<_> = peers
        .into_iter()
        .filter(|peer| !same_fingerprint(&peer.fingerprint, fingerprint))
        .collect();
    write_peers(path, &kept)
}

/// TXT pairs for a Whoosh advertisement, including the computers this one has added.
pub fn native_txt(name: &str, own_fingerprint: &str) -> Vec<(String, String)> {
    let mut txt = vec![
        ("n".into(), name.to_string()),
        ("v".into(), "1".into()),
        ("fp".into(), own_fingerprint.to_string()),
    ];
    txt.extend(announced_trust(own_fingerprint));
    txt
}

/// `tf0`, `tf1`, … naming every saved fingerprint except this computer.
pub fn announced_trust(own_fingerprint: &str) -> Vec<(String, String)> {
    peers_path()
        .map(|path| announced_trust_at(&path, own_fingerprint))
        .unwrap_or_default()
}

pub fn announced_trust_at(path: &Path, own_fingerprint: &str) -> Vec<(String, String)> {
    let mut prints: Vec<String> = list_trusted_at(path)
        .into_iter()
        .map(|device| device.fingerprint.to_ascii_lowercase())
        .filter(|fingerprint| !fingerprint.eq_ignore_ascii_case(own_fingerprint))
        .collect();
    prints.sort();
    prints.dedup();
    prints
        .into_iter()
        .take(ANNOUNCED_TRUST_LIMIT)
        .enumerate()
        .map(|(index, fingerprint)| (format!("tf{index}"), fingerprint))
        .collect()
}

/// True when `pairs` is a Whoosh TXT set that names `own_fingerprint`.
pub fn announcement_trusts_us<'a>(
    pairs: impl IntoIterator<Item = (&'a str, &'a str)>,
    own_fingerprint: &str,
) -> bool {
    let Some(own) = normalize_fingerprint(own_fingerprint) else {
        return false;
    };
    pairs.into_iter().any(|(key, value)| {
        is_trust_announcement_key(key)
            && normalize_fingerprint(value).as_deref() == Some(own.as_str())
    })
}

fn normalize_fingerprint(text: &str) -> Option<String> {
    parse_fingerprint(text).map(|bytes| fingerprint_hex(&bytes))
}

fn is_trust_announcement_key(key: &str) -> bool {
    let Some(rest) = key.strip_prefix("tf") else {
        return false;
    };
    !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit())
}

pub fn parse_fingerprint(text: &str) -> Option<[u8; 32]> {
    let compact: String = text.chars().filter(|ch| ch.is_ascii_hexdigit()).collect();
    if compact.len() != 64 {
        return None;
    }
    let bytes = hex::decode(compact).ok()?;
    bytes.try_into().ok()
}

struct SavedPeer {
    address: String,
    fingerprint: [u8; 32],
    name: String,
}

fn load_peers(path: &Path) -> Vec<SavedPeer> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines().filter_map(parse_peer_line).collect()
}

fn parse_peer_line(line: &str) -> Option<SavedPeer> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let mut parts = line.split_whitespace();
    let address = parts.next()?.to_string();
    let fingerprint = parse_fingerprint(parts.next()?)?;
    let name = clean_trusted_name(&parts.collect::<Vec<_>>().join(" "));
    Some(SavedPeer {
        address,
        fingerprint,
        name,
    })
}

fn write_peers(path: &Path, peers: &[SavedPeer]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut body = String::new();
    for peer in peers {
        body.push_str(&peer.address);
        body.push(' ');
        body.push_str(&fingerprint_hex(&peer.fingerprint));
        if !peer.name.is_empty() {
            body.push(' ');
            body.push_str(&peer.name);
        }
        body.push('\n');
    }
    std::fs::write(path, body)?;
    Ok(())
}

fn clean_trusted_name(raw: &str) -> String {
    raw.chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .take(64)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn manual_key(fingerprint: &[u8; 32]) -> String {
    format!("{DEVICE_PREFIX}{}", fingerprint_hex(fingerprint))
}

fn is_manual_key(address: &str) -> bool {
    address.starts_with(DEVICE_PREFIX)
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::{
        announcement_trusts_us, announced_trust_at, fingerprint_hex, fingerprint_is_trusted_at,
        forget_fingerprint_at, list_trusted_at, parse_fingerprint, remember_at, remembered_at,
        trust_fingerprint_at,
    };

    fn fingerprint(byte: u8) -> [u8; 32] {
        let mut value = [0u8; 32];
        value[0] = byte;
        value
    }

    #[test]
    fn replaces_a_saved_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known-peers");
        let addr: SocketAddr = "127.0.0.1:9".parse().unwrap();
        let first = fingerprint(1);
        let second = fingerprint(2);
        remember_at(&path, &addr, &first, "").unwrap();
        remember_at(&path, &addr, &second, "").unwrap();
        assert_eq!(remembered_at(&path, &addr), Some(second));
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(fingerprint_is_trusted_at(&path, &second));
        assert!(!fingerprint_is_trusted_at(&path, &first));
    }

    #[test]
    fn keeps_a_name_when_the_address_is_saved_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known-peers");
        let addr: SocketAddr = "192.168.1.9:60528".parse().unwrap();
        remember_at(&path, &addr, &fingerprint(4), "Office\nMac").unwrap();
        remember_at(&path, &addr, &fingerprint(4), "").unwrap();
        let listed = list_trusted_at(&path);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Office Mac");
        assert_eq!(listed[0].addresses, vec![addr.to_string()]);
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(!text.contains('\n') || text.matches('\n').count() == 1);
    }

    #[test]
    fn lists_one_device_for_every_saved_address() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known-peers");
        let first: SocketAddr = "192.168.1.8:1".parse().unwrap();
        let second: SocketAddr = "192.168.1.9:2".parse().unwrap();
        let other: SocketAddr = "192.168.1.10:3".parse().unwrap();
        remember_at(&path, &first, &fingerprint(7), "Kitchen").unwrap();
        remember_at(&path, &second, &fingerprint(7), "").unwrap();
        remember_at(&path, &other, &fingerprint(8), "Phone").unwrap();
        let listed = list_trusted_at(&path);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].name, "Kitchen");
        assert_eq!(
            listed[0].addresses,
            vec![first.to_string(), second.to_string()]
        );
        assert_eq!(listed[1].name, "Phone");
        assert!(fingerprint_is_trusted_at(&path, &fingerprint(7)));
    }

    #[test]
    fn adds_and_removes_a_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known-peers");
        let addr: SocketAddr = "192.168.1.12:4".parse().unwrap();
        let saved = fingerprint(9);
        remember_at(&path, &addr, &saved, "Old name").unwrap();
        trust_fingerprint_at(&path, &saved, "Studio").unwrap();
        let listed = list_trusted_at(&path);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Studio");
        assert_eq!(listed[0].addresses, vec![addr.to_string()]);
        assert!(fingerprint_is_trusted_at(&path, &saved));
        forget_fingerprint_at(&path, &saved).unwrap();
        assert!(list_trusted_at(&path).is_empty());
        assert!(!fingerprint_is_trusted_at(&path, &saved));
        assert!(remembered_at(&path, &addr).is_none());
    }

    #[test]
    fn reads_a_grouped_fingerprint() {
        let grouped = "ab12 cd34 ef56 ab12 cd34 ef56 ab12 cd34\nef56 ab12 cd34 ef56 ab12 cd34 ef56 ab12";
        let parsed = parse_fingerprint(grouped).unwrap();
        assert_eq!(parsed[0], 0xab);
        assert_eq!(parsed[1], 0x12);
        assert!(parse_fingerprint("ab12").is_none());
        assert!(parse_fingerprint(&format!("{grouped} ab12")).is_none());
    }

    #[test]
    fn announcement_names_every_other_computer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known-peers");
        let own = fingerprint(1);
        let first = fingerprint(3);
        let second = fingerprint(2);
        trust_fingerprint_at(&path, &own, "This computer").unwrap();
        trust_fingerprint_at(&path, &first, "Mac").unwrap();
        trust_fingerprint_at(&path, &second, "Other").unwrap();
        let pairs = announced_trust_at(&path, &fingerprint_hex(&own));
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, "tf0");
        assert_eq!(pairs[1].0, "tf1");
        assert!(pairs[0].1 < pairs[1].1);
        assert!(!pairs.iter().any(|(_, value)| value == &fingerprint_hex(&own)));
        let own_text = fingerprint_hex(&own);
        assert!(!announcement_trusts_us(
            pairs.iter().map(|(key, value)| (key.as_str(), value.as_str())),
            &own_text,
        ));
        assert!(announcement_trusts_us(
            [("tf0", pairs[0].1.as_str())],
            &pairs[0].1,
        ));
        assert!(!announcement_trusts_us([("n", pairs[0].1.as_str())], &pairs[0].1));
        assert!(!announcement_trusts_us([("tf", pairs[0].1.as_str())], &pairs[0].1));
    }

    #[test]
    fn announcement_stops_after_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known-peers");
        for byte in 1..20 {
            trust_fingerprint_at(&path, &fingerprint(byte), "").unwrap();
        }
        let pairs = announced_trust_at(&path, &fingerprint_hex(&fingerprint(1)));
        assert_eq!(pairs.len(), 16);
        assert_eq!(pairs[0].0, "tf0");
        assert_eq!(pairs[15].0, "tf15");
    }
}
