use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};

use crate::airdrop::{interoperable_connector, send_plain, send_tls};
use crate::discover::{self, device_name};
use crate::error::{Error, Result};
use crate::native::{self, fingerprint_hex, Trust};
use crate::quickshare::{self, SERVICE_TYPE as QUICKSHARE_SERVICE};
use crate::service::{run, DaemonConfig};

#[derive(Parser)]
#[command(
    name = "crossdrop",
    version,
    about = "Send files, photos, and videos between macOS, Windows, and Linux",
    arg_required_else_help = true
)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Receive with Crossdrop, Android Quick Share, and Apple AirDrop.
    Receive {
        #[arg(long, default_value = "Crossdrop")]
        dir: PathBuf,
        #[arg(long)]
        name: Option<String>,
        /// Accept every transfer without a prompt.
        #[arg(long)]
        yes: bool,
        /// Require this process's printed pin for native transfers.
        #[arg(long)]
        require_pin: bool,
        /// Put photos, videos, and audio in their own folders.
        #[arg(long)]
        sort_media: bool,
        #[arg(long)]
        no_native: bool,
        #[arg(long)]
        no_quickshare: bool,
        #[arg(long)]
        no_airdrop: bool,
        /// Largest accepted file, in mebibytes.
        #[arg(long, default_value_t = 8192)]
        max_mib: u64,
    },
    /// Send to another Crossdrop app over QUIC.
    Send {
        /// `host:port` or a discovered device name.
        #[arg(long)]
        to: String,
        #[arg(required = true)]
        files: Vec<PathBuf>,
        #[arg(long)]
        pin: Option<String>,
        /// Remember the peer certificate on first use.
        #[arg(long)]
        trust_first: bool,
    },
    /// Send to an Android Quick Share receiver on the same Wi-Fi.
    Quickshare {
        #[arg(long)]
        to: String,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Send to an AirDrop receiver.
    Airdrop {
        #[arg(long)]
        to: String,
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Speak HTTP instead of HTTPS. Real iPhones require HTTPS.
        #[arg(long)]
        http: bool,
    },
    /// List Crossdrop, Quick Share, and AirDrop devices for a few seconds.
    Discover {
        #[arg(long, default_value_t = 3)]
        seconds: u64,
    },
}

pub async fn execute() -> Result<()> {
    match Cli::parse().command {
        Command::Receive {
            dir,
            name,
            yes,
            require_pin,
            sort_media,
            no_native,
            no_quickshare,
            no_airdrop,
            max_mib,
        } => {
            let mut config = DaemonConfig::receive(name.unwrap_or_else(device_name), dir, yes);
            config.require_pin = require_pin;
            config.sort_media = sort_media;
            config.native = !no_native;
            config.quickshare = !no_quickshare;
            config.airdrop = !no_airdrop;
            config.max_file_bytes = max_mib.saturating_mul(1024 * 1024);
            run(config).await
        }
        Command::Send {
            to,
            files,
            pin,
            trust_first,
        } => {
            let addr = resolve_native(&to).await?;
            let trust = trust_for(&addr, trust_first)?;
            let report =
                native::send_files(addr, &device_name(), &files, pin.as_deref(), trust).await?;
            if trust_first {
                remember(&addr, &report.fingerprint)?;
            }
            println!(
                "sent {} file(s), {} bytes, fingerprint {}",
                report.files,
                report.bytes,
                fingerprint_hex(&report.fingerprint)
            );
            Ok(())
        }
        Command::Quickshare { to, files } => {
            let addr = resolve_service(QUICKSHARE_SERVICE, &to).await?;
            let done = quickshare::send_paths(addr, &device_name(), &files).await?;
            println!("sent {} bytes", done.bytes);
            Ok(())
        }
        Command::Airdrop { to, files, http } => {
            let addr = resolve_service(crate::airdrop::SERVICE_TYPE, &to).await?;
            let mut payloads = Vec::new();
            for path in &files {
                let name = path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .ok_or(Error::UnsafeName)?
                    .to_string();
                payloads.push((name, std::fs::read(path)?));
            }
            if http {
                send_plain(addr, &device_name(), &payloads).await?;
            } else {
                let connector = interoperable_connector()?;
                send_tls(addr, &device_name(), &payloads, connector).await?;
            }
            println!("sent {} file(s)", payloads.len());
            Ok(())
        }
        Command::Discover { seconds } => {
            let wait = Duration::from_secs(seconds);
            println!("crossdrop");
            for peer in discover::browse(native::SERVICE_TYPE, wait).await? {
                let name = peer.txt.get("n").cloned().unwrap_or(peer.instance);
                println!(
                    "  {name}  {}  fp {}",
                    peer.addr,
                    peer.txt.get("fp").map(String::as_str).unwrap_or("-")
                );
            }
            println!("quick share");
            for peer in discover::browse(QUICKSHARE_SERVICE, wait).await? {
                let name = peer
                    .txt
                    .get("n")
                    .and_then(|value| {
                        quickshare::EndpointInfo::decode(&quickshare::decode_b64(value).ok()?).ok()
                    })
                    .and_then(|info| info.name)
                    .unwrap_or(peer.instance);
                println!("  {name}  {}", peer.addr);
            }
            println!("airdrop");
            for peer in discover::browse(crate::airdrop::SERVICE_TYPE, wait).await? {
                println!("  {}  {}", peer.instance, peer.addr);
            }
            Ok(())
        }
    }
}

async fn resolve_native(to: &str) -> Result<SocketAddr> {
    if let Ok(addr) = to.parse::<SocketAddr>() {
        return Ok(addr);
    }
    resolve_service(native::SERVICE_TYPE, to).await
}

async fn resolve_service(service: &str, to: &str) -> Result<SocketAddr> {
    if let Ok(addr) = to.parse::<SocketAddr>() {
        return Ok(addr);
    }
    let peers = discover::browse(service, Duration::from_secs(3)).await?;
    peers
        .into_iter()
        .find(|peer| peer.instance == to || peer.txt.get("n").is_some_and(|name| name == to))
        .map(|peer| peer.addr)
        .ok_or_else(|| Error::NotFound(to.to_string()))
}

fn trust_for(addr: &SocketAddr, trust_first: bool) -> Result<Trust> {
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

fn peers_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| Error::protocol("home directory is not set"))?;
    let dir = home.join(".config").join("crossdrop");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("known-peers"))
}

fn remembered(addr: &SocketAddr) -> Option<[u8; 32]> {
    let path = peers_path().ok()?;
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

fn remember(addr: &SocketAddr, fingerprint: &[u8; 32]) -> Result<()> {
    let path = peers_path()?;
    let mut lines = std::fs::read_to_string(&path).unwrap_or_default();
    if !lines.is_empty() && !lines.ends_with('\n') {
        lines.push('\n');
    }
    lines.push_str(&format!("{addr} {}\n", fingerprint_hex(fingerprint)));
    std::fs::write(path, lines)?;
    Ok(())
}

fn parse_fingerprint(text: &str) -> Option<[u8; 32]> {
    let bytes = hex::decode(text).ok()?;
    bytes.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::Parser;

    #[test]
    fn receive_and_send_parse() {
        assert!(Cli::try_parse_from(["crossdrop", "receive", "--yes", "--require-pin"]).is_ok());
        assert!(Cli::try_parse_from([
            "crossdrop",
            "send",
            "--to",
            "127.0.0.1:9",
            "--trust-first",
            "a.jpg"
        ])
        .is_ok());
        assert!(Cli::try_parse_from(["crossdrop", "discover"]).is_ok());
    }
}
