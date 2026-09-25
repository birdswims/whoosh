use std::io;

/// Recoverable whoosh failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Protocol(String),
    #[error("transfer rejected: {0}")]
    Rejected(String),
    #[error("unsafe file name")]
    UnsafeName,
    #[error("file exceeds the size limit")]
    TooLarge,
    #[error("peer closed the connection")]
    Closed,
    #[error("timed out")]
    Timeout,
    #[error("wrong or missing pin")]
    Pin,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("hash mismatch for {0}")]
    Hash(String),
    #[error("untrusted peer certificate {0}")]
    Untrusted(String),
    #[error("cryptography failure: {0}")]
    Crypto(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn protocol(message: impl Into<String>) -> Self {
        Self::Protocol(message.into())
    }

    pub fn crypto(message: impl Into<String>) -> Self {
        Self::Crypto(message.into())
    }
}

pub(crate) async fn read_exact<R>(reader: &mut R, buf: &mut [u8]) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    match reader.read_exact(buf).await {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Err(Error::Closed),
        Err(error) => Err(error.into()),
    }
}
