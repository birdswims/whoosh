use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};

use crate::airdrop::{
    interoperable_connector, send_plain_reporting, send_tls_reporting, SendProgress,
};
use crate::discover::{self, device_name};
use crate::error::{Error, Result};
use crate::native::{self, fingerprint_hex};
use crate::progress::SendBar;
use crate::quickshare::{self, SERVICE_TYPE as QUICKSHARE_SERVICE};
use crate::service::{run, DaemonConfig};

#[derive(Parser)]
#[command(
    name = "whoosh",
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
    /// Receive with Whoosh, Android Quick Share, and Apple AirDrop.
    Receive {
        #[arg(long, default_value = "Whoosh")]
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
    /// Send to another Whoosh app over QUIC.
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
    /// List Whoosh, Quick Share, and AirDrop devices for a few seconds.
    Discover {
        #[arg(long, default_value_t = 3)]
        seconds: u64,
    },
    /// Speak the JSON control channel used by the desktop apps.
    App,
}

/// Points later logs away from the handle the desktop app is reading.
///
/// Call this before anything writes to stdout. `whoosh app` does it from `main`.
pub fn prepare_app_stdio() {
    crate::appctl::prepare_app_stdio();
}

/// Runs the command on a worker thread.
///
/// On macOS the calling thread pumps the main dispatch queue. AirDrop
/// discovery callbacks are delivered there, and a Tokio runtime parked on
/// this thread never drains them. Each pass runs for a couple of seconds:
/// shorter passes return before the phone has been asked to announce.
pub fn launch() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let (tx, rx) = std::sync::mpsc::channel();
    runtime.spawn(async move {
        let outcome = execute().await;
        let _ = tx.send(outcome);
        #[cfg(target_os = "macos")]
        crate::discover::stop_main_run_loop();
    });

    #[cfg(target_os = "macos")]
    {
        loop {
            match rx.try_recv() {
                Ok(Ok(())) => {
                    crate::discover::pump_main_run_loop(0.0);
                    break;
                }
                Ok(Err(error)) => {
                    crate::discover::pump_main_run_loop(0.0);
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    eprintln!("error: stopped");
                    std::process::exit(1);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    crate::discover::pump_main_run_loop(2.0);
                }
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    match rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("error: stopped");
            std::process::exit(1);
        }
    }
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
            let trust = crate::trust::trust_for(&addr, trust_first)?;
            let bar = SendBar::new(format!("to {to}"));
            let hook = bar.hook();
            bar.note(&format!("sending to {to}…"));
            let report = native::send_files_with_progress(
                addr,
                &device_name(),
                &files,
                pin.as_deref(),
                trust,
                hook,
            )
            .await;
            bar.finish();
            let report = report?;
            if trust_first {
                crate::trust::remember(&addr, &report.fingerprint)?;
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
            let bar = SendBar::new(format!("to {to}"));
            let hook = bar.hook();
            bar.note(&format!("sending to {to}…"));
            let done =
                quickshare::send_paths_with_progress(addr, &device_name(), &files, hook).await;
            bar.finish();
            let done = done?;
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
            let bar = SendBar::new(format!("to {to}"));
            let hook = bar.hook();
            let noted = bar.clone();
            let peer = to.clone();
            let stage = move |stage: SendProgress| note_airdrop(&noted, &peer, stage);
            let result = if http {
                send_plain_reporting(addr, &device_name(), &payloads, &stage, &|progress| {
                    hook(progress)
                })
                .await
            } else {
                let connector = interoperable_connector()?;
                send_tls_reporting(
                    addr,
                    &device_name(),
                    &payloads,
                    connector,
                    &stage,
                    &|progress| hook(progress),
                )
                .await
            };
            bar.finish();
            result?;
            println!("sent {} file(s)", payloads.len());
            Ok(())
        }
        Command::Discover { seconds } => {
            let wait = Duration::from_secs(seconds);
            println!("whoosh");
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
                let name = quickshare::nearby_label(
                    &peer.txt,
                    peer.txt_raw.get("n").map(Vec::as_slice),
                    &peer.instance,
                    &peer.hostname,
                );
                println!("  {name}  {}", peer.addr);
            }
            println!("airdrop");
            for peer in discover::browse(crate::airdrop::SERVICE_TYPE, wait).await? {
                let recorded = peer
                    .txt
                    .get("name")
                    .cloned()
                    .filter(|name| !name.is_empty());
                let name = if let Some(name) = recorded {
                    name
                } else if discover::unnamed_airdrop_instance(&peer.instance) {
                    crate::airdrop::discover_receiver_name(peer.addr)
                        .await
                        .unwrap_or_else(|| "AirDrop device".into())
                } else {
                    peer.instance
                };
                println!("  {name}  {}", peer.addr);
            }
            Ok(())
        }
        Command::App => crate::appctl::run().await,
    }
}

fn note_airdrop(bar: &SendBar, peer: &str, stage: SendProgress) {
    let text = match stage {
        SendProgress::Connecting => format!("connecting to {peer}…"),
        SendProgress::SecuringConnection => format!("securing connection to {peer}…"),
        SendProgress::Discovering => format!("checking AirDrop on {peer}…"),
        SendProgress::RequestingAcceptance => format!("waiting for {peer} to accept…"),
        SendProgress::Uploading => return,
    };
    bar.note(&text);
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

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::Parser;

    #[test]
    fn receive_and_send_parse() {
        assert!(Cli::try_parse_from(["whoosh", "receive", "--yes", "--require-pin"]).is_ok());
        assert!(Cli::try_parse_from([
            "whoosh",
            "send",
            "--to",
            "127.0.0.1:9",
            "--trust-first",
            "a.jpg"
        ])
        .is_ok());
        assert!(Cli::try_parse_from(["whoosh", "discover"]).is_ok());
        assert!(Cli::try_parse_from(["whoosh", "app"]).is_ok());
    }
}
