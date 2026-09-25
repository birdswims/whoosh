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

/// Why an iPhone share sheet shows the Mac's computer name instead of `--name`.
///
/// The row labeled with the computer name is macOS AirDrop (Bluetooth). Whoosh
/// sends its own name only in the HTTPS Discover response, after a phone has
/// already chosen a row. While macOS AirDrop is discoverable, that system row
/// is the one the phone lists.
pub fn share_sheet_conflict(whoosh_name: &str, mode: &str, computer_name: &str) -> Option<String> {
    if !system_airdrop_visible(mode) {
        return None;
    }
    let computer_name = computer_name.trim();
    if computer_name.is_empty() {
        return None;
    }
    Some(format!(
        "an iPhone lists \"{computer_name}\" from macOS AirDrop ({mode}), not \"{whoosh_name}\". That row is the system receiver and saves into Downloads. Set this Mac to Receiving Off under System Settings → General → AirDrop & Handoff, keep Whoosh running, and set the iPhone to Everyone for 10 minutes. The Whoosh row is \"{whoosh_name}\"."
    ))
}

pub fn macos_share_sheet_conflict(whoosh_name: &str) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let mode = command_line(
            "defaults",
            &["read", "com.apple.sharingd", "DiscoverableMode"],
        )?;
        let computer = command_line("scutil", &["--get", "ComputerName"])?;
        return share_sheet_conflict(whoosh_name, mode.trim(), computer.trim());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = whoosh_name;
        None
    }
}

fn system_airdrop_visible(mode: &str) -> bool {
    let mode = mode.trim();
    !mode.is_empty() && !mode.eq_ignore_ascii_case("off")
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
    use super::share_sheet_conflict;

    #[test]
    fn system_airdrop_hides_the_whoosh_name() {
        let warning = share_sheet_conflict("Harry's Mac", "Everyone", "Hariom’s MacBook Pro")
            .expect("visible mode");
        assert!(warning.contains("Hariom’s MacBook Pro"));
        assert!(warning.contains("Harry's Mac"));
        assert!(warning.contains("Receiving Off"));
        assert!(share_sheet_conflict("Harry's Mac", "Off", "Hariom’s MacBook Pro").is_none());
        assert!(share_sheet_conflict("Harry's Mac", "  off ", "Hariom’s MacBook Pro").is_none());
        assert!(share_sheet_conflict("Harry's Mac", "Contacts Only", "   ").is_none());
    }
}
