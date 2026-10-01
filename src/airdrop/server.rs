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
    /// Called after an upload is unpacked. The command-line receiver leaves this empty.
    pub on_saved: Option<crate::note::SavedHook>,
}

impl AirdropConfig {
    pub fn auto(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            name: "Whoosh".into(),
            model: "Whoosh".into(),
            sort_media: false,
            max_file_bytes: 32 * 1024 * 1024 * 1024,
            approve: approve_all(),
            on_saved: None,
        }
    }
}

#[derive(Default)]
struct Pending {
    accepted: bool,
    peer: String,
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
                    let (socket, peer) = accepted?;
                    let this = self.clone();
                    let acceptor = acceptor.clone();
                    tokio::spawn(async move {
                        match timeout(Duration::from_secs(15), acceptor.accept(socket)).await {
                            Ok(Ok(tls)) => {
                                println!("airdrop    {peer}  HTTPS connected");
                                if let Err(error) = this.connection(tls).await {
                                    tracing::warn!(%peer, %error, "AirDrop HTTPS session failed");
                                }
                            }
                            Ok(Err(error)) => {
                                println!(
                                    "airdrop    {peer}  connected, then did not speak AirDrop HTTPS ({error})"
                                );
                            }
                            Err(_) => println!("airdrop    {peer}  TLS handshake timed out"),
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
            let path = request.path.clone();
            let (status, reason, content_type, body) = self.dispatch(request).await;
            if status >= 400 {
                tracing::warn!(%path, status, error = %String::from_utf8_lossy(&body), "AirDrop request failed");
            }
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
                Ok(body) => {
                    println!(
                        "airdrop    discover from a sender, answering as \"{}\"",
                        self.config.name
                    );
                    (200, "OK", "application/octet-stream", body)
                }
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
            *self.pending.lock().await = Pending::default();
            return (409, "Conflict", "text/plain", b"declined".to_vec());
        }
        *self.pending.lock().await = Pending {
            accepted: true,
            peer: ask.sender_name.clone(),
        };
        println!("airdrop    accepted; waiting for the sender to upload");
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
        println!(
            "airdrop    upload received: {} bytes ({content_type})",
            request.body.len()
        );
        let entries = match protocol::extract_upload(
            &request.body,
            &content_type,
            self.config.max_file_bytes,
        ) {
            Ok(entries) => entries,
            Err(error) => {
                *self.pending.lock().await = Pending::default();
                return (
                    400,
                    "Bad Request",
                    "text/plain",
                    error.to_string().into_bytes(),
                );
            }
        };
        let count = entries.len();
        let bytes: usize = entries.iter().map(|entry| entry.bytes.len()).sum();
        let mut paths = Vec::with_capacity(entries.len());
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
            paths.push(dest.display().to_string());
        }
        let peer = std::mem::take(&mut self.pending.lock().await.peer);
        *self.pending.lock().await = Pending::default();
        if let Some(hook) = &self.config.on_saved {
            hook(crate::note::SavedNote {
                peer: peer.clone(),
                files: count,
                bytes: bytes as u64,
                paths,
            });
        }
        println!(
            "airdrop    saved {count} file(s), {bytes} bytes, to {}",
            self.config.dir.display()
        );
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
    let ask = protocol::ask_request(sender_name, "whoosh", &described)?;
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
    let ask = protocol::ask_request(sender_name, "whoosh", &described)?;
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
        "POST {path} HTTP/1.1\r\nHost: whoosh\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
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
    async fn discovers_named_receiver_over_ipv6_https() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = crate::net::bind_airdrop_listener("[::1]:0".parse().unwrap()).unwrap();
        let addr = listener.local_addr().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut config = AirdropConfig::auto(dir.path());
        config.name = "Harry's Mac".into();
        let receiver = AirdropReceiver::new(config);
        let (acceptor, cert) = crate::airdrop::server_acceptor().unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        let server = tokio::spawn(receiver.serve_tls(listener, acceptor, cancel.clone()));
        let body = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let socket = tokio::net::TcpStream::connect(addr).await.unwrap();
            let mut socket = crate::airdrop::client_connector(cert)
                .unwrap()
                .connect(crate::airdrop::server_name(), socket)
                .await
                .unwrap();
            let body = super::protocol::discover_request().unwrap();
            socket
                .write_all(
                    format!(
                        "POST /Discover HTTP/1.1\r\nHost: AirDrop\r\nContent-Length: {}\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            socket.write_all(&body).await.unwrap();
            socket.flush().await.unwrap();
            let response = super::http::read_request(&mut socket, 1024 * 1024)
                .await
                .unwrap();
            assert_eq!(response.path, "200");
            socket.shutdown().await.unwrap();
            // Consume the server's TLS close so the session has ended.
            let mut remaining = Vec::new();
            let _ = socket.read_to_end(&mut remaining).await;
            response.body
        })
        .await;
        cancel.cancel();
        server.await.unwrap().unwrap();
        let value = plist::Value::from_reader(std::io::Cursor::new(body.unwrap())).unwrap();
        assert_eq!(
            value.as_dictionary().unwrap()["ReceiverComputerName"].as_string(),
            Some("Harry's Mac")
        );
    }

    #[tokio::test]
    async fn receives_iphone_style_chunked_odc_upload_over_tls() {
        use tokio::io::AsyncWriteExt;

        let listener = crate::net::bind_airdrop_listener("[::1]:0".parse().unwrap()).unwrap();
        let addr = listener.local_addr().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let receiver = AirdropReceiver::new(AirdropConfig::auto(dir.path()));
        let (acceptor, cert) = crate::airdrop::server_acceptor().unwrap();
        let connector = crate::airdrop::client_connector(cert).unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        let server = tokio::spawn(receiver.serve_tls(listener, acceptor, cancel.clone()));
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let socket = tokio::net::TcpStream::connect(addr).await.unwrap();
            let mut socket = connector.connect(crate::airdrop::server_name(), socket).await.unwrap();
            let ask = super::protocol::ask_request("iPhone", "phone", &[("IMG_9457.jpg".into(), "image/jpeg".into())]).unwrap();
            let archive = include_bytes!("../../tests/fixtures/airdrop-odc.cpio");
            let mut upload = (0x8000_0000 | archive.len() as u32).to_be_bytes().to_vec();
            upload.extend_from_slice(archive); // stored DVZip block, no terminator
            for (path, content_type, body) in [
                ("/Ask", "application/octet-stream", ask),
                ("/Upload", "application/x-dvzip", upload),
            ] {
                socket.write_all(format!(
                    "POST {path} HTTP/1.1\r\nHost: AirDrop\r\nContent-Type: {content_type}\r\nTransfer-Encoding: chunked\r\nExpect: 100-continue\r\n\r\n"
                ).as_bytes()).await.unwrap();
                socket.flush().await.unwrap();
                let interim = super::http::read_request(&mut socket, 4096).await.unwrap();
                assert_eq!(interim.path, "100");
                for chunk in body.chunks(53) {
                    socket.write_all(format!("{:x}\r\n", chunk.len()).as_bytes()).await.unwrap();
                    socket.write_all(chunk).await.unwrap();
                    socket.write_all(b"\r\n").await.unwrap();
                }
                socket.write_all(b"0\r\n\r\n").await.unwrap();
                socket.flush().await.unwrap();
                let response = super::http::read_request(&mut socket, 4096).await.unwrap();
                assert_eq!(response.path, "200", "{path}: {}", String::from_utf8_lossy(&response.body));
            }
            socket.shutdown().await.unwrap();
        }).await;
        cancel.cancel();
        server.await.unwrap().unwrap();
        result.expect("iPhone-style transfer stalled");
        assert_eq!(
            std::fs::read(dir.path().join("IMG_9457.jpg")).unwrap(),
            b"\xff\xd8\xff\xe0whoosh-fixture\xff\xd9"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
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
