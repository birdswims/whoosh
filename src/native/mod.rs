//! QUIC transfer between Whoosh peers.
//!
//! This is the fast path. Photos and videos are streamed in 1 MiB reads over
//! parallel QUIC streams with BBR and multi-megabyte flow-control windows.
//! BLAKE3 is computed while the bytes move, so a large video is not read twice
//! and is never buffered whole.

mod codec;
mod session;
mod tls;

pub use session::{
    pull_clipboard, send_files, send_files_with_progress, ClipboardItem, ClipboardStore,
    NativeListener, ReceiveOptions, ReceiveReport, SendReport, SessionResult,
};
pub use tls::{
    client_config_with_identity, fingerprint_hex, generate_identity, identity_from_der,
    install_crypto, Trust,
};

pub const SERVICE_TYPE: &str = "_whoosh._udp.local.";
