//! A finished transfer, reported to the macOS app when a listener saved files.
//!
//! The command-line receiver keeps printing its own line. The app installs a
//! hook on the listener config so the same moment becomes a JSON event.

use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct SavedNote {
    pub peer: String,
    pub files: usize,
    pub bytes: u64,
    pub paths: Vec<String>,
}

pub type SavedHook = Arc<dyn Fn(SavedNote) + Send + Sync>;
