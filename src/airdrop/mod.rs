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

/// Second AirDrop browse type compiled into Sharing.framework
/// (`_kBonjourTypeAirDropAlt`). The system receiver itself is advertised as
/// `_companion-link._tcp`, not as `_airdrop._tcp`.
pub const ALT_SERVICE_TYPE: &str = "_airdrop-alt._tcp.local.";

/// The service an iPhone AirDrop sheet lists. The instance name is the label
/// on that sheet. macOS publishes the computer name here. Whoosh publishes
/// `--name` as a second record, on its own hostname, and does not rename the Mac.
pub const COMPANION_SERVICE_TYPE: &str = "_companion-link._tcp.local.";

/// mDNS TXT `flags`. Apple ignores a receiver that sets neither mixed-types
/// (`0x08`) nor pipelining (`0x04`). `0x80` marks support for `/Discover`.
/// `140` is `0x8C`: discover, mixed types, and pipelining.
pub const MDNS_FLAGS: &str = "140";

/// What to print about the phone share sheets.
pub fn airdrop_visibility_line(whoosh_name: &str, computer_name: &str) -> String {
    let computer_name = if computer_name.trim().is_empty() {
        "this Mac"
    } else {
        computer_name.trim()
    };
    format!(
        "look for a separate row \"{whoosh_name}\". \"{computer_name}\" stays the macOS AirDrop name and saves into Downloads. AirDrop on the iPhone must be Everyone for 10 minutes. Quick Share must be open and set to Everyone on this Wi-Fi."
    )
}

/// TXT keys copied from a live macOS AirDrop companion-link record.
/// `rpFl=0x20000` is the flag this Mac advertises while AirDrop is on.
pub fn companion_txt() -> Vec<(String, String)> {
    let mut bytes = [0u8; 30];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut bytes);
    let mac = format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        bytes[24], bytes[25], bytes[26], bytes[27], bytes[28], bytes[29]
    );
    vec![
        ("rpMac".into(), "0".into()),
        ("rpHN".into(), hex::encode(&bytes[0..6])),
        ("rpFl".into(), "0x20000".into()),
        ("rpHA".into(), hex::encode(&bytes[6..12])),
        ("rpVr".into(), "715.2".into()),
        ("rpAD".into(), hex::encode(&bytes[12..18])),
        ("rpHI".into(), hex::encode(&bytes[18..24])),
        ("rpBA".into(), mac),
    ]
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
    fn iphone_keeps_the_mac_name_and_lists_whoosh() {
        let line = airdrop_visibility_line("Harry's Mac", "Hariom’s MacBook Pro");
        assert!(line.contains("separate row \"Harry's Mac\""));
        assert!(line.contains("Hariom’s MacBook Pro"));
        assert!(line.contains("stays the macOS AirDrop name"));
        assert!(line.contains("Downloads"));
        assert!(line.contains("Everyone"));
    }

    #[test]
    fn companion_record_uses_the_live_rapport_keys() {
        let txt = super::companion_txt();
        let keys: Vec<&str> = txt.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(
            keys,
            ["rpMac", "rpHN", "rpFl", "rpHA", "rpVr", "rpAD", "rpHI", "rpBA"]
        );
        assert_eq!(value(&txt, "rpFl"), "0x20000");
        assert_eq!(value(&txt, "rpVr"), "715.2");
        assert_eq!(value(&txt, "rpMac"), "0");
        let mac = value(&txt, "rpBA");
        assert_eq!(mac.len(), 17);
        assert_eq!(mac.chars().filter(|ch| *ch == ':').count(), 5);
    }

    fn value(txt: &[(String, String)], key: &str) -> String {
        txt.iter()
            .find(|(name, _)| name == key)
            .map(|(_, item)| item.clone())
            .unwrap_or_default()
    }
}
