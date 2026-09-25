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

/// What to print about the iPhone share sheet.
///
/// `switched_from` is set when Whoosh had to turn macOS AirDrop to Everyone
/// because Contacts Only and Receiving Off never answer a phone that is not
/// already a contact, so the sheet stays empty.
pub fn airdrop_visibility_line(
    whoosh_name: &str,
    computer_name: &str,
    switched_from: Option<&str>,
) -> String {
    let computer_name = if computer_name.trim().is_empty() {
        "this Mac"
    } else {
        computer_name.trim()
    };
    match switched_from.map(str::trim).filter(|mode| !mode.is_empty()) {
        Some(mode) => format!(
            "macOS AirDrop was {mode}, so the iPhone showed no devices. It stays Everyone until you stop Whoosh, then {mode} is restored. On the iPhone set AirDrop to Everyone for 10 minutes. The system row is \"{computer_name}\" and saves into Downloads. Choose \"{whoosh_name}\" to save into this folder."
        ),
        None => format!(
            "on the iPhone set AirDrop to Everyone for 10 minutes. The system row is \"{computer_name}\" and saves into Downloads. Choose \"{whoosh_name}\" to save into this folder."
        ),
    }
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

/// Turns macOS AirDrop to Everyone for the life of this value, then restores
/// the previous mode. Contacts Only and Receiving Off do not answer an iPhone
/// that is not already a contact, so the share sheet stays empty.
pub struct RestoredAirDropMode {
    previous: String,
}

impl RestoredAirDropMode {
    pub fn previous(&self) -> &str {
        &self.previous
    }

    pub fn everyone_for_this_process() -> Option<Self> {
        #[cfg(target_os = "macos")]
        {
            let mode = command_line(
                "defaults",
                &["read", "com.apple.sharingd", "DiscoverableMode"],
            )?;
            let mode = mode.trim();
            if mode.eq_ignore_ascii_case("everyone") {
                return None;
            }
            let wrote = std::process::Command::new("defaults")
                .args([
                    "write",
                    "com.apple.sharingd",
                    "DiscoverableMode",
                    "-string",
                    "Everyone",
                ])
                .status()
                .map(|status| status.success())
                .unwrap_or(false);
            if !wrote {
                return None;
            }
            // sharingd keeps the old mode in memory until it starts again.
            let _ = std::process::Command::new("killall")
                .arg("sharingd")
                .status();
            return Some(Self {
                previous: mode.to_string(),
            });
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }
}

impl Drop for RestoredAirDropMode {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("defaults")
                .args([
                    "write",
                    "com.apple.sharingd",
                    "DiscoverableMode",
                    "-string",
                    &self.previous,
                ])
                .status();
            let _ = std::process::Command::new("killall")
                .arg("sharingd")
                .status();
        }
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
    fn contacts_only_explains_the_empty_sheet() {
        let line =
            airdrop_visibility_line("Harry's Mac", "Hariom’s MacBook Pro", Some("Contacts Only"));
        assert!(line.contains("showed no devices"));
        assert!(line.contains("Contacts Only"));
        assert!(line.contains("Harry's Mac"));
        assert!(line.contains("Hariom’s MacBook Pro"));
        assert!(!line.contains("Receiving Off"));
        let already = airdrop_visibility_line("Harry's Mac", "Hariom’s MacBook Pro", None);
        assert!(already.contains("Downloads"));
        assert!(!already.contains("showed no devices"));
    }
}
