//! QUIC transfer between Whoosh peers.
//!
//! This is the fast path. Photos and videos are streamed in 1 MiB reads over
//! parallel QUIC streams with BBR and multi-megabyte flow-control windows.
//! BLAKE3 is computed while the bytes move, so a large video is not read twice
//! and is never buffered whole.

mod codec;
mod session;
mod tls;

pub use session::{send_files, NativeListener, ReceiveOptions, ReceiveReport, SendReport};
pub use tls::{fingerprint_hex, generate_identity, install_crypto, Trust};

pub const SERVICE_TYPE: &str = "_whoosh._udp.local.";
