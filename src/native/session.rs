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
use crate::native::codec::{self, Control, FileOffer, Hello, Offer, MAGIC};
use crate::native::tls::{self, client_config, server_config, IdentityCert, Trust};
use crate::paths::{destination, partial_path};
use crate::sanitize::safe_file_name;

const BUF: usize = 1024 * 1024;
const DECISION_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub struct ReceiveOptions {
    pub dir: PathBuf,
    pub sort_media: bool,
    pub max_file_bytes: u64,
    pub approve: Approval,
}

impl ReceiveOptions {
    pub fn auto(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            sort_media: false,
            max_file_bytes: 32 * 1024 * 1024 * 1024,
            approve: approve_all(),
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

    fn bind_with(
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

    pub async fn receive_one(&self, options: ReceiveOptions) -> Result<ReceiveReport> {
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

pub async fn send_files(
    addr: SocketAddr,
    name: &str,
    files: &[PathBuf],
    pin: Option<&str>,
    trust: Trust,
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
    let report = handle_send(connection, name, files, pin).await?;
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
        tasks.push(tokio::spawn(async move {
            let result = send_file(connection, &offer, &path).await;
            drop(permit);
            result
        }));
    }
    for task in tasks {
        task.await
            .map_err(|error| Error::protocol(error.to_string()))??;
    }
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
) -> Result<ReceiveReport> {
    let (mut send, mut recv) = connection
        .accept_bi()
        .await
        .map_err(|error| Error::protocol(error.to_string()))?;
    let mut magic = [0u8; 8];
    error::read_exact(&mut recv, &mut magic).await?;
    if &magic != MAGIC {
        return Err(Error::protocol("not a crossdrop peer"));
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
            mime: file.mime.clone(),
            kind: kind_of_mime(&file.mime),
        });
    }
    let decision = timeout(
        DECISION_TIMEOUT,
        (options.approve)(TransferOffer {
            protocol: "crossdrop",
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
                tokio::spawn(async move {
                    let result = receive_file(stream, &offers, &dir, sort, max).await;
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
    write_control(
        &mut send,
        &Control::Done {
            transfer_id: offer.transfer_id,
        },
    )
    .await?;
    finish_peer(&mut send, &connection).await;
    tracing::info!(peer = %peer.name, files = files.len(), bytes, "native transfer complete");
    Ok(ReceiveReport {
        peer: peer.name,
        files,
        bytes,
    })
}

async fn send_file(connection: quinn::Connection, offer: &FileOffer, path: &Path) -> Result<()> {
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
    if len == 0 || len > 1024 * 1024 {
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
    use super::{send_files, NativeListener, ReceiveOptions};
    use crate::approve::{approval, approve_all};
    use crate::native::tls::Trust;
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
        let got = receive.await.unwrap().unwrap();
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
}
