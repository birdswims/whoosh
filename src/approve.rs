use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::mime::MediaKind;

/// One file a peer wants to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingFile {
    pub name: String,
    pub bytes: u64,
    pub mime: String,
    pub kind: MediaKind,
}

/// A transfer waiting for a local yes or no.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferOffer {
    pub protocol: &'static str,
    pub peer: String,
    pub pin: Option<String>,
    pub files: Vec<IncomingFile>,
}

impl TransferOffer {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.bytes).sum()
    }
}

pub type Approval =
    Arc<dyn Fn(TransferOffer) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;

pub fn approval<F, Fut>(decide: F) -> Approval
where
    F: Fn(TransferOffer) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = bool> + Send + 'static,
{
    Arc::new(move |offer| Box::pin(decide(offer)))
}

pub fn approve_all() -> Approval {
    approval(|_| async { true })
}

pub fn reject_all() -> Approval {
    approval(|_| async { false })
}
