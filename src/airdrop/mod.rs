//! Apple AirDrop receive and send.
//!
//! The data path is the public Discover, Ask, and Upload HTTPS contract.
//! Contacts-only transfers need an Apple-signed identity, which this program
//! does not ship. Everyone mode on the same network can reach the receiver.
//! On macOS the daemon also binds `awdl0` when that interface is up, which is
//! what an iPhone uses for AirDrop.

mod cpio;
mod http;
mod protocol;
mod server;
mod tls;

pub use tls::{client_connector, interoperable_connector, server_acceptor, server_name};

pub use server::{send_plain, send_tls, AirdropConfig, AirdropReceiver};

pub const SERVICE_TYPE: &str = "_airdrop._tcp.local.";

/// mDNS TXT `flags`. Apple ignores a receiver that sets neither mixed-types
/// (`0x08`) nor pipelining (`0x04`). `0x80` marks support for `/Discover`.
/// `136` is `0x88`, the minimum OpenDrop found macOS will keep.
pub const MDNS_FLAGS: &str = "136";

/// What to print about the phone share sheets.
pub fn airdrop_visibility_line(whoosh_name: &str, computer_name: &str) -> String {
    let computer_name = if computer_name.trim().is_empty() {
        "this Mac"
    } else {
        computer_name.trim()
    };
    format!(
        "look for \"{whoosh_name}\" on the phone. Quick Share must be open and set to Everyone on this Wi-Fi. AirDrop on the iPhone must be Everyone for 10 minutes. \"{computer_name}\" is macOS AirDrop and saves into Downloads."
    )
}

pub fn macos_computer_name() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        return command_line("scutil", &["--get", "ComputerName"]);
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(target_os = "macos")]
fn command_line(program: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::airdrop_visibility_line;

    #[test]
    fn iphone_sheet_is_companion_link_not_whoosh() {
        let line = airdrop_visibility_line("Harry's Mac", "Hariom’s MacBook Pro");
        assert!(line.contains("Harry's Mac"));
        assert!(line.contains("Hariom’s MacBook Pro"));
        assert!(line.contains("Downloads"));
        assert!(line.contains("Everyone"));
    }
}
