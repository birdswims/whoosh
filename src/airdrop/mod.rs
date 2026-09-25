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
