//! Remembered Whoosh peers.
//!
//! The file is one `address fingerprint` pair per line in
//! `~/.config/whoosh/known-peers`. The command-line sender and the macOS app
//! share it.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::native::{fingerprint_hex, Trust};

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

pub fn remembered_at(path: &Path, addr: &SocketAddr) -> Option<[u8; 32]> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(saved) = parts.next() else { continue };
        let Some(fingerprint) = parts.next() else {
            continue;
        };
        if saved == addr.to_string() {
            return parse_fingerprint(fingerprint);
        }
    }
    None
}

pub fn remember(addr: &SocketAddr, fingerprint: &[u8; 32]) -> Result<()> {
    remember_at(&peers_path()?, addr, fingerprint)
}

pub fn remember_at(path: &Path, addr: &SocketAddr, fingerprint: &[u8; 32]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let key = addr.to_string();
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = existing
        .lines()
        .filter(|line| !line.is_empty())
        .filter(|line| line.split_whitespace().next() != Some(key.as_str()))
        .map(str::to_string)
        .collect();
    lines.push(format!("{key} {}", fingerprint_hex(fingerprint)));
    std::fs::write(path, lines.join("\n") + "\n")?;
    Ok(())
}

pub fn parse_fingerprint(text: &str) -> Option<[u8; 32]> {
    let bytes = hex::decode(text.trim()).ok()?;
    bytes.try_into().ok()
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::{remember_at, remembered_at};

    #[test]
    fn replaces_a_saved_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known-peers");
        let addr: SocketAddr = "127.0.0.1:9".parse().unwrap();
        let mut first = [0u8; 32];
        first[0] = 1;
        let mut second = [0u8; 32];
        second[0] = 2;
        remember_at(&path, &addr, &first).unwrap();
        remember_at(&path, &addr, &second).unwrap();
        assert_eq!(remembered_at(&path, &addr), Some(second));
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 1);
    }
}
