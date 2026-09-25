use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use prost::Message;
use rand::RngCore;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::{interval, timeout, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use crate::approve::{approve_all, Approval, IncomingFile, TransferOffer};
use crate::error::{self, Error, Result};
use crate::mime::{kind_of_mime, sniff, MediaKind};
use crate::paths::{destination, partial_path};
use crate::quickshare::endpoint::{self, EndpointInfo, DEVICE_LAPTOP};
use crate::quickshare::secure::SecureChannel;
use crate::quickshare::ukey2::{ClientHandshake, ServerHandshake};
use crate::quickshare::wire::{conn, share};
use crate::sanitize::safe_file_name;

const CHUNK: usize = 512 * 1024;

#[derive(Clone)]
pub struct QuickshareConfig {
    pub dir: PathBuf,
    pub name: String,
    pub sort_media: bool,
    pub max_file_bytes: u64,
    pub approve: Approval,
    pub device_type: u8,
}

impl QuickshareConfig {
    pub fn auto(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            name: "Crossdrop".into(),
            sort_media: false,
            max_file_bytes: 32 * 1024 * 1024 * 1024,
            approve: approve_all(),
            device_type: DEVICE_LAPTOP,
        }
    }
}

#[derive(Debug, Default)]
pub struct TransferDone {
    pub accepted: bool,
    pub files: Vec<PathBuf>,
    pub bytes: u64,
}

pub async fn serve(
    listener: TcpListener,
    config: QuickshareConfig,
    cancel: CancellationToken,
) -> Result<()> {
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (socket, peer) = accepted?;
                tracing::debug!(%peer, "quick share connection");
                let config = config.clone();
                tokio::spawn(async move {
                    if let Err(error) = handle_server(socket, config).await {
                        tracing::debug!(%error, "quick share session ended");
                    }
                });
            }
            _ = cancel.cancelled() => return Ok(()),
        }
    }
}

pub async fn accept_one(listener: TcpListener, config: QuickshareConfig) -> Result<TransferDone> {
    let (socket, _) = timeout(Duration::from_secs(10), listener.accept())
        .await
        .map_err(|_| Error::Timeout)??;
    handle_server(socket, config).await
}

pub async fn send_paths(addr: SocketAddr, name: &str, paths: &[PathBuf]) -> Result<TransferDone> {
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        files.push(Outgoing::open(path).await?);
    }
    send_prepared(addr, name, files).await
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn send_buffers(
    addr: SocketAddr,
    name: &str,
    files: Vec<(String, Vec<u8>)>,
) -> Result<TransferDone> {
    let prepared = files
        .into_iter()
        .map(|(name, bytes)| Outgoing::memory(name, bytes))
        .collect();
    send_prepared(addr, name, prepared).await
}

async fn send_prepared(
    addr: SocketAddr,
    name: &str,
    mut outgoing_files: Vec<Outgoing>,
) -> Result<TransferDone> {
    let socket = timeout(Duration::from_secs(10), TcpStream::connect(addr))
        .await
        .map_err(|_| Error::Timeout)??;
    let _ = socket.set_nodelay(true);
    let (mut reader, writer) = socket.into_split();
    let (outgoing, writer_task) = spawn_writer(writer);

    let endpoint_id = endpoint::random_endpoint_id();
    let info = EndpointInfo::visible(name, endpoint::DEVICE_PHONE);
    let mut nonce = [0u8; 4];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    send_raw(
        &outgoing,
        &conn::OfflineFrame::new(
            conn::CONNECTION_REQUEST,
            conn::V1Frame {
                connection_request: Some(conn::ConnectionRequestFrame {
                    endpoint_id: Some(endpoint_id),
                    endpoint_name: Some(name.to_string()),
                    nonce: Some(i32::from_le_bytes(nonce)),
                    mediums: vec![conn::WIFI_LAN],
                    endpoint_info: Some(info.encode()?),
                    keep_alive_interval_millis: Some(5_000),
                    keep_alive_timeout_millis: Some(30_000),
                }),
                ..conn::V1Frame::empty()
            },
        )
        .encode_to_vec(),
    )?;
    let client_hs = ClientHandshake::start()?;
    let client_finish = client_hs.client_finish().to_vec();
    send_raw(&outgoing, client_hs.client_init())?;
    let server_init = read_body(&mut reader).await?;
    let secrets = client_hs.finish(&server_init)?;
    let pin = secrets.pin();
    tracing::info!(%pin, "quick share pin, compare it with the receiver");
    send_raw(&outgoing, &client_finish)?;
    let peer_response = read_message::<conn::OfflineFrame>(&mut reader).await?;
    if !connection_accepted(&peer_response) {
        return Err(Error::Rejected("receiver rejected the connection".into()));
    }
    send_raw(
        &outgoing,
        &conn::OfflineFrame::connection_response().encode_to_vec(),
    )?;
    let mut secure = secrets.client_channel()?;

    let mut sent_key = false;
    let mut sent_result = false;
    let mut got_key = false;
    let mut got_result = false;
    let mut sent_intro = false;
    let mut accepted = false;
    let mut index = 0usize;
    let mut assemblers: HashMap<i64, ByteBuf> = HashMap::new();
    let mut ticker = keepalive_ticker();

    loop {
        tokio::select! {
            biased;
            body = read_body(&mut reader) => {
                let plain = secure.decrypt(&body?)?;
                let frame = decode_frame::<conn::OfflineFrame>(&plain)?;
                match frame.v1.as_ref().and_then(|v1| v1.frame_type) {
                    Some(conn::KEEP_ALIVE) | Some(conn::DISCONNECTION) => {}
                    Some(conn::PAYLOAD_TRANSFER) => {
                        if let Some(bytes) = push_payload(&mut assemblers, frame.v1.unwrap().payload_transfer.unwrap())? {
                            if let Ok(share_frame) = decode_frame::<share::Frame>(&bytes) {
                                if share_frame.is_control() {
                                    match share_frame.v1.as_ref().and_then(|v1| v1.frame_type) {
                                        Some(share::PAIRED_KEY_ENCRYPTION) => got_key = true,
                                        Some(share::PAIRED_KEY_RESULT) => got_result = true,
                                        Some(share::RESPONSE) => {
                                            let status = share_frame.v1.unwrap().connection_response.unwrap().status.unwrap_or(0);
                                            if status != share::ACCEPT {
                                                return Err(Error::Rejected(format!("receiver status {status}")));
                                            }
                                            accepted = true;
                                        }
                                        Some(share::CANCEL) => return Err(Error::Rejected("cancelled".into())),
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ = ticker.tick() => {
                send_secure(&outgoing, &mut secure, &conn::OfflineFrame::keep_alive())?;
            }
            _ = std::future::ready(()), if !sent_key && got_key => {
                send_bytes_payload(&outgoing, &mut secure, &paired_encryption())?;
                sent_key = true;
            }
            _ = std::future::ready(()), if sent_key && got_key && !sent_result => {
                send_bytes_payload(&outgoing, &mut secure, &paired_result())?;
                sent_result = true;
            }
            _ = std::future::ready(()), if sent_result && got_result && !sent_intro => {
                send_bytes_payload(&outgoing, &mut secure, &introduction(&outgoing_files))?;
                sent_intro = true;
            }
            _ = std::future::ready(()), if accepted && index < outgoing_files.len() => {
                if send_next_chunk(&outgoing, &mut secure, &mut outgoing_files[index]).await? {
                    index += 1;
                }
            }
            _ = std::future::ready(()), if accepted && index >= outgoing_files.len() => {
                let _ = send_secure(&outgoing, &mut secure, &conn::OfflineFrame::disconnect());
                break;
            }
        }
    }
    drop(outgoing);
    let _ = writer_task.await;
    let bytes = outgoing_files.iter().map(|file| file.total).sum();
    Ok(TransferDone {
        accepted: true,
        files: Vec::new(),
        bytes,
    })
}

async fn handle_server(socket: TcpStream, config: QuickshareConfig) -> Result<TransferDone> {
    let _ = socket.set_nodelay(true);
    let (mut reader, writer) = socket.into_split();
    let (outgoing, writer_task) = spawn_writer(writer);
    let request = read_message::<conn::OfflineFrame>(&mut reader).await?;
    let peer_name = peer_name_from_request(&request);
    let client_init = read_body(&mut reader).await?;
    let server_hs = ServerHandshake::start(&client_init)?;
    send_raw(&outgoing, server_hs.server_init())?;
    let client_finish = read_body(&mut reader).await?;
    let secrets = server_hs.finish(&client_finish)?;
    let pin = secrets.pin();
    tracing::info!(%pin, peer = %peer_name, "quick share pin");
    send_raw(
        &outgoing,
        &conn::OfflineFrame::connection_response().encode_to_vec(),
    )?;
    let peer_response = read_message::<conn::OfflineFrame>(&mut reader).await?;
    if !connection_accepted(&peer_response) {
        return Err(Error::Rejected("sender rejected the connection".into()));
    }
    let mut secure = secrets.server_channel()?;
    send_bytes_payload(&outgoing, &mut secure, &paired_encryption())?;

    let mut got_key = false;
    let mut sent_result = false;
    let mut accepted = false;
    let mut files: HashMap<i64, InFile> = HashMap::new();
    let mut assemblers: HashMap<i64, ByteBuf> = HashMap::new();
    let (decision_tx, mut decision_rx) = mpsc::channel(1);
    let mut waiting = false;
    let mut ticker = keepalive_ticker();
    let mut done = TransferDone::default();

    loop {
        tokio::select! {
            biased;
            body = read_body(&mut reader) => {
                let plain = secure.decrypt(&body?)?;
                let frame = decode_frame::<conn::OfflineFrame>(&plain)?;
                let frame_type = frame.v1.as_ref().and_then(|v1| v1.frame_type);
                if frame_type == Some(conn::DISCONNECTION) {
                    break;
                }
                if frame_type != Some(conn::PAYLOAD_TRANSFER) {
                    continue;
                }
                let transfer = frame.v1.unwrap().payload_transfer.unwrap();
                let payload_type = transfer.payload_header.as_ref().and_then(|header| header.payload_type);
                let payload_id = transfer.payload_header.as_ref().and_then(|header| header.id).unwrap_or(0);
                if transfer.packet_type == Some(conn::PACKET_ACK) {
                    continue;
                }
                if payload_type == Some(conn::FILE) {
                    if !accepted {
                        return Err(Error::protocol("file bytes arrived before acceptance"));
                    }
                    absorb_file(&config, &outgoing, &mut secure, &mut files, &mut done, transfer).await?;
                    if !files.is_empty() && files.values().all(|file| file.done) {
                        break;
                    }
                    continue;
                }
                if let Some(bytes) = push_payload(&mut assemblers, transfer)? {
                    if let Ok(share_frame) = decode_frame::<share::Frame>(&bytes) {
                        if share_frame.is_control() {
                            on_share_frame(
                                &config,
                                &peer_name,
                                &pin,
                                &outgoing,
                                &mut secure,
                                &decision_tx,
                                &mut got_key,
                                &mut sent_result,
                                &mut waiting,
                                &mut accepted,
                                &mut files,
                                share_frame,
                            )?;
                            continue;
                        }
                    }
                    if accepted {
                        if let Some(file) = files.get_mut(&payload_id) {
                            if file.kind_name == "text" {
                                write_bytes_file(&config, file, &bytes).await?;
                                file.done = true;
                                done.files.push(file.dest.clone());
                                done.bytes += bytes.len() as u64;
                                if files.values().all(|item| item.done) {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            decision = decision_rx.recv(), if waiting => {
                waiting = false;
                let allow = decision.unwrap_or(false);
                let status = if allow { share::ACCEPT } else { share::REJECT };
                send_bytes_payload(&outgoing, &mut secure, &share_response(status))?;
                if !allow {
                    done.accepted = false;
                    break;
                }
                accepted = true;
                done.accepted = true;
            }
            _ = ticker.tick() => {
                send_secure(&outgoing, &mut secure, &conn::OfflineFrame::keep_alive())?;
            }
        }
    }
    drop(outgoing);
    let _ = writer_task.await;
    if !done.accepted {
        return Ok(done);
    }
    Ok(done)
}

fn on_share_frame(
    config: &QuickshareConfig,
    peer_name: &str,
    pin: &str,
    outgoing: &mpsc::UnboundedSender<Vec<u8>>,
    secure: &mut SecureChannel,
    decision_tx: &mpsc::Sender<bool>,
    got_key: &mut bool,
    sent_result: &mut bool,
    waiting: &mut bool,
    accepted: &mut bool,
    files: &mut HashMap<i64, InFile>,
    frame: share::Frame,
) -> Result<()> {
    let v1 = frame
        .v1
        .ok_or_else(|| Error::protocol("empty share frame"))?;
    match v1.frame_type {
        Some(share::PAIRED_KEY_ENCRYPTION) => {
            *got_key = true;
            if !*sent_result {
                send_bytes_payload(outgoing, secure, &paired_result())?;
                *sent_result = true;
            }
        }
        Some(share::PAIRED_KEY_RESULT) | Some(share::CANCEL) => {
            if v1.frame_type == Some(share::CANCEL) {
                return Err(Error::Rejected("cancelled".into()));
            }
        }
        Some(share::INTRODUCTION) => {
            let intro = v1.introduction.unwrap_or(share::IntroductionFrame {
                file_metadata: Vec::new(),
                text_metadata: Vec::new(),
            });
            let mut offer_files = Vec::new();
            for file in intro.file_metadata {
                let name = file.name.unwrap_or_default();
                safe_file_name(&name)?;
                let size = u64::try_from(file.size.unwrap_or(0)).unwrap_or(u64::MAX);
                if size > config.max_file_bytes {
                    send_bytes_payload(outgoing, secure, &share_response(share::NOT_ENOUGH_SPACE))?;
                    return Err(Error::TooLarge);
                }
                let mime = file
                    .mime_type
                    .unwrap_or_else(|| sniff(&[], &name).1.to_string());
                let id = file.payload_id.unwrap_or(0);
                files.insert(
                    id,
                    InFile {
                        name: name.clone(),
                        size,
                        mime: mime.clone(),
                        kind: kind_of_mime(&mime),
                        kind_name: "file".into(),
                        written: 0,
                        partial: None,
                        dest: PathBuf::new(),
                        done: false,
                    },
                );
                offer_files.push(IncomingFile {
                    name,
                    bytes: size,
                    mime,
                    kind: MediaKind::Other,
                });
            }
            for text in intro.text_metadata {
                let id = text.payload_id.unwrap_or(0);
                let size = u64::try_from(text.size.unwrap_or(0)).unwrap_or(0);
                let name = text
                    .text_title
                    .filter(|title| safe_file_name(title).is_ok())
                    .map(|title| {
                        if title.ends_with(".txt") {
                            title
                        } else {
                            format!("{title}.txt")
                        }
                    })
                    .unwrap_or_else(|| "shared-text.txt".into());
                files.insert(
                    id,
                    InFile {
                        name: name.clone(),
                        size,
                        mime: "text/plain".into(),
                        kind: MediaKind::Other,
                        kind_name: "text".into(),
                        written: 0,
                        partial: None,
                        dest: PathBuf::new(),
                        done: false,
                    },
                );
                offer_files.push(IncomingFile {
                    name,
                    bytes: size,
                    mime: "text/plain".into(),
                    kind: MediaKind::Other,
                });
            }
            if offer_files.is_empty() {
                return Err(Error::protocol("quick share introduction has no files"));
            }
            // Fix kinds now that mime is known.
            for item in &mut offer_files {
                item.kind = kind_of_mime(&item.mime);
            }
            let offer = TransferOffer {
                protocol: "quickshare",
                peer: peer_name.to_string(),
                pin: Some(pin.to_string()),
                files: offer_files,
            };
            let approve = config.approve.clone();
            let decision_tx = decision_tx.clone();
            *waiting = true;
            tokio::spawn(async move {
                let allow = match timeout(Duration::from_secs(120), approve(offer)).await {
                    Ok(value) => value,
                    Err(_) => false,
                };
                let _ = decision_tx.send(allow).await;
            });
            let _ = accepted;
        }
        _ => {}
    }
    Ok(())
}

async fn absorb_file(
    config: &QuickshareConfig,
    outgoing: &mpsc::UnboundedSender<Vec<u8>>,
    secure: &mut SecureChannel,
    files: &mut HashMap<i64, InFile>,
    done: &mut TransferDone,
    transfer: conn::PayloadTransferFrame,
) -> Result<()> {
    let header = transfer
        .payload_header
        .ok_or_else(|| Error::protocol("file payload is missing a header"))?;
    let id = header.id.unwrap_or(0);
    let file = files
        .get_mut(&id)
        .ok_or_else(|| Error::protocol("unknown quick share file"))?;
    let chunk = transfer.payload_chunk.unwrap_or(conn::PayloadChunk {
        flags: None,
        offset: Some(0),
        body: None,
    });
    let offset = u64::try_from(chunk.offset.unwrap_or(0)).unwrap_or(u64::MAX);
    let body = chunk.body.unwrap_or_default();
    if offset != file.written {
        return Err(Error::protocol("quick share file chunk is out of order"));
    }
    if file.written + body.len() as u64 > file.size.max(file.written) && file.size > 0 {
        return Err(Error::protocol(
            "quick share file exceeded its declared size",
        ));
    }
    if file.partial.is_none() {
        let dest = destination(&config.dir, &file.name, file.kind, config.sort_media)?;
        let partial = partial_path(&dest);
        let handle = PartialFile::create(&partial).await?;
        file.dest = dest;
        file.partial = Some(handle);
    }
    if !body.is_empty() {
        file.partial.as_mut().unwrap().write(&body).await?;
        file.written += body.len() as u64;
    }
    let last = chunk.flags.unwrap_or(0) & conn::LAST_CHUNK != 0;
    if last {
        if file.size != file.written {
            return Err(Error::protocol(format!(
                "{} is incomplete ({} of {} bytes)",
                file.name, file.written, file.size
            )));
        }
        let dest = file.dest.clone();
        if let Some(partial) = file.partial.as_mut() {
            partial.commit(&dest).await?;
        }
        file.done = true;
        done.files.push(dest);
        done.bytes += file.written;
        send_secure(
            outgoing,
            secure,
            &conn::payload_ack(id, conn::FILE, file.size as i64),
        )?;
    }
    Ok(())
}

async fn write_bytes_file(
    config: &QuickshareConfig,
    file: &mut InFile,
    bytes: &[u8],
) -> Result<()> {
    safe_file_name(&file.name)?;
    let dest = destination(&config.dir, &file.name, file.kind, config.sort_media)?;
    let partial = partial_path(&dest);
    let mut handle = PartialFile::create(&partial).await?;
    handle.write(bytes).await?;
    handle.commit(&dest).await?;
    file.dest = dest;
    Ok(())
}

struct InFile {
    name: String,
    size: u64,
    #[allow(dead_code)]
    mime: String,
    kind: MediaKind,
    kind_name: String,
    written: u64,
    partial: Option<PartialFile>,
    dest: PathBuf,
    done: bool,
}

enum Source {
    #[cfg_attr(not(test), allow(dead_code))]
    Memory(Vec<u8>),
    File(tokio::fs::File),
}

struct Outgoing {
    name: String,
    mime: String,
    kind: MediaKind,
    payload_id: i64,
    total: u64,
    source: Source,
    offset: u64,
}

impl Outgoing {
    #[cfg_attr(not(test), allow(dead_code))]
    fn memory(name: String, data: Vec<u8>) -> Self {
        let (kind, mime) = sniff(&data[..data.len().min(64)], &name);
        Self::from_parts(
            name,
            mime.to_string(),
            kind,
            data.len() as u64,
            Source::Memory(data),
        )
    }

    async fn open(path: &Path) -> Result<Self> {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or(Error::UnsafeName)?;
        safe_file_name(name)?;
        let mut file = tokio::fs::File::open(path).await?;
        let total = file.metadata().await?.len();
        let mut header = [0u8; 64];
        let read = file.read(&mut header).await?;
        file.seek(std::io::SeekFrom::Start(0)).await?;
        let (kind, mime) = sniff(&header[..read], name);
        Ok(Self::from_parts(
            name.to_string(),
            mime.to_string(),
            kind,
            total,
            Source::File(file),
        ))
    }

    fn from_parts(name: String, mime: String, kind: MediaKind, total: u64, source: Source) -> Self {
        Self {
            name,
            mime,
            kind,
            payload_id: new_id(),
            total,
            source,
            offset: 0,
        }
    }
}

struct ByteBuf {
    total: Option<u64>,
    buf: Vec<u8>,
}

fn push_payload(
    assemblers: &mut HashMap<i64, ByteBuf>,
    transfer: conn::PayloadTransferFrame,
) -> Result<Option<Vec<u8>>> {
    if transfer.packet_type == Some(conn::PACKET_ACK) {
        return Ok(None);
    }
    let header = transfer.payload_header.as_ref();
    if header.and_then(|header| header.payload_type) == Some(conn::FILE) {
        return Ok(None);
    }
    let id = header.and_then(|header| header.id).unwrap_or(0);
    let entry = assemblers.entry(id).or_insert_with(|| ByteBuf {
        total: header
            .and_then(|header| header.total_size)
            .map(|size| size as u64),
        buf: Vec::new(),
    });
    let chunk = transfer.payload_chunk.unwrap_or(conn::PayloadChunk {
        flags: None,
        offset: Some(0),
        body: None,
    });
    let offset = chunk.offset.unwrap_or(0) as usize;
    let body = chunk.body.unwrap_or_default();
    if offset != entry.buf.len() {
        return Err(Error::protocol(
            "quick share control payload is out of order",
        ));
    }
    if entry.buf.len() + body.len() > wire_max() {
        return Err(Error::TooLarge);
    }
    entry.buf.extend_from_slice(&body);
    let last = chunk.flags.unwrap_or(0) & conn::LAST_CHUNK != 0;
    if last {
        if let Some(total) = entry.total {
            if entry.buf.len() as u64 != total {
                return Err(Error::protocol("control payload size mismatch"));
            }
        }
        return Ok(Some(std::mem::take(&mut entry.buf)));
    }
    Ok(None)
}

async fn send_next_chunk(
    outgoing: &mpsc::UnboundedSender<Vec<u8>>,
    secure: &mut SecureChannel,
    file: &mut Outgoing,
) -> Result<bool> {
    if file.total == 0 && file.offset == 0 {
        send_secure(
            outgoing,
            secure,
            &conn::file_chunk(file.payload_id, 0, 0, &[], true, &file.name),
        )?;
        return Ok(true);
    }
    let mut buffer = vec![0u8; CHUNK];
    let read = match &mut file.source {
        Source::Memory(data) => {
            let start = file.offset as usize;
            if start >= data.len() {
                return Ok(true);
            }
            let count = (data.len() - start).min(CHUNK);
            buffer[..count].copy_from_slice(&data[start..start + count]);
            count
        }
        Source::File(handle) => handle.read(&mut buffer).await?,
    };
    if read == 0 {
        if file.offset != file.total {
            return Err(Error::protocol(format!(
                "{} ended before {} bytes",
                file.name, file.total
            )));
        }
        return Ok(true);
    }
    let last = file.offset + read as u64 >= file.total;
    send_secure(
        outgoing,
        secure,
        &conn::file_chunk(
            file.payload_id,
            file.total as i64,
            file.offset as i64,
            &buffer[..read],
            last,
            &file.name,
        ),
    )?;
    file.offset += read as u64;
    Ok(last)
}

fn introduction(files: &[Outgoing]) -> Vec<u8> {
    share::Frame::new(
        share::INTRODUCTION,
        share::V1Frame {
            introduction: Some(share::IntroductionFrame {
                file_metadata: files
                    .iter()
                    .map(|file| share::FileMetadata {
                        name: Some(file.name.clone()),
                        file_type: Some(file.kind.quickshare_type()),
                        payload_id: Some(file.payload_id),
                        size: Some(file.total as i64),
                        mime_type: Some(file.mime.clone()),
                        id: Some(file.payload_id),
                    })
                    .collect(),
                text_metadata: Vec::new(),
            }),
            ..share::V1Frame::empty()
        },
    )
    .encode_to_vec()
}

fn paired_encryption() -> Vec<u8> {
    let mut signed = vec![0u8; 72];
    let mut hash = vec![0u8; 6];
    rand::rngs::OsRng.fill_bytes(&mut signed);
    rand::rngs::OsRng.fill_bytes(&mut hash);
    share::Frame::new(
        share::PAIRED_KEY_ENCRYPTION,
        share::V1Frame {
            paired_key_encryption: Some(share::PairedKeyEncryptionFrame {
                signed_data: Some(signed),
                secret_id_hash: Some(hash),
            }),
            ..share::V1Frame::empty()
        },
    )
    .encode_to_vec()
}

fn paired_result() -> Vec<u8> {
    share::Frame::new(
        share::PAIRED_KEY_RESULT,
        share::V1Frame {
            paired_key_result: Some(share::PairedKeyResultFrame {
                status: Some(share::UNABLE),
            }),
            ..share::V1Frame::empty()
        },
    )
    .encode_to_vec()
}

fn share_response(status: i32) -> Vec<u8> {
    share::Frame::new(
        share::RESPONSE,
        share::V1Frame {
            connection_response: Some(share::ConnectionResponseFrame {
                status: Some(status),
            }),
            ..share::V1Frame::empty()
        },
    )
    .encode_to_vec()
}

fn send_bytes_payload(
    outgoing: &mpsc::UnboundedSender<Vec<u8>>,
    secure: &mut SecureChannel,
    payload: &[u8],
) -> Result<()> {
    for frame in conn::bytes_payload(new_id(), payload) {
        send_secure(outgoing, secure, &frame)?;
    }
    Ok(())
}

fn send_secure(
    outgoing: &mpsc::UnboundedSender<Vec<u8>>,
    secure: &mut SecureChannel,
    frame: &conn::OfflineFrame,
) -> Result<()> {
    let encrypted = secure.encrypt(&frame.encode_to_vec())?;
    send_raw(outgoing, &encrypted)
}

fn send_raw(outgoing: &mpsc::UnboundedSender<Vec<u8>>, body: &[u8]) -> Result<()> {
    outgoing.send(body.to_vec()).map_err(|_| Error::Closed)
}

fn connection_accepted(frame: &conn::OfflineFrame) -> bool {
    let Some(v1) = &frame.v1 else {
        return false;
    };
    if v1.frame_type != Some(conn::CONNECTION_RESPONSE) {
        return false;
    }
    v1.connection_response
        .as_ref()
        .and_then(|response| response.response)
        != Some(2)
}

fn peer_name_from_request(frame: &conn::OfflineFrame) -> String {
    let Some(request) = frame
        .v1
        .as_ref()
        .and_then(|v1| v1.connection_request.as_ref())
    else {
        return "Android".into();
    };
    if let Some(info) = request
        .endpoint_info
        .as_ref()
        .and_then(|bytes| EndpointInfo::decode(bytes).ok())
    {
        if let Some(name) = info.name {
            return name;
        }
    }
    request
        .endpoint_name
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Android".into())
}

fn new_id() -> i64 {
    let mut bytes = [0u8; 8];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let value = i64::from_le_bytes(bytes) & i64::MAX;
    if value == 0 {
        1
    } else {
        value
    }
}

fn keepalive_ticker() -> tokio::time::Interval {
    let mut ticker = interval(Duration::from_secs(5));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    ticker
}

fn wire_max() -> usize {
    crate::quickshare::wire::MAX_FRAME
}

fn decode_frame<T: Message + Default>(bytes: &[u8]) -> Result<T> {
    T::decode(bytes).map_err(|error| Error::protocol(error.to_string()))
}

async fn read_message<T: Message + Default>(
    reader: &mut tokio::net::tcp::OwnedReadHalf,
) -> Result<T> {
    decode_frame(&read_body(reader).await?)
}

async fn read_body(reader: &mut tokio::net::tcp::OwnedReadHalf) -> Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    error::read_exact(reader, &mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > wire_max() {
        return Err(Error::TooLarge);
    }
    let mut body = vec![0u8; len];
    error::read_exact(reader, &mut body).await?;
    Ok(body)
}

fn spawn_writer(
    mut writer: tokio::net::tcp::OwnedWriteHalf,
) -> (mpsc::UnboundedSender<Vec<u8>>, tokio::task::JoinHandle<()>) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let task = tokio::spawn(async move {
        while let Some(body) = rx.recv().await {
            if write_body(&mut writer, &body).await.is_err() {
                break;
            }
        }
    });
    (tx, task)
}

async fn write_body(writer: &mut tokio::net::tcp::OwnedWriteHalf, body: &[u8]) -> Result<()> {
    writer.write_u32(body.len() as u32).await?;
    writer.write_all(body).await?;
    Ok(())
}

struct PartialFile {
    path: PathBuf,
    file: Option<tokio::fs::File>,
    committed: bool,
}

impl PartialFile {
    async fn create(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        Ok(Self {
            path: path.to_path_buf(),
            file: Some(tokio::fs::File::create(path).await?),
            committed: false,
        })
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.file
            .as_mut()
            .ok_or_else(|| Error::protocol("partial file is closed"))?
            .write_all(bytes)
            .await?;
        Ok(())
    }

    async fn commit(&mut self, dest: &Path) -> Result<()> {
        if let Some(file) = self.file.as_mut() {
            file.sync_all().await?;
        }
        self.file.take();
        tokio::fs::rename(&self.path, dest).await?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for PartialFile {
    fn drop(&mut self) {
        self.file.take();
        if !self.committed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{accept_one, send_buffers, send_paths, QuickshareConfig};
    use crate::approve::approval;
    use std::net::SocketAddr;
    use tokio::net::TcpListener;

    async fn pair() -> (TcpListener, SocketAddr, tempfile::TempDir) {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let dir = tempfile::tempdir().unwrap();
        (listener, addr, dir)
    }

    #[tokio::test]
    async fn transfers_a_photo_and_a_video() {
        let (listener, addr, dir) = pair().await;
        let config = QuickshareConfig::auto(dir.path());
        let server = tokio::spawn(async move { accept_one(listener, config).await });
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0x00, 0x11];
        let mut video = vec![0, 0, 0, 24];
        video.extend_from_slice(b"ftypisom");
        video.extend(std::iter::repeat(9).take(80_000));
        let sent = send_buffers(
            addr,
            "Pixel",
            vec![
                ("café.jpg".into(), jpeg.clone()),
                ("clip.mp4".into(), video.clone()),
            ],
        )
        .await
        .unwrap();
        assert!(sent.accepted);
        let got = server.await.unwrap().unwrap();
        assert!(got.accepted);
        assert_eq!(std::fs::read(dir.path().join("café.jpg")).unwrap(), jpeg);
        assert_eq!(std::fs::read(dir.path().join("clip.mp4")).unwrap(), video);
        assert_eq!(got.bytes, jpeg.len() as u64 + video.len() as u64);
    }

    #[tokio::test]
    async fn decline_writes_nothing() {
        let (listener, addr, dir) = pair().await;
        let mut config = QuickshareConfig::auto(dir.path());
        config.approve = approval(|_| async { false });
        let server = tokio::spawn(async move { accept_one(listener, config).await });
        let sent = send_buffers(
            addr,
            "Pixel",
            vec![("a.jpg".into(), vec![0xFF, 0xD8, 0xFF])],
        )
        .await;
        assert!(sent.is_err());
        let got = server.await.unwrap().unwrap();
        assert!(!got.accepted);
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    #[tokio::test]
    async fn rejects_a_traversing_name_and_an_oversized_file() {
        let (listener, addr, dir) = pair().await;
        let config = QuickshareConfig::auto(dir.path());
        let server = tokio::spawn(async move { accept_one(listener, config).await });
        let error = send_buffers(addr, "Pixel", vec![("../x".into(), b"pwn".to_vec())])
            .await
            .unwrap_err();
        assert!(server.await.unwrap().is_err());
        assert!(!error.to_string().is_empty());
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());

        let (listener, addr, dir) = pair().await;
        let mut config = QuickshareConfig::auto(dir.path());
        config.max_file_bytes = 4;
        let server = tokio::spawn(async move { accept_one(listener, config).await });
        let error = send_buffers(addr, "Pixel", vec![("big.bin".into(), vec![1, 2, 3, 4, 5])])
            .await
            .unwrap_err();
        assert!(error.to_string().contains("status") || server.await.unwrap().is_err());
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    #[tokio::test]
    async fn send_paths_roundtrips_from_disk() {
        let (listener, addr, dir) = pair().await;
        let source = tempfile::tempdir().unwrap();
        let path = source.path().join("note.txt");
        std::fs::write(&path, b"hello").unwrap();
        let config = QuickshareConfig::auto(dir.path());
        let server = tokio::spawn(async move { accept_one(listener, config).await });
        send_paths(addr, "Phone", &[path]).await.unwrap();
        server.await.unwrap().unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("note.txt")).unwrap(),
            b"hello"
        );
    }
}
