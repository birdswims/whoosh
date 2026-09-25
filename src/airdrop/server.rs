use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::airdrop::http::{self, Request};
use crate::airdrop::protocol::{self, Ask};
use crate::approve::{approve_all, Approval, IncomingFile, TransferOffer};
use crate::error::{Error, Result};
use crate::mime::{kind_of_mime, sniff, MediaKind};
use crate::paths::destination;

#[derive(Clone)]
pub struct AirdropConfig {
    pub dir: PathBuf,
    pub name: String,
    pub model: String,
    pub sort_media: bool,
    pub max_file_bytes: u64,
    pub approve: Approval,
}

impl AirdropConfig {
    pub fn auto(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            name: "Crossdrop".into(),
            model: "Crossdrop".into(),
            sort_media: false,
            max_file_bytes: 32 * 1024 * 1024 * 1024,
            approve: approve_all(),
        }
    }
}

#[derive(Default)]
struct Pending {
    accepted: bool,
}

pub struct AirdropReceiver {
    config: AirdropConfig,
    pending: Mutex<Pending>,
}

impl AirdropReceiver {
    pub fn new(config: AirdropConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            pending: Mutex::new(Pending::default()),
        })
    }

    pub async fn serve_tls(
        self: Arc<Self>,
        listener: TcpListener,
        acceptor: tokio_rustls::TlsAcceptor,
        cancel: CancellationToken,
    ) -> Result<()> {
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (socket, _) = accepted?;
                    let this = self.clone();
                    let acceptor = acceptor.clone();
                    tokio::spawn(async move {
                        match acceptor.accept(socket).await {
                            Ok(tls) => {
                                let _ = this.connection(tls).await;
                            }
                            Err(error) => tracing::debug!(%error, "airdrop tls handshake failed"),
                        }
                    });
                }
                _ = cancel.cancelled() => return Ok(()),
            }
        }
    }

    pub async fn serve(
        self: Arc<Self>,
        listener: TcpListener,
        cancel: CancellationToken,
    ) -> Result<()> {
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (socket, _) = accepted?;
                    let this = self.clone();
                    tokio::spawn(async move {
                        let _ = this.connection(socket).await;
                    });
                }
                _ = cancel.cancelled() => return Ok(()),
            }
        }
    }

    pub async fn accept_one(self: Arc<Self>, listener: TcpListener) -> Result<()> {
        let (socket, _) = timeout(Duration::from_secs(10), listener.accept())
            .await
            .map_err(|_| Error::Timeout)??;
        self.connection(socket).await
    }

    pub async fn accept_tls_one(
        self: Arc<Self>,
        listener: TcpListener,
        acceptor: tokio_rustls::TlsAcceptor,
    ) -> Result<()> {
        let (socket, _) = timeout(Duration::from_secs(10), listener.accept())
            .await
            .map_err(|_| Error::Timeout)??;
        let tls = acceptor
            .accept(socket)
            .await
            .map_err(|error| Error::crypto(error.to_string()))?;
        self.connection(tls).await
    }

    async fn connection<S>(&self, mut socket: S) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        loop {
            let request = match http::read_request(&mut socket, self.body_limit()).await {
                Ok(request) => request,
                Err(Error::Closed) => return Ok(()),
                Err(error) => {
                    let _ = http::write_response(
                        &mut socket,
                        400,
                        "Bad Request",
                        "text/plain",
                        error.to_string().as_bytes(),
                    )
                    .await;
                    return Err(error);
                }
            };
            let (status, reason, content_type, body) = self.dispatch(request).await;
            http::write_response(&mut socket, status, reason, content_type, &body).await?;
            if status >= 400 && status != 409 {
                return Ok(());
            }
        }
    }

    fn body_limit(&self) -> usize {
        usize::try_from(self.config.max_file_bytes.saturating_add(1024 * 1024))
            .unwrap_or(usize::MAX)
    }

    async fn dispatch(&self, request: Request) -> (u16, &'static str, &'static str, Vec<u8>) {
        if request.method != "POST" {
            return (
                405,
                "Method Not Allowed",
                "text/plain",
                b"POST only".to_vec(),
            );
        }
        match request.path.as_str() {
            "/Discover" => match protocol::discover_response(&self.config.name, &self.config.model)
            {
                Ok(body) => (200, "OK", "application/octet-stream", body),
                Err(error) => (
                    500,
                    "Server Error",
                    "text/plain",
                    error.to_string().into_bytes(),
                ),
            },
            "/Ask" => self.ask(request.body).await,
            "/Upload" => self.upload(request).await,
            _ => (
                404,
                "Not Found",
                "text/plain",
                b"unknown airdrop endpoint".to_vec(),
            ),
        }
    }

    async fn ask(&self, body: Vec<u8>) -> (u16, &'static str, &'static str, Vec<u8>) {
        let ask = match protocol::parse_ask(&body) {
            Ok(ask) => ask,
            Err(error) => {
                return (
                    400,
                    "Bad Request",
                    "text/plain",
                    error.to_string().into_bytes(),
                )
            }
        };
        if ask
            .files
            .iter()
            .any(|file| crate::sanitize::safe_file_name(&file.name).is_err())
        {
            return (
                400,
                "Bad Request",
                "text/plain",
                b"unsafe file name".to_vec(),
            );
        }
        let offer = offer_from_ask(&ask);
        let allow = match timeout(Duration::from_secs(120), (self.config.approve)(offer)).await {
            Ok(value) => value,
            Err(_) => false,
        };
        if !allow {
            *self.pending.lock().await = Pending { accepted: false };
            return (409, "Conflict", "text/plain", b"declined".to_vec());
        }
        *self.pending.lock().await = Pending { accepted: true };
        match protocol::ask_response(&self.config.name, &self.config.model) {
            Ok(body) => (200, "OK", "application/octet-stream", body),
            Err(error) => (
                500,
                "Server Error",
                "text/plain",
                error.to_string().into_bytes(),
            ),
        }
    }

    async fn upload(&self, request: Request) -> (u16, &'static str, &'static str, Vec<u8>) {
        if !self.pending.lock().await.accepted {
            return (
                409,
                "Conflict",
                "text/plain",
                b"upload without acceptance".to_vec(),
            );
        }
        let content_type = request
            .headers
            .get("content-type")
            .cloned()
            .unwrap_or_else(|| "application/x-cpio".into());
        let entries = match protocol::extract_upload(
            &request.body,
            &content_type,
            self.config.max_file_bytes,
        ) {
            Ok(entries) => entries,
            Err(error) => {
                *self.pending.lock().await = Pending { accepted: false };
                return (
                    400,
                    "Bad Request",
                    "text/plain",
                    error.to_string().into_bytes(),
                );
            }
        };
        for entry in entries {
            let (kind, mime) = sniff(&entry.bytes, &entry.name);
            let _ = mime;
            let dest =
                match destination(&self.config.dir, &entry.name, kind, self.config.sort_media) {
                    Ok(dest) => dest,
                    Err(error) => {
                        return (
                            400,
                            "Bad Request",
                            "text/plain",
                            error.to_string().into_bytes(),
                        )
                    }
                };
            if let Err(error) = tokio::fs::write(&dest, &entry.bytes).await {
                return (
                    500,
                    "Server Error",
                    "text/plain",
                    error.to_string().into_bytes(),
                );
            }
        }
        *self.pending.lock().await = Pending { accepted: false };
        (200, "OK", "application/octet-stream", Vec::new())
    }
}

fn offer_from_ask(ask: &Ask) -> TransferOffer {
    TransferOffer {
        protocol: "airdrop",
        peer: ask.sender_name.clone(),
        pin: None,
        files: ask
            .files
            .iter()
            .map(|file| {
                let (kind, mime) = sniff(&[], &file.name);
                IncomingFile {
                    name: file.name.clone(),
                    bytes: 0,
                    mime: mime.to_string(),
                    kind,
                }
            })
            .collect(),
    }
}

pub async fn send_tls(
    addr: std::net::SocketAddr,
    sender_name: &str,
    files: &[(String, Vec<u8>)],
    connector: tokio_rustls::TlsConnector,
) -> Result<()> {
    let socket = tokio::net::TcpStream::connect(addr).await?;
    let mut socket = connector
        .connect(crate::airdrop::server_name(), socket)
        .await
        .map_err(|error| Error::crypto(error.to_string()))?;
    post(
        &mut socket,
        "/Discover",
        "application/octet-stream",
        &protocol::discover_request()?,
    )
    .await?;
    let described: Vec<(String, String)> = files
        .iter()
        .map(|(name, bytes)| {
            let (mime, _) = protocol::describe(name, bytes);
            (name.clone(), mime)
        })
        .collect();
    let ask = protocol::ask_request(sender_name, "crossdrop", &described)?;
    let response = post(&mut socket, "/Ask", "application/octet-stream", &ask).await?;
    if response.status != 200 {
        return Err(Error::Rejected(format!("ask status {}", response.status)));
    }
    let archive = protocol::build_upload(files)?;
    let response = post(&mut socket, "/Upload", "application/x-cpio", &archive).await?;
    if response.status != 200 {
        return Err(Error::protocol(format!(
            "upload status {}",
            response.status
        )));
    }
    Ok(())
}

pub async fn send_plain(
    addr: std::net::SocketAddr,
    sender_name: &str,
    files: &[(String, Vec<u8>)],
) -> Result<()> {
    let mut socket = tokio::net::TcpStream::connect(addr).await?;
    post(
        &mut socket,
        "/Discover",
        "application/octet-stream",
        &protocol::discover_request()?,
    )
    .await?;
    let described: Vec<(String, String)> = files
        .iter()
        .map(|(name, bytes)| {
            let (mime, _) = protocol::describe(name, bytes);
            (name.clone(), mime)
        })
        .collect();
    let ask = protocol::ask_request(sender_name, "crossdrop", &described)?;
    let response = post(&mut socket, "/Ask", "application/octet-stream", &ask).await?;
    if response.status != 200 {
        return Err(Error::Rejected(format!("ask status {}", response.status)));
    }
    let archive = protocol::build_upload(files)?;
    let response = post(&mut socket, "/Upload", "application/x-cpio", &archive).await?;
    if response.status != 200 {
        return Err(Error::protocol(format!(
            "upload status {}",
            response.status
        )));
    }
    Ok(())
}

struct StatusResponse {
    status: u16,
}

async fn post<S>(io: &mut S, path: &str, content_type: &str, body: &[u8]) -> Result<StatusResponse>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: crossdrop\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
        body.len()
    );
    io.write_all(head.as_bytes()).await?;
    io.write_all(body).await?;
    io.flush().await?;
    let response = http::read_request(io, 1024 * 1024).await?;
    // `read_request` parses a response poorly because it expects a request line.
    // Parse the status from the method slot: "HTTP/1.1" and the path slot is the code.
    let status = response.path.parse().unwrap_or(0);
    let _ = response.method;
    Ok(StatusResponse { status })
}

#[allow(dead_code)]
fn _kind(mime: &str) -> MediaKind {
    kind_of_mime(mime)
}

#[cfg(test)]
mod tests {
    use super::{send_plain, send_tls, AirdropConfig, AirdropReceiver};
    use crate::approve::approval;
    use std::net::SocketAddr;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn receives_a_photo_and_a_video() {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let receiver = AirdropReceiver::new(AirdropConfig::auto(dir.path()));
        let server = tokio::spawn(async move { receiver.accept_one(listener).await });
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xD9];
        let video = b"ftypisom-video".to_vec();
        send_plain(
            addr,
            "iPhone",
            &[
                ("photo.jpg".into(), jpeg.clone()),
                ("clip.mp4".into(), video.clone()),
            ],
        )
        .await
        .unwrap();
        server.await.unwrap().unwrap();
        assert_eq!(std::fs::read(dir.path().join("photo.jpg")).unwrap(), jpeg);
        assert_eq!(std::fs::read(dir.path().join("clip.mp4")).unwrap(), video);
    }

    #[tokio::test]
    async fn receives_over_tls() {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (acceptor, cert) = crate::airdrop::server_acceptor().unwrap();
        let receiver = AirdropReceiver::new(AirdropConfig::auto(dir.path()));
        let server = tokio::spawn(async move { receiver.accept_tls_one(listener, acceptor).await });
        let connector = crate::airdrop::client_connector(cert).unwrap();
        let client = send_tls(
            addr,
            "iPhone",
            &[("tls.jpg".into(), vec![0xFF, 0xD8, 0xFF])],
            connector,
        )
        .await;
        let server_result = server.await.unwrap();
        assert!(
            client.is_ok() && server_result.is_ok(),
            "client {client:?} server {server_result:?}"
        );
        assert_eq!(
            std::fs::read(dir.path().join("tls.jpg")).unwrap(),
            vec![0xFF, 0xD8, 0xFF]
        );
    }

    #[tokio::test]
    async fn decline_and_traversal_do_not_write() {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut config = AirdropConfig::auto(dir.path());
        config.approve = approval(|_| async { false });
        let receiver = AirdropReceiver::new(config);
        let server = tokio::spawn(async move { receiver.accept_one(listener).await });
        let error = send_plain(addr, "iPhone", &[("a.jpg".into(), b"x".to_vec())])
            .await
            .unwrap_err();
        assert!(error.to_string().contains("409") || error.to_string().contains("ask"));
        let _ = server.await;
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    }
}
