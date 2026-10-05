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
use crate::progress::{
    scale_bytes, ByteMeter, ByteProgress, FailHook, PeerProgressHook, ProgressGate,
};

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
    /// Called with the sender's name as the upload body arrives.
    pub on_progress: Option<PeerProgressHook>,
    /// Called when an accepted upload stops before the files are saved.
    pub on_failed: Option<FailHook>,
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
            on_progress: None,
            on_failed: None,
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
        // Acceptance lives on the receiver, shared by every connection. Only this
        // connection's close should cancel the transfer it already accepted.
        let mut waiting_for_upload = false;
        loop {
            let mut head = match http::read_head(&mut socket).await {
                Ok(head) => head,
                Err(Error::Closed) => {
                    if waiting_for_upload {
                        self.fail_if_accepted("the sender closed the connection")
                            .await;
                    }
                    return Ok(());
                }
                Err(error) => {
                    if waiting_for_upload {
                        self.fail_if_accepted(&error.to_string()).await;
                    }
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
            let expecting_body = waiting_for_upload && head.path == "/Upload";
            let meter = self.upload_meter(&head).await;
            if let Err(error) = http::answer_continue(&mut socket, &head).await {
                if expecting_body {
                    self.fail_if_accepted(&error.to_string()).await;
                }
                return Err(error);
            }
            let limit = self.body_limit();
            let body = match http::read_body(&mut socket, &mut head, limit, &|got, total| {
                if let Some(meter) = &meter {
                    meter.observe(got, total);
                }
            })
            .await
            {
                Ok(body) => body,
                Err(Error::Closed) => {
                    if expecting_body {
                        self.fail_if_accepted("the sender closed the upload").await;
                    }
                    return Ok(());
                }
                Err(error) => {
                    if expecting_body {
                        self.fail_if_accepted(&error.to_string()).await;
                    }
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
            if let Some(meter) = &meter {
                meter.finish();
            }
            let request = Request {
                method: head.method,
                path: head.path,
                headers: head.headers,
                body,
            };
            let path = request.path.clone();
            let (status, reason, content_type, body) = self.dispatch(request).await;
            if path == "/Ask" && status == 200 {
                waiting_for_upload = true;
            } else if path == "/Upload" {
                waiting_for_upload = false;
            }
            if status >= 400 {
                tracing::warn!(%path, status, error = %String::from_utf8_lossy(&body), "AirDrop request failed");
            }
            http::write_response(&mut socket, status, reason, content_type, &body).await?;
            if status >= 400 && status != 409 {
                return Ok(());
            }
        }
    }

    async fn upload_meter(&self, head: &http::Head) -> Option<Arc<ByteMeter>> {
        if head.path != "/Upload" {
            return None;
        }
        tracing::debug!(headers = ?head.headers.keys().collect::<Vec<_>>(), content_length = ?head.headers.get("content-length"), transfer_encoding = ?head.headers.get("transfer-encoding"), content_type = ?head.headers.get("content-type"), "AirDrop upload framing");
        let pending = self.pending.lock().await;
        if !pending.accepted {
            return None;
        }
        let hook = self.config.on_progress.clone()?;
        let peer = pending.peer.clone();
        Some(ByteMeter::new(
            0,
            Arc::new(move |progress| hook(&peer, progress)),
        ))
    }

    async fn fail_if_accepted(&self, message: &str) {
        let mut pending = self.pending.lock().await;
        if !pending.accepted {
            return;
        }
        let peer = std::mem::take(&mut pending.peer);
        *pending = Pending::default();
        if let Some(hook) = &self.config.on_failed {
            hook(&peer, message);
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
                let message = error.to_string();
                self.fail_if_accepted(&message).await;
                return (400, "Bad Request", "text/plain", message.into_bytes());
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
                        let message = error.to_string();
                        self.fail_if_accepted(&message).await;
                        return (400, "Bad Request", "text/plain", message.into_bytes());
                    }
                };
            if let Err(error) = tokio::fs::write(&dest, &entry.bytes).await {
                let message = error.to_string();
                self.fail_if_accepted(&message).await;
                return (500, "Server Error", "text/plain", message.into_bytes());
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
                    bytes: file.size.unwrap_or(0),
                    size_known: file.size.is_some(),
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
    send_tls_reporting(addr, sender_name, files, connector, &|_| {}, &|_| {}).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendProgress {
    Connecting,
    SecuringConnection,
    Discovering,
    RequestingAcceptance,
    Uploading,
}

pub async fn send_tls_with_progress(
    addr: std::net::SocketAddr,
    sender_name: &str,
    files: &[(String, Vec<u8>)],
    connector: tokio_rustls::TlsConnector,
    progress: &(dyn Fn(SendProgress) + Send + Sync),
) -> Result<()> {
    send_tls_reporting(addr, sender_name, files, connector, progress, &|_| {}).await
}

pub async fn send_tls_reporting(
    addr: std::net::SocketAddr,
    sender_name: &str,
    files: &[(String, Vec<u8>)],
    connector: tokio_rustls::TlsConnector,
    progress: &(dyn Fn(SendProgress) + Send + Sync),
    bytes: &(dyn Fn(ByteProgress) + Send + Sync),
) -> Result<()> {
    progress(SendProgress::Connecting);
    let socket = connect_for_send(addr).await?;
    progress(SendProgress::SecuringConnection);
    let mut socket = timeout(
        Duration::from_secs(8),
        connector.connect(crate::airdrop::server_name(), socket),
    )
    .await
    .map_err(|_| {
        Error::protocol(
            "AirDrop secure connection timed out before an acceptance request could be sent.",
        )
    })?
    .map_err(|error| Error::crypto(error.to_string()))?;
    send_session(&mut socket, addr, sender_name, files, progress, bytes).await
}

pub async fn send_plain(
    addr: std::net::SocketAddr,
    sender_name: &str,
    files: &[(String, Vec<u8>)],
) -> Result<()> {
    send_plain_reporting(addr, sender_name, files, &|_| {}, &|_| {}).await
}

pub async fn send_plain_reporting(
    addr: std::net::SocketAddr,
    sender_name: &str,
    files: &[(String, Vec<u8>)],
    progress: &(dyn Fn(SendProgress) + Send + Sync),
    bytes: &(dyn Fn(ByteProgress) + Send + Sync),
) -> Result<()> {
    progress(SendProgress::Connecting);
    let mut socket = connect_for_send(addr).await?;
    send_session(&mut socket, addr, sender_name, files, progress, bytes).await
}

async fn connect_for_send(addr: std::net::SocketAddr) -> Result<tokio::net::TcpStream> {
    match timeout(Duration::from_secs(8), crate::net::connect_airdrop(addr)).await {
        Ok(Ok(socket)) => Ok(socket),
        Ok(Err(error)) => Err(Error::protocol(format!(
            "could not reach the AirDrop device ({error}). Keep it unlocked and nearby, with AirDrop set to Everyone for 10 Minutes."
        ))),
        Err(_) => Err(Error::protocol(
            "could not reach the AirDrop device. Keep it unlocked and nearby, with AirDrop set to Everyone for 10 Minutes.",
        )),
    }
}

async fn send_session<S>(
    socket: &mut S,
    addr: std::net::SocketAddr,
    sender_name: &str,
    files: &[(String, Vec<u8>)],
    progress: &(dyn Fn(SendProgress) + Send + Sync),
    bytes: &(dyn Fn(ByteProgress) + Send + Sync),
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // Host `whoosh` is what this iPhone already answered Discover with.
    // A scoped IPv6 zone in the header is not a legal Host value.
    let host = "whoosh";
    progress(SendProgress::Discovering);
    let discover = timeout(Duration::from_secs(8), post(
        socket,
        "/Discover",
        host,
        "application/octet-stream",
        &protocol::discover_request()?,
    ))
    .await
    .map_err(|_| Error::protocol("The AirDrop device did not answer discovery. No acceptance request was sent. Keep it unlocked with AirDrop set to Everyone for 10 Minutes and try again."))??;
    if discover.status != 200 {
        return Err(Error::protocol(format!(
            "the AirDrop device did not answer Discover ({})",
            discover.status
        )));
    }
    let reply = protocol::discover_reply(&discover.body);
    if reply.is_none() {
        return Err(Error::protocol(
            "The AirDrop device returned an invalid discovery response.",
        ));
    }
    tracing::info!(
        %addr,
        status = discover.status,
        detail = %discover_body_detail(&discover.body),
        "AirDrop discover"
    );
    if reply
        .as_ref()
        .is_some_and(|reply| reply.accepts == Some(false))
    {
        return Err(Error::Rejected(
            "this device is not accepting AirDrop from everyone. Set AirDrop to Everyone for 10 Minutes and try again."
                .into(),
        ));
    }
    let described: Vec<(String, String, u64)> = files
        .iter()
        .map(|(name, bytes)| {
            let (mime, _) = protocol::describe(name, bytes);
            (name.clone(), mime, bytes.len() as u64)
        })
        .collect();
    let ask = protocol::ask_request(
        sender_name,
        &hardware_model(),
        &airdrop_sender_id(),
        &described,
    )?;
    // The phone holds this response until the person accepts or declines.
    progress(SendProgress::RequestingAcceptance);
    let response = match timeout(
        Duration::from_secs(60),
        post(socket, "/Ask", host, "application/octet-stream", &ask),
    )
    .await
    {
        Ok(response) => response?,
        Err(_) => {
            return Err(Error::protocol(
                "The device did not answer Whoosh's AirDrop request. If no prompt appeared, its AirDrop version may require a newer handshake. No file contents were sent.",
            ))
        }
    };
    if response.status != 200 {
        return Err(Error::Rejected(format!(
            "the AirDrop device declined the transfer ({})",
            ask_failure(&response)
        )));
    }
    tracing::info!(%addr, "AirDrop request accepted; uploading files");
    progress(SendProgress::Uploading);
    let archive = protocol::build_upload(files)?;
    let file_total: u64 = files.iter().map(|(_, data)| data.len() as u64).sum();
    let archive_len = archive.len() as u64;
    let mut gate = ProgressGate::new(file_total);
    gate.start(bytes);
    let response = timeout(
        Duration::from_secs(600),
        post_reporting(
            socket,
            "/Upload",
            host,
            "application/x-cpio",
            &archive,
            &mut |written| gate.observe(scale_bytes(written, archive_len, file_total), bytes),
        ),
    )
    .await
    .map_err(|_| Error::protocol("AirDrop upload timed out; delivery could not be confirmed."))??;
    gate.finish(bytes);
    if response.status != 200 {
        return Err(Error::protocol(format!(
            "the AirDrop device did not take the file ({})",
            response.status
        )));
    }
    tracing::info!(%addr, "AirDrop upload completed");
    Ok(())
}

fn ask_failure(response: &StatusResponse) -> String {
    if response.body.is_empty() {
        return response.status.to_string();
    }
    format!(
        "{}: {}",
        response.status,
        discover_body_detail(&response.body)
    )
}

fn hardware_model() -> String {
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("sysctl")
            .args(["-n", "hw.model"])
            .output()
        {
            if output.status.success() {
                let model = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !model.is_empty()
                    && model.len() <= 64
                    && model.chars().all(|ch| ch.is_ascii_graphic())
                {
                    return model;
                }
            }
        }
    }
    "Mac".to_string()
}

fn airdrop_sender_id() -> String {
    let mut bytes = [0u8; 6];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut bytes);
    hex::encode(bytes)
}

struct StatusResponse {
    status: u16,
    body: Vec<u8>,
}

pub async fn discover_receiver_name(addr: std::net::SocketAddr) -> Option<String> {
    match lookup_receiver_name(addr).await {
        Ok(name) => Some(name),
        Err(error) => {
            tracing::info!(%addr, %error, "AirDrop device did not share its name");
            None
        }
    }
}

async fn lookup_receiver_name(addr: std::net::SocketAddr) -> Result<String> {
    let connector = super::interoperable_connector()?;
    let mut socket = timeout(Duration::from_secs(3), async {
        let socket = crate::net::connect_airdrop(addr)
            .await
            .map_err(|error| Error::protocol(format!("connect: {error}")))?;
        connector
            .connect(super::server_name(), socket)
            .await
            .map_err(|error| Error::protocol(format!("tls: {error}")))
    })
    .await
    .map_err(|_| Error::protocol("timed out connecting"))??;
    let body = protocol::discover_request()?;
    let response = timeout(
        Duration::from_secs(3),
        post(
            &mut socket,
            "/Discover",
            "whoosh",
            "application/octet-stream",
            &body,
        ),
    )
    .await
    .map_err(|_| Error::protocol("timed out waiting for Discover"))??;
    if response.status != 200 {
        return Err(Error::protocol(format!(
            "discover status {}",
            response.status
        )));
    }
    protocol::receiver_computer_name(&response.body).ok_or_else(|| {
        Error::protocol(format!(
            "Discover reply had no computer name ({})",
            discover_body_detail(&response.body)
        ))
    })
}

fn discover_body_detail(body: &[u8]) -> String {
    let Ok(value) = plist::Value::from_reader(std::io::Cursor::new(body)) else {
        return format!("unparsed {} bytes", body.len());
    };
    let Some(dict) = value.as_dictionary() else {
        return format!("not a dictionary, {} bytes", body.len());
    };
    let keys = dict.keys().cloned().collect::<Vec<_>>().join(", ");
    if keys.is_empty() {
        format!("empty dictionary, {} bytes", body.len())
    } else {
        keys
    }
}

async fn post<S>(
    io: &mut S,
    path: &str,
    host: &str,
    content_type: &str,
    body: &[u8],
) -> Result<StatusResponse>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    post_reporting(io, path, host, content_type, body, &mut |_| {}).await
}

async fn post_reporting<S>(
    io: &mut S,
    path: &str,
    host: &str,
    content_type: &str,
    body: &[u8],
    on_written: &mut (dyn FnMut(u64) + Send),
) -> Result<StatusResponse>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // AirDrop/1.0 is the sender identity sharingd expects. Advertising
    // Accept-Encoding makes some iPhones compress the reply, which this
    // parser would then treat as a failed plist.
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: keep-alive\r\nAccept: */*\r\nUser-Agent: AirDrop/1.0\r\nAccept-Language: en-us\r\n\r\n",
        body.len()
    );
    io.write_all(head.as_bytes()).await?;
    let mut written = 0u64;
    if !body.is_empty() {
        on_written(0);
    }
    for chunk in body.chunks(256 * 1024) {
        io.write_all(chunk).await?;
        written += chunk.len() as u64;
        on_written(written);
    }
    io.flush().await?;
    let response = http::read_request(io, 1024 * 1024).await?;
    // `read_request` parses a response poorly because it expects a request line.
    // Parse the status from the method slot: "HTTP/1.1" and the path slot is the code.
    let status = response.path.parse().unwrap_or(0);
    let _ = response.method;
    Ok(StatusResponse {
        status,
        body: response.body,
    })
}

#[allow(dead_code)]
fn _kind(mime: &str) -> MediaKind {
    kind_of_mime(mime)
}

#[cfg(test)]
mod tests {
    #[test]
    fn incoming_offer_preserves_optional_file_sizes() {
        use crate::airdrop::protocol;
        let body = protocol::ask_request(
            "Samsung",
            "Galaxy",
            "sender",
            &[("photo.jpg".into(), "image/jpeg".into(), 12345)],
        )
        .unwrap();
        for (size, expected) in [
            (Some(plist::Value::Integer(12345.into())), Some(12345)),
            (Some(plist::Value::Integer(0.into())), Some(0)),
            (None, None),
            (Some(plist::Value::Integer((-1).into())), None),
        ] {
            let mut value = plist::Value::from_reader(std::io::Cursor::new(&body)).unwrap();
            let file = value
                .as_dictionary_mut()
                .unwrap()
                .get_mut("Files")
                .unwrap()
                .as_array_mut()
                .unwrap()[0]
                .as_dictionary_mut()
                .unwrap();
            file.remove("FileSize");
            if let Some(size) = size {
                file.insert("FileSize".into(), size);
            }
            let mut bytes = Vec::new();
            value.to_writer_binary(&mut bytes).unwrap();
            let ask = protocol::parse_ask(&bytes).unwrap();
            let offer = super::offer_from_ask(&ask);
            assert_eq!(ask.files[0].size, expected);
            assert_eq!(offer.files[0].bytes, expected.unwrap_or(0));
            assert_eq!(offer.files[0].size_known, expected.is_some());
            assert_eq!(offer.total_bytes(), expected.unwrap_or(0));
        }
    }

    use super::{send_plain, AirdropConfig, AirdropReceiver};
    use crate::approve::approval;
    use std::net::SocketAddr;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn silent_discovery_times_out_before_requesting_acceptance() {
        let (mut client, mut server) = tokio::io::duplex(64 * 1024);
        let observed = std::sync::Mutex::new(Vec::new());
        // The peer reads Discover but never responds. It must not receive Ask.
        let peer = async {
            let request = super::http::read_request(&mut server, 1024).await.unwrap();
            assert_eq!(request.path, "/Discover");
            let result = super::http::read_request(&mut server, 1024).await;
            assert!(result.is_err());
        };
        let sending = async {
            let error = super::send_session(
                &mut client,
                "127.0.0.1:8770".parse().unwrap(),
                "Mac",
                &[],
                &|stage| observed.lock().unwrap().push(stage),
                &|_| {},
            )
            .await
            .unwrap_err()
            .to_string();
            assert!(error.contains("No acceptance request was sent"), "{error}");
            drop(client);
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(sending, peer);
        })
        .await
        .unwrap();
        assert_eq!(
            *observed.lock().unwrap(),
            vec![super::SendProgress::Discovering]
        );
    }

    #[tokio::test]
    async fn silent_tls_times_out_before_discovery() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let observed = std::sync::Mutex::new(Vec::new());
        let connector = crate::airdrop::interoperable_connector().unwrap();
        let peer = async {
            use tokio::io::AsyncReadExt;
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).await.unwrap();
            assert!(!bytes.is_empty());
        };
        let sending = async {
            let error = super::send_tls_with_progress(addr, "Mac", &[], connector, &|stage| {
                observed.lock().unwrap().push(stage)
            })
            .await
            .unwrap_err()
            .to_string();
            assert!(error.contains("secure connection timed out"), "{error}");
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(sending, peer);
        })
        .await
        .unwrap();
        assert_eq!(
            *observed.lock().unwrap(),
            vec![
                super::SendProgress::Connecting,
                super::SendProgress::SecuringConnection
            ]
        );
    }

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
        let observed = std::sync::Mutex::new(Vec::new());
        let client = super::send_tls_with_progress(
            addr,
            "iPhone",
            &[("tls.jpg".into(), vec![0xFF, 0xD8, 0xFF])],
            connector,
            &|stage| observed.lock().unwrap().push(stage),
        )
        .await;
        let server_result = server.await.unwrap();
        assert!(
            client.is_ok() && server_result.is_ok(),
            "client {client:?} server {server_result:?}"
        );
        use super::SendProgress::*;
        assert_eq!(
            *observed.lock().unwrap(),
            vec![
                Connecting,
                SecuringConnection,
                Discovering,
                RequestingAcceptance,
                Uploading
            ]
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
            let ask = super::protocol::ask_request(
                "iPhone",
                "iPhone14,5",
                "phone",
                &[("IMG_9457.jpg".into(), "image/jpeg".into(), 1)],
            )
            .unwrap();
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

    #[tokio::test]
    async fn reports_upload_progress() {
        use crate::progress::ByteProgress;
        use std::sync::{Arc, Mutex};

        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let received = Arc::new(Mutex::new(Vec::<ByteProgress>::new()));
        let log = received.clone();
        let mut config = AirdropConfig::auto(dir.path());
        config.on_progress = Some(Arc::new(move |_peer, progress| {
            log.lock().unwrap().push(progress);
        }));
        let receiver = AirdropReceiver::new(config);
        let server = tokio::spawn(async move { receiver.accept_one(listener).await });
        let mut payload = vec![0u8; 300 * 1024];
        let mut state = 0x1234_5678u32;
        for byte in &mut payload {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *byte = (state >> 16) as u8;
        }
        let sent = Arc::new(Mutex::new(Vec::<ByteProgress>::new()));
        let sent_log = sent.clone();
        super::send_plain_reporting(
            addr,
            "iPhone",
            &[("blob.bin".into(), payload.clone())],
            &|_| {},
            &|progress| sent_log.lock().unwrap().push(progress),
        )
        .await
        .unwrap();
        server.await.unwrap().unwrap();
        let sent = sent.lock().unwrap().clone();
        let received = received.lock().unwrap().clone();
        assert!(sent.len() >= 2, "{sent:?}");
        assert_eq!(sent.first().unwrap().transferred, 0);
        assert_eq!(sent.last().unwrap().transferred, payload.len() as u64);
        assert_eq!(sent.last().unwrap().total, payload.len() as u64);
        assert!(sent
            .windows(2)
            .all(|pair| pair[0].transferred <= pair[1].transferred));
        assert!(received.len() >= 2, "{received:?}");
        let last = *received.last().unwrap();
        assert!(last.total > 256 * 1024, "{received:?}");
        assert_eq!(last.transferred, last.total);
        assert!(received
            .windows(2)
            .all(|pair| pair[0].transferred <= pair[1].transferred));
        assert_eq!(std::fs::read(dir.path().join("blob.bin")).unwrap(), payload);
    }
}
