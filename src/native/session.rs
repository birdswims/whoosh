use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rand::RngCore;
use rustls::pki_types::CertificateDer;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Semaphore;
use tokio::time::timeout;

use crate::approve::{approve_all, Approval, IncomingFile, TransferOffer};
use crate::error::{self, Error, Result};
use crate::mime::{kind_of_mime, sniff};
use crate::native::codec::{self, Clipboard, Control, FileOffer, Hello, Offer, MAGIC};
use crate::native::tls::{
    self, client_config, client_config_with_identity, server_config, IdentityCert, Trust,
};
use crate::paths::{destination, partial_path};
use crate::progress::{ByteMeter, PeerProgressHook, ProgressHook};
use crate::sanitize::safe_file_name;

const BUF: usize = 1024 * 1024;
const DECISION_TIMEOUT: Duration = Duration::from_secs(120);
const CLIPBOARD_DENIED: &str = "not a trusted device";

/// Decides whether a presented device certificate may read the clipboard.
pub type DeviceAllow = Arc<dyn Fn(&[u8; 32]) -> bool + Send + Sync>;

/// Latest clipboard snapshot held in memory. It is never written to disk.
#[derive(Clone, Default)]
pub struct ClipboardStore {
    inner: Arc<std::sync::Mutex<ClipboardItem>>,
}

/// What a trusted device can paste.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClipboardItem {
    pub text: String,
    pub image_png: Vec<u8>,
}

impl ClipboardStore {
    pub fn publish(&self, text: String, image_png: Vec<u8>) -> Result<()> {
        if text.len() > codec::MAX_CLIPBOARD_TEXT {
            return Err(Error::protocol("clipboard text is too long"));
        }
        if image_png.len() > codec::MAX_CLIPBOARD_IMAGE {
            return Err(Error::protocol("clipboard image is too large"));
        }
        if !image_png.is_empty() && !image_png.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err(Error::protocol("clipboard image is not a png"));
        }
        *self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = ClipboardItem { text, image_png };
        Ok(())
    }

    fn get(&self) -> ClipboardItem {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }
}

fn allow_none() -> DeviceAllow {
    Arc::new(|_| false)
}

#[derive(Debug)]
pub enum SessionResult {
    Files(ReceiveReport),
    Clipboard,
}

#[derive(Clone)]
pub struct ReceiveOptions {
    pub dir: PathBuf,
    pub sort_media: bool,
    pub max_file_bytes: u64,
    pub approve: Approval,
    /// Called with the sender's name as file bytes arrive. Empty for callers that
    /// only need the finished report.
    pub on_progress: Option<PeerProgressHook>,
    pub clipboard: ClipboardStore,
    /// Fingerprints allowed to pull the clipboard. File transfers do not use this.
    pub allow_device: DeviceAllow,
}

impl ReceiveOptions {
    pub fn auto(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            sort_media: false,
            max_file_bytes: 32 * 1024 * 1024 * 1024,
            approve: approve_all(),
            on_progress: None,
            clipboard: ClipboardStore::default(),
            allow_device: allow_none(),
        }
    }
}

#[derive(Debug)]
pub struct ReceiveReport {
    pub peer: String,
    pub files: Vec<PathBuf>,
    pub bytes: u64,
}

#[derive(Debug)]
pub struct SendReport {
    pub fingerprint: [u8; 32],
    pub files: usize,
    pub bytes: u64,
}

pub struct NativeListener {
    endpoint: quinn::Endpoint,
    pub local_addr: SocketAddr,
    pub cert_der: CertificateDer<'static>,
    pub fingerprint: [u8; 32],
    name: String,
    device_id: [u8; 16],
    pin: Option<String>,
}

impl NativeListener {
    pub fn bind(addr: SocketAddr, name: impl Into<String>, pin: Option<String>) -> Result<Self> {
        let identity = tls::generate_identity()?;
        Self::bind_with(addr, name, pin, identity)
    }

    pub(crate) fn bind_with(
        addr: SocketAddr,
        name: impl Into<String>,
        pin: Option<String>,
        identity: IdentityCert,
    ) -> Result<Self> {
        let config = server_config(&identity)?;
        let endpoint = quinn::Endpoint::server(config, addr)?;
        let local_addr = endpoint.local_addr()?;
        let mut device_id = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut device_id);
        Ok(Self {
            endpoint,
            local_addr,
            cert_der: identity.cert_der,
            fingerprint: identity.fingerprint,
            name: name.into(),
            device_id,
            pin,
        })
    }

    pub fn pin(&self) -> Option<&str> {
        self.pin.as_deref()
    }

    pub async fn receive_one(&self, options: ReceiveOptions) -> Result<SessionResult> {
        let incoming = self.endpoint.accept().await.ok_or(Error::Closed)?;
        let connection = incoming
            .await
            .map_err(|error| Error::protocol(error.to_string()))?;
        handle_receive(
            connection,
            &self.name,
            self.device_id,
            self.pin.as_deref(),
            options,
        )
        .await
    }
}

pub async fn pull_clipboard(
    addr: SocketAddr,
    name: &str,
    trust: Trust,
    identity: &IdentityCert,
) -> Result<ClipboardItem> {
    let crypto = client_config_with_identity(trust, Some(identity))?;
    let mut endpoint = quinn::Endpoint::client(SocketAddr::from(([0, 0, 0, 0], 0)))?;
    endpoint.set_default_client_config(crypto.config);
    let connection = endpoint
        .connect(addr, tls::SERVER_NAME)
        .map_err(|error| Error::protocol(error.to_string()))?
        .await
        .map_err(|error| Error::protocol(error.to_string()))?;
    let item = request_clipboard(connection, name).await?;
    endpoint.wait_idle().await;
    Ok(item)
}

pub async fn send_files(
    addr: SocketAddr,
    name: &str,
    files: &[PathBuf],
    pin: Option<&str>,
    trust: Trust,
) -> Result<SendReport> {
    send_files_with_progress(addr, name, files, pin, trust, Arc::new(|_| {})).await
}

pub async fn send_files_with_progress(
    addr: SocketAddr,
    name: &str,
    files: &[PathBuf],
    pin: Option<&str>,
    trust: Trust,
    progress: ProgressHook,
) -> Result<SendReport> {
    if files.is_empty() {
        return Err(Error::protocol("no files to send"));
    }
    let crypto = client_config(trust)?;
    let mut endpoint = quinn::Endpoint::client(SocketAddr::from(([0, 0, 0, 0], 0)))?;
    endpoint.set_default_client_config(crypto.config);
    let connection = endpoint
        .connect(addr, tls::SERVER_NAME)
        .map_err(|error| Error::protocol(error.to_string()))?
        .await
        .map_err(|error| Error::protocol(error.to_string()))?;
    let fingerprint = crypto
        .seen
        .lock()
        .expect("fingerprint lock")
        .unwrap_or([0u8; 32]);
    let report = handle_send(connection, name, files, pin, progress).await?;
    endpoint.wait_idle().await;
    Ok(SendReport {
        fingerprint,
        files: report.files,
        bytes: report.bytes,
    })
}

async fn handle_send(
    connection: quinn::Connection,
    name: &str,
    paths: &[PathBuf],
    pin: Option<&str>,
    progress: ProgressHook,
) -> Result<SendReport> {
    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .map_err(|error| Error::protocol(error.to_string()))?;
    AsyncWriteExt::write_all(&mut send, MAGIC).await?;
    let mut device_id = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut device_id);
    write_control(
        &mut send,
        &Control::Hello(Hello {
            name: name.to_string(),
            device_id,
            pin_required: false,
        }),
    )
    .await?;
    let peer = match read_control(&mut recv).await? {
        Control::Hello(hello) => hello,
        _ => return Err(Error::protocol("expected hello")),
    };
    if peer.pin_required && pin.unwrap_or("").is_empty() {
        return Err(Error::Pin);
    }
    let mut offers = Vec::with_capacity(paths.len());
    let mut total = 0u64;
    for (index, path) in paths.iter().enumerate() {
        let (size, mime, _kind) = describe_file(path)?;
        total = total.saturating_add(size);
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or(Error::UnsafeName)?;
        safe_file_name(name)?;
        offers.push(FileOffer {
            id: index as u32,
            size,
            name: name.to_string(),
            mime,
        });
    }
    let mut transfer_id = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut transfer_id);
    write_control(
        &mut send,
        &Control::Offer(Offer {
            transfer_id,
            pin: pin.unwrap_or("").to_string(),
            files: offers.clone(),
        }),
    )
    .await?;
    match read_control(&mut recv).await? {
        Control::Accept {
            transfer_id: accepted,
        } if accepted == transfer_id => {}
        Control::Reject { reason, .. } => return Err(Error::Rejected(reason)),
        _ => return Err(Error::protocol("expected accept or reject")),
    }

    let meter = ByteMeter::new(total, progress);
    meter.start();
    let semaphore = Arc::new(Semaphore::new(4));
    let mut tasks = Vec::with_capacity(paths.len());
    for (offer, path) in offers.iter().zip(paths.iter()) {
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::Closed)?;
        let connection = connection.clone();
        let offer = offer.clone();
        let path = path.clone();
        let meter = Arc::clone(&meter);
        tasks.push(tokio::spawn(async move {
            let result = send_file(connection, &offer, &path, &meter).await;
            drop(permit);
            result
        }));
    }
    for task in tasks {
        task.await
            .map_err(|error| Error::protocol(error.to_string()))??;
    }
    meter.finish();
    match read_control(&mut recv).await? {
        Control::Done {
            transfer_id: done_id,
        } if done_id == transfer_id => {}
        Control::Reject { reason, .. } => return Err(Error::Rejected(reason)),
        _ => return Err(Error::protocol("expected completion")),
    }
    connection.close(0u32.into(), b"done");
    Ok(SendReport {
        fingerprint: [0u8; 32],
        files: paths.len(),
        bytes: total,
    })
}

async fn handle_receive(
    connection: quinn::Connection,
    name: &str,
    device_id: [u8; 16],
    pin: Option<&str>,
    options: ReceiveOptions,
) -> Result<SessionResult> {
    let (mut send, mut recv) = connection
        .accept_bi()
        .await
        .map_err(|error| Error::protocol(error.to_string()))?;
    let mut magic = [0u8; 8];
    error::read_exact(&mut recv, &mut magic).await?;
    if &magic != MAGIC {
        return Err(Error::protocol("not a whoosh peer"));
    }
    let peer = match read_control(&mut recv).await? {
        Control::Hello(hello) => hello,
        _ => return Err(Error::protocol("expected hello")),
    };
    write_control(
        &mut send,
        &Control::Hello(Hello {
            name: name.to_string(),
            device_id,
            pin_required: pin.is_some(),
        }),
    )
    .await?;
    let offer = match read_control(&mut recv).await? {
        Control::ClipboardPull { request_id } => {
            serve_clipboard(&connection, &mut send, request_id, &options).await?;
            return Ok(SessionResult::Clipboard);
        }
        Control::Offer(offer) => offer,
        _ => return Err(Error::protocol("expected offer")),
    };
    if let Some(expected) = pin {
        if !constant_eq(expected, &offer.pin) {
            write_control(
                &mut send,
                &Control::Reject {
                    transfer_id: offer.transfer_id,
                    reason: "wrong pin".into(),
                },
            )
            .await?;
            finish_peer(&mut send, &connection).await;
            return Err(Error::Pin);
        }
    }
    if offer.files.is_empty() {
        return Err(Error::protocol("empty offer"));
    }
    let mut total = 0u64;
    let mut incoming = Vec::with_capacity(offer.files.len());
    for file in &offer.files {
        safe_file_name(&file.name).map_err(|_| Error::UnsafeName)?;
        if file.size > options.max_file_bytes {
            write_control(
                &mut send,
                &Control::Reject {
                    transfer_id: offer.transfer_id,
                    reason: "file is too large".into(),
                },
            )
            .await?;
            finish_peer(&mut send, &connection).await;
            return Err(Error::TooLarge);
        }
        total = total.saturating_add(file.size);
        incoming.push(IncomingFile {
            name: file.name.clone(),
            bytes: file.size,
            size_known: true,
            mime: file.mime.clone(),
            kind: kind_of_mime(&file.mime),
        });
    }
    let decision = timeout(
        DECISION_TIMEOUT,
        (options.approve)(TransferOffer {
            protocol: "whoosh",
            peer: peer.name.clone(),
            pin: pin.map(str::to_string),
            files: incoming,
        }),
    )
    .await;
    let accepted = matches!(decision, Ok(true));
    if !accepted {
        let reason = if decision.is_err() {
            "timed out"
        } else {
            "declined"
        };
        write_control(
            &mut send,
            &Control::Reject {
                transfer_id: offer.transfer_id,
                reason: reason.into(),
            },
        )
        .await?;
        finish_peer(&mut send, &connection).await;
        return Err(Error::Rejected(reason.into()));
    }
    write_control(
        &mut send,
        &Control::Accept {
            transfer_id: offer.transfer_id,
        },
    )
    .await?;

    let meter = options.on_progress.clone().map(|hook| {
        let peer_name = peer.name.clone();
        ByteMeter::new(total, Arc::new(move |progress| hook(&peer_name, progress)))
    });
    if let Some(meter) = &meter {
        meter.start();
    }

    let offers = Arc::new(
        offer
            .files
            .iter()
            .map(|file| (file.id, file.clone()))
            .collect::<HashMap<_, _>>(),
    );
    let expected = offer.files.len();
    let (tx, mut rx) = tokio::sync::mpsc::channel(expected);
    let mut received = 0usize;
    let mut files = Vec::new();
    let mut bytes = 0u64;
    loop {
        tokio::select! {
            incoming = connection.accept_uni() => {
                let stream = incoming.map_err(|error| Error::protocol(error.to_string()))?;
                let tx = tx.clone();
                let offers = offers.clone();
                let dir = options.dir.clone();
                let sort = options.sort_media;
                let max = options.max_file_bytes;
                let meter = meter.clone();
                tokio::spawn(async move {
                    let result = receive_file(stream, &offers, &dir, sort, max, meter).await;
                    let _ = tx.send(result).await;
                });
            }
            result = rx.recv() => {
                let result = result.ok_or(Error::Closed)?;
                let (path, size) = result?;
                files.push(path);
                bytes += size;
                received += 1;
                if received == expected {
                    break;
                }
            }
        }
    }
    if let Some(meter) = &meter {
        meter.finish();
    }
    write_control(
        &mut send,
        &Control::Done {
            transfer_id: offer.transfer_id,
        },
    )
    .await?;
    finish_peer(&mut send, &connection).await;
    tracing::info!(peer = %peer.name, files = files.len(), bytes, "native transfer complete");
    Ok(SessionResult::Files(ReceiveReport {
        peer: peer.name,
        files,
        bytes,
    }))
}

async fn request_clipboard(connection: quinn::Connection, name: &str) -> Result<ClipboardItem> {
    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .map_err(|error| Error::protocol(error.to_string()))?;
    AsyncWriteExt::write_all(&mut send, MAGIC).await?;
    let mut device_id = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut device_id);
    write_control(
        &mut send,
        &Control::Hello(Hello {
            name: name.to_string(),
            device_id,
            pin_required: false,
        }),
    )
    .await?;
    match read_control(&mut recv).await? {
        Control::Hello(_) => {}
        _ => return Err(Error::protocol("expected hello")),
    }
    let mut request_id = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut request_id);
    write_control(
        &mut send,
        &Control::ClipboardPull { request_id },
    )
    .await?;
    let item = match read_control(&mut recv).await? {
        Control::Clipboard(clipboard) if clipboard.request_id == request_id => ClipboardItem {
            text: clipboard.text,
            image_png: clipboard.image_png,
        },
        Control::Reject { reason, .. } => return Err(Error::Rejected(reason)),
        _ => return Err(Error::protocol("expected clipboard")),
    };
    let _ = send.finish();
    connection.close(0u32.into(), b"clipboard");
    Ok(item)
}

async fn serve_clipboard(
    connection: &quinn::Connection,
    send: &mut quinn::SendStream,
    request_id: [u8; 16],
    options: &ReceiveOptions,
) -> Result<()> {
    // Anonymous LAN peers can open a QUIC connection. The clipboard leaves only
    // when the client certificate is one this computer has already trusted.
    let allowed = client_fingerprint(connection).is_some_and(|fingerprint| (options.allow_device)(&fingerprint));
    if !allowed {
        write_control(
            send,
            &Control::Reject {
                transfer_id: request_id,
                reason: CLIPBOARD_DENIED.into(),
            },
        )
        .await?;
        finish_peer(send, connection).await;
        return Ok(());
    }
    let item = options.clipboard.get();
    write_control(
        send,
        &Control::Clipboard(Clipboard {
            request_id,
            text: item.text,
            image_png: item.image_png,
        }),
    )
    .await?;
    finish_peer(send, connection).await;
    Ok(())
}

fn client_fingerprint(connection: &quinn::Connection) -> Option<[u8; 32]> {
    let identity = connection.peer_identity()?;
    let chain = identity.downcast::<Vec<CertificateDer<'static>>>().ok()?;
    let cert = chain.first()?;
    Some(tls::fingerprint(cert.as_ref()))
}

async fn send_file(
    connection: quinn::Connection,
    offer: &FileOffer,
    path: &Path,
    meter: &ByteMeter,
) -> Result<()> {
    let mut send = connection
        .open_uni()
        .await
        .map_err(|error| Error::protocol(error.to_string()))?;
    AsyncWriteExt::write_all(&mut send, &codec::encode_file_header(offer.id, offer.size)).await?;
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; BUF];
    let mut sent = 0u64;
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        AsyncWriteExt::write_all(&mut send, &buffer[..read]).await?;
        sent += read as u64;
        meter.add(read as u64);
    }
    if sent != offer.size {
        return Err(Error::protocol(format!(
            "{} changed size while it was being sent",
            offer.name
        )));
    }
    AsyncWriteExt::write_all(&mut send, hasher.finalize().as_bytes()).await?;
    send.finish()
        .map_err(|error| Error::protocol(error.to_string()))?;
    Ok(())
}

async fn receive_file(
    mut recv: quinn::RecvStream,
    offers: &HashMap<u32, FileOffer>,
    dir: &Path,
    sort_media: bool,
    max_file_bytes: u64,
    meter: Option<Arc<ByteMeter>>,
) -> Result<(PathBuf, u64)> {
    let mut header = [0u8; 12];
    error::read_exact(&mut recv, &mut header).await?;
    let id = u32::from_le_bytes(header[..4].try_into().unwrap());
    let size = u64::from_le_bytes(header[4..].try_into().unwrap());
    let offer = offers
        .get(&id)
        .ok_or_else(|| Error::protocol("unknown file id"))?;
    if offer.size != size || size > max_file_bytes {
        return Err(Error::protocol("file size does not match the offer"));
    }
    let kind = kind_of_mime(&offer.mime);
    let dest = destination(dir, &offer.name, kind, sort_media)?;
    let partial = partial_path(&dest);
    let mut guard = Partial::create(&partial).await?;
    if size > 0 {
        guard.file().set_len(size).await?;
    }
    let mut hasher = blake3::Hasher::new();
    let mut left = size;
    let mut buffer = vec![0u8; BUF];
    while left > 0 {
        let want = std::cmp::min(buffer.len() as u64, left) as usize;
        let read = AsyncReadExt::read(&mut recv, &mut buffer[..want]).await?;
        if read == 0 {
            return Err(Error::Closed);
        }
        hasher.update(&buffer[..read]);
        guard.file().write_all(&buffer[..read]).await?;
        left -= read as u64;
        if let Some(meter) = &meter {
            meter.add(read as u64);
        }
    }
    let mut expected = [0u8; 32];
    error::read_exact(&mut recv, &mut expected).await?;
    if hasher.finalize().as_bytes() != &expected {
        return Err(Error::Hash(offer.name.clone()));
    }
    guard.commit(&dest).await?;
    Ok((dest, size))
}

struct Partial {
    path: PathBuf,
    file: Option<tokio::fs::File>,
    committed: bool,
}

impl Partial {
    async fn create(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let file = tokio::fs::File::create(path).await?;
        Ok(Self {
            path: path.to_path_buf(),
            file: Some(file),
            committed: false,
        })
    }

    fn file(&mut self) -> &mut tokio::fs::File {
        self.file.as_mut().expect("partial file is open")
    }

    async fn commit(&mut self, dest: &Path) -> Result<()> {
        if let Some(file) = self.file.as_mut() {
            file.sync_all().await?;
        }
        // Windows cannot rename a file that still has an open handle.
        self.file.take();
        tokio::fs::rename(&self.path, dest).await?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for Partial {
    fn drop(&mut self) {
        self.file.take();
        if !self.committed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn describe_file(path: &Path) -> Result<(u64, String, crate::mime::MediaKind)> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(Error::protocol(format!("{} is not a file", path.display())));
    }
    let mut file = std::fs::File::open(path)?;
    let mut header = [0u8; 64];
    let read = std::io::Read::read(&mut file, &mut header)?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("file");
    let (kind, mime) = sniff(&header[..read], name);
    Ok((metadata.len(), mime.to_string(), kind))
}

async fn write_control<W>(send: &mut W, message: &Control) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    send.write_all(&codec::encode(message)?).await?;
    Ok(())
}

async fn read_control<R>(recv: &mut R) -> Result<Control>
where
    R: AsyncRead + Unpin,
{
    let mut len_buf = [0u8; 4];
    error::read_exact(recv, &mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 || len > codec::MAX_CONTROL_FRAME {
        return Err(Error::protocol("invalid control frame"));
    }
    let mut body = vec![0u8; len];
    error::read_exact(recv, &mut body).await?;
    let mut framed = Vec::with_capacity(4 + len);
    framed.extend_from_slice(&len_buf);
    framed.extend_from_slice(&body);
    let (message, used) = codec::decode(&framed)?;
    if used != framed.len() {
        return Err(Error::protocol("trailing control bytes"));
    }
    Ok(message)
}

/// Keep the QUIC driver alive until the peer has read the final control message.
async fn finish_peer(send: &mut quinn::SendStream, connection: &quinn::Connection) {
    let _ = send.finish();
    let _ = timeout(Duration::from_secs(5), connection.closed()).await;
}

fn constant_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.bytes().zip(right.bytes()) {
        diff |= a ^ b;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::{pull_clipboard, send_files, ClipboardStore, NativeListener, ReceiveOptions};
    use crate::approve::{approval, approve_all};
    use crate::native::tls::{generate_identity, Trust};
    use std::net::SocketAddr;
    use std::path::PathBuf;

    async fn transfer(
        files: Vec<(&str, Vec<u8>)>,
        pin: Option<&str>,
        approve: bool,
    ) -> Vec<Vec<u8>> {
        let source = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let mut paths = Vec::new();
        for (name, bytes) in &files {
            let path = source.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            paths.push(path);
        }
        let listener = NativeListener::bind(
            SocketAddr::from(([127, 0, 0, 1], 0)),
            "receiver",
            pin.map(str::to_string),
        )
        .unwrap();
        let addr = listener.local_addr;
        let cert = listener.cert_der.clone();
        let expected_fp = listener.fingerprint;
        let dir = dest.path().to_path_buf();
        let options = ReceiveOptions {
            dir: dir.clone(),
            sort_media: false,
            max_file_bytes: 32 * 1024 * 1024,
            approve: if approve {
                approve_all()
            } else {
                approval(|_| async { false })
            },
            on_progress: None,
            clipboard: ClipboardStore::default(),
            allow_device: super::allow_none(),
        };
        let receive = tokio::spawn(async move { listener.receive_one(options).await });
        let send = send_files(addr, "sender", &paths, pin, Trust::Roots(cert)).await;
        if !approve {
            assert!(
                send.unwrap_err().to_string().contains("declined")
                    || receive.await.unwrap().is_err()
            );
            assert!(std::fs::read_dir(&dir).unwrap().next().is_none());
            return Vec::new();
        }
        let report = send.unwrap();
        assert_eq!(report.files, files.len());
        let super::SessionResult::Files(got) = receive.await.unwrap().unwrap() else {
            panic!("expected a file transfer");
        };
        assert_eq!(got.files.len(), files.len());
        assert_eq!(report.fingerprint, [0u8; 32]); // roots path does not capture the peer cert
        let _ = expected_fp;
        let mut out = Vec::new();
        for (name, bytes) in &files {
            let saved = std::fs::read(dir.join(name)).unwrap();
            assert_eq!(&saved, bytes);
            out.push(saved);
        }
        out
    }

    #[tokio::test]
    async fn sends_photo_video_and_empty_file() {
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xD9];
        let mut video = vec![0, 0, 0, 32];
        video.extend_from_slice(b"ftypisom");
        video.extend(std::iter::repeat(7).take(250_000));
        let got = transfer(
            vec![
                ("café.jpg", jpeg.clone()),
                ("clip.mp4", video.clone()),
                ("empty.bin", Vec::new()),
            ],
            None,
            true,
        )
        .await;
        assert_eq!(got[0], jpeg);
        assert_eq!(got[1], video);
        assert!(got[2].is_empty());
    }

    #[tokio::test]
    async fn pin_and_decline_block_the_transfer() {
        let err = {
            let source = tempfile::tempdir().unwrap();
            let path = source.path().join("a.txt");
            std::fs::write(&path, b"hello").unwrap();
            let listener = NativeListener::bind(
                SocketAddr::from(([127, 0, 0, 1], 0)),
                "r",
                Some("1234".into()),
            )
            .unwrap();
            let addr = listener.local_addr;
            let cert = listener.cert_der.clone();
            let receive = tokio::spawn(async move {
                listener
                    .receive_one(ReceiveOptions::auto(tempfile::tempdir().unwrap().keep()))
                    .await
            });
            let send = send_files(addr, "s", &[path], Some("9999"), Trust::Roots(cert)).await;
            let _ = receive.await;
            send.unwrap_err()
        };
        assert!(err.to_string().contains("pin") || matches!(err, crate::error::Error::Pin));
        transfer(vec![("a.txt", b"abc".to_vec())], None, false).await;
    }

    #[tokio::test]
    async fn rejects_a_wrong_fingerprint() {
        let source = tempfile::tempdir().unwrap();
        let path = source.path().join("a.txt");
        std::fs::write(&path, b"abc").unwrap();
        let listener =
            NativeListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)), "r", None).unwrap();
        let addr = listener.local_addr;
        let receive = tokio::spawn(async move {
            listener
                .receive_one(ReceiveOptions::auto(
                    tempfile::tempdir().unwrap().path().to_path_buf(),
                ))
                .await
        });
        let error = send_files(
            addr,
            "s",
            &[PathBuf::from(&path)],
            None,
            Trust::Fingerprint(Some([9u8; 32])),
        )
        .await
        .unwrap_err();
        assert!(!error.to_string().is_empty());
        receive.abort();
    }

    #[tokio::test]
    async fn reports_byte_progress_for_send_and_receive() {
        use super::send_files_with_progress;
        use crate::progress::ByteProgress;
        use std::sync::{Arc, Mutex};

        let source = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let path = source.path().join("clip.bin");
        let size = 2 * 1024 * 1024 + 64;
        std::fs::write(&path, vec![9u8; size]).unwrap();
        let listener =
            NativeListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)), "receiver", None).unwrap();
        let addr = listener.local_addr;
        let cert = listener.cert_der.clone();
        let sent = Arc::new(Mutex::new(Vec::new()));
        let got = Arc::new(Mutex::new(Vec::new()));
        let sent_log = Arc::clone(&sent);
        let got_log = Arc::clone(&got);
        let mut options = ReceiveOptions::auto(dest.path());
        options.on_progress = Some(Arc::new(move |_peer, progress: ByteProgress| {
            got_log.lock().unwrap().push(progress.transferred);
        }));
        let receive = tokio::spawn(async move { listener.receive_one(options).await });
        let report = send_files_with_progress(
            addr,
            "sender",
            &[path],
            None,
            Trust::Roots(cert),
            Arc::new(move |progress| sent_log.lock().unwrap().push(progress.transferred)),
        )
        .await
        .unwrap();
        receive.await.unwrap().unwrap();
        assert_eq!(report.bytes, size as u64);
        assert_monotonic_end(&sent.lock().unwrap(), size as u64);
        assert_monotonic_end(&got.lock().unwrap(), size as u64);
    }

    fn assert_monotonic_end(values: &[u64], total: u64) {
        assert!(values.len() >= 2, "{values:?}");
        assert!(
            values.windows(2).all(|pair| pair[0] <= pair[1]),
            "{values:?}"
        );
        assert_eq!(*values.last().unwrap(), total);
    }

    #[tokio::test]
    async fn clipboard_is_only_for_a_trusted_device() {
        let listener =
            NativeListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)), "receiver", None).unwrap();
        let addr = listener.local_addr;
        let server_fp = listener.fingerprint;
        let mine = generate_identity().unwrap();
        let stranger = generate_identity().unwrap();
        let allowed = mine.fingerprint;
        let store = ClipboardStore::default();
        let png = b"\x89PNG\r\n\x1a\nclipboard".to_vec();
        store.publish("secret note".into(), png.clone()).unwrap();
        let mut options = ReceiveOptions::auto(tempfile::tempdir().unwrap().keep());
        options.clipboard = store;
        options.allow_device = std::sync::Arc::new(move |fingerprint| fingerprint == &allowed);
        tokio::spawn(async move {
            for _ in 0..2 {
                let _ = listener.receive_one(options.clone()).await;
            }
        });

        let denied = pull_clipboard(
            addr,
            "stranger",
            Trust::Fingerprint(Some(server_fp)),
            &stranger,
        )
        .await
        .unwrap_err();
        assert!(
            denied.to_string().contains("not a trusted device"),
            "{denied}"
        );

        let copied = pull_clipboard(addr, "mine", Trust::Fingerprint(Some(server_fp)), &mine)
            .await
            .unwrap();
        assert_eq!(copied.text, "secret note");
        assert_eq!(copied.image_png, png);
    }
}
