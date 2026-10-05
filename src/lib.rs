//! Whoosh moves files, photos, and videos between macOS, Windows, and Linux.
//!
//! The native protocol is QUIC. Android Quick Share and Apple AirDrop are
//! separate sessions that speak those wire protocols.
#![deny(unsafe_code)]

pub mod airdrop;
#[allow(unsafe_code)]
mod appctl;
pub mod approve;
#[allow(unsafe_code)]
mod children;
pub mod cli;
pub mod discover;
pub mod error;
pub mod mime;
pub mod native;
pub mod net;
mod note;
pub mod paths;
mod progress;
pub mod quickshare;
#[allow(unsafe_code)]
pub mod radio;
pub mod sanitize;
pub mod service;
mod trust;

pub use error::{Error, Result};
