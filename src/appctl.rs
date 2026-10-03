//! JSON line control channel for the desktop apps.
//!
//! The app writes one command per line on stdin and reads one event per line
//! on stdout. Tracing stays on stderr. Listener logs are pointed at the null
//! device so a `println` cannot split a JSON line.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::airdrop::{
    self, server_acceptor, AirdropConfig, AirdropReceiver, MDNS_FLAGS, SERVICE_TYPE as AIRDROP,
};
use crate::approve::{approval, TransferOffer};
use crate::discover::{self, device_name, Advertisement, FoundPeer, PeerBrowser, PeerUpdate};
use crate::error::{Error, Result};
use crate::mime::MediaKind;
use crate::native::{
    self, fingerprint_hex, generate_identity, identity_from_der, NativeListener, ReceiveOptions,
    Trust,
};
use crate::net::{self, awdl_listeners, bind_airdrop_listener};
use crate::note::{SavedHook, SavedNote};
use crate::progress::{ByteProgress, ProgressHook};
use crate::quickshare::{
    self, random_endpoint_id, service_instance_name, EndpointInfo, QuickshareConfig, DEVICE_LAPTOP,
    SERVICE_TYPE as QUICKSHARE,
};
use crate::radio::Radio;
use crate::trust;

static SAVED_STDOUT: Mutex<Option<File>> = Mutex::new(None);

/// Saves the current stdout handle and sends later `println` output to the null device.
pub(crate) fn prepare_app_stdio() {
    if let Ok(file) = detach_stdout() {
        *SAVED_STDOUT
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(file);
    }
}

fn take_stdout() -> Result<File> {
    if let Some(file) = SAVED_STDOUT
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .take()
    {
        return Ok(file);
    }
    detach_stdout()
}

pub async fn run() -> Result<()> {
    let (tx, rx) = mpsc::unbounded_channel();
    thread::spawn(move || read_commands(tx));
    let emit = Emit::file(take_stdout()?);
    let config_dir = trust::config_dir()?;
    serve(
        rx,
        emit,
        ServeOptions {
            config_dir,
            watch: true,
        },
    )
    .await
}

struct ServeOptions {
    config_dir: PathBuf,
    watch: bool,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct Command {
    id: String,
    op: String,
    name: Option<String>,
    dir: Option<String>,
    native: Option<bool>,
    quickshare: Option<bool>,
    airdrop: Option<bool>,
    sort_media: Option<bool>,
    require_pin: Option<bool>,
    max_mib: Option<u64>,
    enabled: Option<bool>,
    via: Option<String>,
    target: Option<String>,
    files: Option<Vec<String>>,
    pin: Option<String>,
    trust: Option<bool>,
    fingerprint: Option<String>,
    accept: Option<bool>,
    offer: Option<String>,
    peer_name: Option<String>,
}

impl Default for Command {
    fn default() -> Self {
        Self {
            id: String::new(),
            op: String::new(),
            name: None,
            dir: None,
            native: None,
            quickshare: None,
            airdrop: None,
            sort_media: None,
            require_pin: None,
            max_mib: None,
            enabled: None,
            via: None,
            target: None,
            files: None,
            pin: None,
            trust: None,
            fingerprint: None,
            accept: None,
            offer: None,
            peer_name: None,
        }
    }
}

enum Incoming {
    Command(Command),
    Invalid(String),
    Closed,
}

struct Config {
    name: String,
    dir: PathBuf,
    native: bool,
    quickshare: bool,
    airdrop: bool,
    sort_media: bool,
    require_pin: bool,
    max_file_bytes: u64,
}

struct Runtime {
    cancel: CancellationToken,
    tasks: Vec<JoinHandle<()>>,
    adverts: Vec<Advertisement>,
    radio: Option<Radio>,
    pin: Option<String>,
    native_port: Option<u16>,
}

#[derive(Clone, Default)]
struct Filter {
    native_port: Option<u16>,
    quickshare_port: Option<u16>,
    airdrop_port: Option<u16>,
    fingerprint: Option<String>,
    airdrop_instance: Option<String>,
}

#[derive(Clone)]
struct Listed {
    id: String,
    via: &'static str,
    name: String,
    detail: String,
    address: String,
    fingerprint: Option<String>,
    trusted: bool,
    instance: String,
    airdrop_target: Option<Value>,
}

struct LiveReceive {
    id: String,
    via: String,
    peer: String,
    files: usize,
}

struct Host {
    config: Config,
    cert_der: Vec<u8>,
    key_der: Vec<u8>,
    fingerprint: String,
    runtime: Option<Runtime>,
    warnings: Vec<String>,
    offers: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
    incoming: Arc<Mutex<Vec<LiveReceive>>>,
    filter: Arc<Mutex<Filter>>,
}

struct SendJob {
    via: String,
    target: String,
    files: Vec<PathBuf>,
    pin: Option<String>,
    trust: bool,
    fingerprint: Option<String>,
    peer_name: String,
    sender_name: String,
}

#[derive(Clone)]
struct Emit {
    out: Arc<Mutex<EmitOut>>,
}

enum EmitOut {
    File(File),
    #[cfg(test)]
    Memory(Arc<Mutex<Vec<Value>>>),
}

impl Emit {
    fn file(file: File) -> Self {
        Self {
            out: Arc::new(Mutex::new(EmitOut::File(file))),
        }
    }

    #[cfg(test)]
    fn memory(values: Arc<Mutex<Vec<Value>>>) -> Self {
        Self {
            out: Arc::new(Mutex::new(EmitOut::Memory(values))),
        }
    }

    fn event(&self, value: Value) {
        let mut guard = self.out.lock().unwrap_or_else(|poison| poison.into_inner());
        match &mut *guard {
            EmitOut::File(file) => {
                let mut line = serde_json::to_vec(&value).unwrap_or_else(|_| b"{}".to_vec());
                line.push(b'\n');
                let _ = file.write_all(&line);
                let _ = file.flush();
            }
            #[cfg(test)]
            EmitOut::Memory(values) => {
                values
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .push(value);
            }
        }
    }

    fn ack(&self, id: &str, result: std::result::Result<(), Error>) {
        match result {
            Ok(()) => self.event(json!({"ev": "ack", "id": id, "ok": true})),
            Err(error) => {
                self.event(json!({"ev": "ack", "id": id, "ok": false, "error": error.to_string()}))
            }
        }
    }
}

async fn serve(
    mut commands: mpsc::UnboundedReceiver<Incoming>,
    emit: Emit,
    options: ServeOptions,
) -> Result<()> {
    let (cert_der, key_der, fingerprint) = load_or_create_identity(&options.config_dir)?;
    let mut host = Host {
        config: Config {
            name: default_name(),
            dir: default_dir(),
            native: true,
            quickshare: true,
            airdrop: true,
            sort_media: false,
            require_pin: false,
            max_file_bytes: 8192 * 1024 * 1024,
        },
        cert_der,
        key_der,
        fingerprint: fingerprint.clone(),
        runtime: None,
        warnings: Vec::new(),
        offers: Arc::new(Mutex::new(HashMap::new())),
        incoming: Arc::new(Mutex::new(Vec::new())),
        filter: Arc::new(Mutex::new(Filter::default())),
    };
    emit.event(json!({
        "ev": "hello",
        "version": env!("CARGO_PKG_VERSION"),
        "device": host.config.name,
        "fingerprint": fingerprint,
    }));
    emit.event(status(&host));

    let watch = if options.watch {
        let cancel = CancellationToken::new();
        let task = tokio::spawn(discover_loop(
            cancel.clone(),
            host.filter.clone(),
            emit.clone(),
        ));
        Some((cancel, task))
    } else {
        None
    };

    let mut stop = std::pin::pin!(interrupted());
    loop {
        tokio::select! {
            incoming = commands.recv() => {
                if !dispatch(&mut host, incoming, &emit).await {
                    break;
                }
            }
            _ = &mut stop => break,
        }
    }

    if let Some((cancel, task)) = watch {
        cancel.cancel();
        task.abort();
    }
    host.stop_runtime();
    Ok(())
}

async fn dispatch(host: &mut Host, incoming: Option<Incoming>, emit: &Emit) -> bool {
    let Some(incoming) = incoming else {
        return false;
    };
    match incoming {
        Incoming::Closed => false,
        Incoming::Invalid(error) => {
            emit.ack("", Err(Error::protocol(error)));
            true
        }
        Incoming::Command(command) => {
            if command.op == "shutdown" {
                emit.ack(&command.id, Ok(()));
                return false;
            }
            let id = command.id.clone();
            let result = handle(host, &command, emit).await;
            emit.ack(&id, result);
            true
        }
    }
}

async fn handle(host: &mut Host, command: &Command, emit: &Emit) -> Result<()> {
    match command.op.as_str() {
        "hello" => {
            emit.event(json!({
                "ev": "hello",
                "version": env!("CARGO_PKG_VERSION"),
                "device": host.config.name,
                "fingerprint": host.fingerprint,
            }));
            emit.event(status(host));
            Ok(())
        }
        "configure" => {
            let changed = apply_config(host, command)?;
            if changed && host.runtime.is_some() {
                start(host, emit).await?;
            } else {
                emit.event(status(host));
            }
            Ok(())
        }
        "receive" => {
            if command.enabled.unwrap_or(true) {
                start(host, emit).await?;
            } else {
                host.stop_runtime();
                emit.event(status(host));
            }
            Ok(())
        }
        "decide" => {
            let id = command
                .offer
                .clone()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| Error::protocol("missing offer"))?;
            let accept = command.accept.unwrap_or(false);
            let sender = host
                .offers
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .remove(&id)
                .ok_or_else(|| Error::protocol("that transfer is no longer waiting"))?;
            let _ = sender.send(accept);
            Ok(())
        }
        "send" => {
            let job = prepare_send(host, command)?;
            let emit = emit.clone();
            tokio::spawn(async move {
                run_send(job, emit).await;
            });
            Ok(())
        }
        other => Err(Error::protocol(format!("unknown command {other}"))),
    }
}

fn apply_config(host: &mut Host, command: &Command) -> Result<bool> {
    let mut changed = false;
    if let Some(name) = &command.name {
        let name = clean_name(name);
        if name != host.config.name {
            host.config.name = name;
            changed = true;
        }
    }
    if let Some(dir) = &command.dir {
        let dir = clean_dir(dir)?;
        if dir != host.config.dir {
            host.config.dir = dir;
            changed = true;
        }
    }
    changed |= assign_bool(&mut host.config.native, command.native);
    changed |= assign_bool(&mut host.config.quickshare, command.quickshare);
    changed |= assign_bool(&mut host.config.airdrop, command.airdrop);
    changed |= assign_bool(&mut host.config.sort_media, command.sort_media);
    changed |= assign_bool(&mut host.config.require_pin, command.require_pin);
    if let Some(max_mib) = command.max_mib {
        let bytes = max_mib.clamp(1, 32_768).saturating_mul(1024 * 1024);
        if bytes != host.config.max_file_bytes {
            host.config.max_file_bytes = bytes;
            changed = true;
        }
    }
    Ok(changed)
}

async fn start(host: &mut Host, emit: &Emit) -> Result<()> {
    host.stop_runtime();
    tokio::fs::create_dir_all(&host.config.dir).await?;
    let cancel = CancellationToken::new();
    let approve = gui_approval(host.offers.clone(), host.incoming.clone(), emit.clone());
    let mut tasks = Vec::new();
    let mut adverts = Vec::new();
    let mut warnings = Vec::new();
    let mut filter = Filter {
        fingerprint: Some(host.fingerprint.clone()),
        ..Filter::default()
    };
    let pin = host.config.require_pin.then(random_pin);
    let radio = (host.config.quickshare || host.config.airdrop)
        .then(|| Radio::start(host.config.name.clone()));

    if host.config.native {
        match start_native(host, pin.clone(), approve.clone(), cancel.clone(), emit).await {
            Ok((task, advert, port)) => {
                tasks.push(task);
                if let Some(advert) = advert {
                    adverts.push(advert);
                }
                filter.native_port = Some(port);
            }
            Err(error) => warnings.push(format!("Whoosh listener: {error}")),
        }
    }
    if host.config.quickshare {
        match start_quickshare(host, approve.clone(), cancel.clone(), emit).await {
            Ok((task, advert, port)) => {
                tasks.push(task);
                if let Some(advert) = advert {
                    adverts.push(advert);
                }
                filter.quickshare_port = Some(port);
            }
            Err(error) => warnings.push(format!("Quick Share listener: {error}")),
        }
    }
    if host.config.airdrop {
        match start_airdrop(host, approve, cancel.clone(), emit).await {
            Ok((air_tasks, advert, port, instance)) => {
                tasks.extend(air_tasks);
                if let Some(advert) = advert {
                    adverts.push(advert);
                }
                filter.airdrop_port = Some(port);
                filter.airdrop_instance = Some(instance);
            }
            Err(error) => warnings.push(format!("AirDrop listener: {error}")),
        }
    }

    if tasks.is_empty() {
        for advert in adverts {
            advert.shutdown();
        }
        drop(radio);
        let message = if warnings.is_empty() {
            "nothing is listening".to_string()
        } else {
            warnings.join(" ")
        };
        host.warnings = warnings;
        emit.event(status(host));
        return Err(Error::protocol(message));
    }

    *host
        .filter
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = filter;
    host.warnings = warnings;
    host.runtime = Some(Runtime {
        cancel,
        tasks,
        adverts,
        radio,
        pin,
        native_port: host
            .filter
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .native_port,
    });
    emit.event(status(host));
    Ok(())
}

async fn start_native(
    host: &Host,
    pin: Option<String>,
    approve: crate::approve::Approval,
    cancel: CancellationToken,
    emit: &Emit,
) -> Result<(JoinHandle<()>, Option<Advertisement>, u16)> {
    let identity = identity_from_der(host.cert_der.clone(), host.key_der.clone());
    let listener = NativeListener::bind_with(
        SocketAddr::from(([0, 0, 0, 0], 0)),
        host.config.name.clone(),
        pin,
        identity,
    )?;
    let port = listener.local_addr.port();
    let fingerprint = fingerprint_hex(&listener.fingerprint);
    let instance = hex::encode(&listener.fingerprint[..8]);
    let advert = match advertise(
        native::SERVICE_TYPE,
        &instance,
        port,
        vec![
            ("n".into(), host.config.name.clone()),
            ("v".into(), "1".into()),
            ("fp".into(), fingerprint),
        ],
    )
    .await
    {
        Ok(advert) => Some(advert),
        Err(error) => {
            tracing::warn!(%error, "native service is not visible");
            None
        }
    };
    let incoming = host.incoming.clone();
    let options = ReceiveOptions {
        dir: host.config.dir.clone(),
        sort_media: host.config.sort_media,
        max_file_bytes: host.config.max_file_bytes,
        approve,
        on_progress: Some(receive_progress(emit, incoming.clone(), "whoosh")),
    };
    let task_emit = emit.clone();
    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                result = listener.receive_one(options.clone()) => match result {
                    Ok(report) => {
                        let id = finish_live(&incoming, "whoosh", &report.peer);
                        let paths = report
                            .files
                            .iter()
                            .map(|path| path.display().to_string())
                            .collect();
                        activity(
                            &task_emit,
                            &id,
                            "in",
                            "done",
                            &files_phrase(report.files.len(), "Received"),
                            &format!("from {}", report.peer),
                            &report.peer,
                            "whoosh",
                            Some(report.bytes),
                            Some(report.bytes),
                            Some(paths),
                        );
                    }
                    Err(Error::Rejected(_) | Error::Closed) => {
                        if let Some(id) = take_live(&incoming, "whoosh", "") {
                            activity(
                                &task_emit,
                                &id,
                                "in",
                                "failed",
                                "Transfer stopped",
                                "The transfer stopped.",
                                "",
                                "whoosh",
                                None,
                                None,
                                None,
                            );
                        }
                    }
                    Err(Error::Pin) => activity(
                        &task_emit,
                        &random_id(),
                        "in",
                        "failed",
                        "Wrong pin",
                        "A sender did not have your pin.",
                        "",
                        "whoosh",
                        None,
                        None,
                        None,
                    ),
                    Err(error) => {
                        let id = finish_live(&incoming, "whoosh", "");
                        activity(
                            &task_emit,
                            &id,
                            "in",
                            "failed",
                            "Transfer stopped",
                            &error.to_string(),
                            "",
                            "whoosh",
                            None,
                            None,
                            None,
                        );
                    }
                },
                _ = cancel.cancelled() => break,
            }
        }
    });
    Ok((task, advert, port))
}

async fn start_quickshare(
    host: &Host,
    approve: crate::approve::Approval,
    cancel: CancellationToken,
    emit: &Emit,
) -> Result<(JoinHandle<()>, Option<Advertisement>, u16)> {
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], 0))).await?;
    let port = listener.local_addr()?.port();
    let endpoint_id = random_endpoint_id();
    let info = EndpointInfo::visible(&host.config.name, DEVICE_LAPTOP);
    let instance = service_instance_name(&endpoint_id)?;
    let txt_name = info.encode_txt()?;
    let advert = match advertise(QUICKSHARE, &instance, port, vec![("n".into(), txt_name)]).await {
        Ok(advert) => Some(advert),
        Err(error) => {
            tracing::warn!(%error, "quick share service is not visible");
            None
        }
    };
    let incoming = host.incoming.clone();
    let config = QuickshareConfig {
        dir: host.config.dir.clone(),
        name: host.config.name.clone(),
        sort_media: host.config.sort_media,
        max_file_bytes: host.config.max_file_bytes,
        approve,
        device_type: DEVICE_LAPTOP,
        on_saved: Some(saved_hook(emit, incoming.clone(), "quickshare")),
        on_progress: Some(receive_progress(emit, incoming.clone(), "quickshare")),
        on_failed: Some(fail_hook(emit, incoming, "quickshare")),
    };
    let task = tokio::spawn(async move {
        let _ = quickshare::serve(listener, config, cancel).await;
    });
    Ok((task, advert, port))
}

async fn start_airdrop(
    host: &Host,
    approve: crate::approve::Approval,
    cancel: CancellationToken,
    emit: &Emit,
) -> Result<(Vec<JoinHandle<()>>, Option<Advertisement>, u16, String)> {
    let listener = bind_airdrop_listener(SocketAddr::from(([0, 0, 0, 0], 0)))?;
    let port = listener.local_addr()?.port();
    let (acceptor, _) = server_acceptor()?;
    let incoming = host.incoming.clone();
    let receiver = AirdropReceiver::new(AirdropConfig {
        dir: host.config.dir.clone(),
        name: host.config.name.clone(),
        model: "Whoosh".into(),
        sort_media: host.config.sort_media,
        max_file_bytes: host.config.max_file_bytes,
        approve,
        on_saved: Some(saved_hook(emit, incoming.clone(), "airdrop")),
        on_progress: Some(receive_progress(emit, incoming.clone(), "airdrop")),
        on_failed: Some(fail_hook(emit, incoming, "airdrop")),
    });
    let mut tasks = Vec::new();
    let primary = receiver.clone();
    let primary_acceptor = acceptor.clone();
    let primary_cancel = cancel.clone();
    tasks.push(tokio::spawn(async move {
        let _ = primary
            .serve_tls(listener, primary_acceptor, primary_cancel)
            .await;
    }));
    for addr in awdl_listeners(port) {
        match bind_airdrop_listener(addr) {
            Ok(extra) => {
                let receiver = receiver.clone();
                let acceptor = acceptor.clone();
                let cancel = cancel.clone();
                tasks.push(tokio::spawn(async move {
                    let _ = receiver.serve_tls(extra, acceptor, cancel).await;
                }));
            }
            Err(error) => tracing::warn!(%addr, %error, "awdl listener did not bind"),
        }
    }
    let instance = hex::encode(random_bytes::<8>());
    let instance = instance[..12].to_string();
    let advert = match advertise(
        AIRDROP,
        &instance,
        port,
        vec![("flags".into(), MDNS_FLAGS.into())],
    )
    .await
    {
        Ok(advert) => Some(advert),
        Err(error) => {
            tracing::warn!(%error, "airdrop service is not visible");
            None
        }
    };
    Ok((tasks, advert, port, instance))
}

fn saved_hook(emit: &Emit, incoming: Arc<Mutex<Vec<LiveReceive>>>, via: &'static str) -> SavedHook {
    let emit = emit.clone();
    Arc::new(move |saved: SavedNote| {
        let id = finish_live(&incoming, via, &saved.peer);
        let detail = if saved.peer.is_empty() {
            protocol_label(via).to_string()
        } else {
            format!("from {}", saved.peer)
        };
        activity(
            &emit,
            &id,
            "in",
            "done",
            &files_phrase(saved.files, "Received"),
            &detail,
            &saved.peer,
            via,
            Some(saved.bytes),
            Some(saved.bytes),
            Some(saved.paths),
        );
    })
}

fn fail_hook(
    emit: &Emit,
    incoming: Arc<Mutex<Vec<LiveReceive>>>,
    via: &'static str,
) -> crate::progress::FailHook {
    let emit = emit.clone();
    Arc::new(move |peer, message| {
        let id = finish_live(&incoming, via, peer);
        activity(
            &emit,
            &id,
            "in",
            "failed",
            "Transfer stopped",
            message,
            peer,
            via,
            None,
            None,
            None,
        );
    })
}

fn receive_progress(
    emit: &Emit,
    incoming: Arc<Mutex<Vec<LiveReceive>>>,
    via: &'static str,
) -> crate::progress::PeerProgressHook {
    let emit = emit.clone();
    Arc::new(move |peer, progress| {
        let rows = incoming.lock().unwrap_or_else(|poison| poison.into_inner());
        let Some(row) = rows.iter().rev().find(|row| live_match(row, via, peer)) else {
            return;
        };
        let id = row.id.clone();
        let files = row.files;
        let shown = if row.peer.is_empty() {
            peer.to_string()
        } else {
            row.peer.clone()
        };
        drop(rows);
        let detail = if shown.is_empty() {
            protocol_label(via).to_string()
        } else {
            format!("from {shown}")
        };
        activity(
            &emit,
            &id,
            "in",
            "working",
            &files_phrase(files, "Receiving"),
            &detail,
            &shown,
            via,
            Some(progress.transferred),
            some_total(progress.total),
            None,
        );
    })
}

fn some_total(total: u64) -> Option<u64> {
    (total > 0).then_some(total)
}

fn live_match(row: &LiveReceive, via: &str, peer: &str) -> bool {
    row.via == via && (row.peer == peer || row.peer.is_empty() || peer.is_empty())
}

fn finish_live(incoming: &Mutex<Vec<LiveReceive>>, via: &str, peer: &str) -> String {
    take_live(incoming, via, peer).unwrap_or_else(random_id)
}

fn take_live(incoming: &Mutex<Vec<LiveReceive>>, via: &str, peer: &str) -> Option<String> {
    let mut rows = incoming.lock().unwrap_or_else(|poison| poison.into_inner());
    let index = rows.iter().rposition(|row| live_match(row, via, peer))?;
    Some(rows.remove(index).id)
}

fn gui_approval(
    offers: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
    incoming: Arc<Mutex<Vec<LiveReceive>>>,
    emit: Emit,
) -> crate::approve::Approval {
    approval(move |offer: TransferOffer| {
        let offers = offers.clone();
        let incoming = incoming.clone();
        let emit = emit.clone();
        async move {
            let id = random_id();
            let (tx, rx) = oneshot::channel();
            offers
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .insert(id.clone(), tx);
            let files: Vec<Value> = offer
                .files
                .iter()
                .map(|file| {
                    json!({
                        "name": file.name,
                        "bytes": file.bytes,
                        "mime": file.mime,
                        "kind": kind_name(file.kind),
                    })
                })
                .collect();
            emit.event(json!({
                "ev": "offer",
                "id": id,
                "via": offer.protocol,
                "peer": offer.peer,
                "pin": offer.pin,
                "files": files,
            }));
            let accepted = matches!(
                tokio::time::timeout(Duration::from_secs(110), rx).await,
                Ok(Ok(true))
            );
            offers
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .remove(&id);
            emit.event(json!({
                "ev": "offer_resolved",
                "id": id,
                "accepted": accepted,
            }));
            if accepted {
                let total = offer.total_bytes();
                incoming
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .push(LiveReceive {
                        id: id.clone(),
                        via: offer.protocol.to_string(),
                        peer: offer.peer.clone(),
                        files: offer.files.len(),
                    });
                activity(
                    &emit,
                    &id,
                    "in",
                    "working",
                    &files_phrase(offer.files.len(), "Receiving"),
                    &format!("from {}", offer.peer),
                    &offer.peer,
                    offer.protocol,
                    some_total(total).map(|_| 0),
                    some_total(total),
                    None,
                );
            } else {
                activity(
                    &emit,
                    &id,
                    "in",
                    "declined",
                    "Declined",
                    &format!("from {}", offer.peer),
                    &offer.peer,
                    offer.protocol,
                    None,
                    None,
                    None,
                );
            }
            accepted
        }
    })
}

fn prepare_send(host: &Host, command: &Command) -> Result<SendJob> {
    let via = command.via.clone().unwrap_or_default();
    if !matches!(via.as_str(), "whoosh" | "quickshare" | "airdrop") {
        return Err(Error::protocol(
            "choose a Whoosh, Quick Share, or AirDrop device",
        ));
    }
    let target = command
        .target
        .clone()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::protocol("choose a device"))?;
    if discover::parse_peer_addr(&target).is_none() {
        return Err(Error::protocol("device address is invalid"));
    }
    let files = command.files.clone().unwrap_or_default();
    if files.is_empty() {
        return Err(Error::protocol("choose a file to send"));
    }
    if files.len() > 500 {
        return Err(Error::protocol("too many files"));
    }
    let mut paths = Vec::with_capacity(files.len());
    for file in files {
        let path = PathBuf::from(&file);
        let metadata = std::fs::metadata(&path)
            .map_err(|_| Error::protocol(format!("{} is not available", path.display())))?;
        if !metadata.is_file() {
            return Err(Error::protocol(format!("{} is not a file", path.display())));
        }
        paths.push(path);
    }
    if let Some(text) = command
        .fingerprint
        .as_deref()
        .filter(|text| !text.is_empty())
    {
        if trust::parse_fingerprint(text).is_none() {
            return Err(Error::protocol("fingerprint is invalid"));
        }
    }
    Ok(SendJob {
        via,
        peer_name: command
            .peer_name
            .clone()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| target.clone()),
        target,
        files: paths,
        pin: command.pin.clone().filter(|pin| !pin.is_empty()),
        trust: command.trust.unwrap_or(false),
        fingerprint: command.fingerprint.clone().filter(|text| !text.is_empty()),
        sender_name: host.config.name.clone(),
    })
}

async fn run_send(job: SendJob, emit: Emit) {
    let id = random_id();
    let count = job.files.len();
    let detail = if job.via == "airdrop" {
        format!("preparing files for {}", job.peer_name)
    } else {
        format!("to {}", job.peer_name)
    };
    activity(
        &emit,
        &id,
        "out",
        "working",
        &files_phrase(count, "Sending"),
        &detail,
        &job.peer_name,
        &job.via,
        None,
        None,
        None,
    );
    let stages = |stage| {
        let detail = match stage {
            airdrop::SendProgress::Connecting => format!("connecting to {}", job.peer_name),
            airdrop::SendProgress::SecuringConnection => {
                format!("securing connection to {}", job.peer_name)
            }
            airdrop::SendProgress::Discovering => format!("checking AirDrop on {}", job.peer_name),
            airdrop::SendProgress::RequestingAcceptance => {
                format!("requesting acceptance on {}", job.peer_name)
            }
            airdrop::SendProgress::Uploading => format!("transferring to {}", job.peer_name),
        };
        activity(
            &emit,
            &id,
            "out",
            "working",
            &files_phrase(count, "Sending"),
            &detail,
            &job.peer_name,
            &job.via,
            None,
            None,
            None,
        );
    };
    let bytes_emit = emit.clone();
    let bytes_id = id.clone();
    let bytes_peer = job.peer_name.clone();
    let bytes_via = job.via.clone();
    let bytes: ProgressHook = Arc::new(move |progress: ByteProgress| {
        activity(
            &bytes_emit,
            &bytes_id,
            "out",
            "working",
            &files_phrase(count, "Sending"),
            &format!("to {bytes_peer}"),
            &bytes_peer,
            &bytes_via,
            Some(progress.transferred),
            some_total(progress.total),
            None,
        );
    });
    match perform_send(&job, &stages, &bytes).await {
        Ok(sent) => activity(
            &emit,
            &id,
            "out",
            "done",
            &files_phrase(count, "Sent"),
            &format!("to {}", job.peer_name),
            &job.peer_name,
            &job.via,
            Some(sent),
            Some(sent),
            None,
        ),
        Err(error) => activity(
            &emit,
            &id,
            "out",
            "failed",
            "Could not send",
            &error.to_string(),
            &job.peer_name,
            &job.via,
            None,
            None,
            None,
        ),
    }
}

async fn perform_send(
    job: &SendJob,
    progress: &(dyn Fn(airdrop::SendProgress) + Send + Sync),
    bytes: &ProgressHook,
) -> Result<u64> {
    let addr = discover::parse_peer_addr(&job.target)
        .ok_or_else(|| Error::protocol("device address is invalid"))?;
    match job.via.as_str() {
        "whoosh" => {
            let trust = whoosh_trust(addr, job)?;
            let report = native::send_files_with_progress(
                addr,
                &job.sender_name,
                &job.files,
                job.pin.as_deref(),
                trust,
                Arc::clone(bytes),
            )
            .await?;
            let fingerprint = job
                .fingerprint
                .as_deref()
                .and_then(trust::parse_fingerprint)
                .unwrap_or(report.fingerprint);
            if fingerprint != [0u8; 32] {
                trust::remember(&addr, &fingerprint)?;
            }
            Ok(report.bytes)
        }
        "quickshare" => {
            let done = quickshare::send_paths_with_progress(
                addr,
                &job.sender_name,
                &job.files,
                Arc::clone(bytes),
            )
            .await?;
            Ok(done.bytes)
        }
        "airdrop" => {
            let total = file_bytes(&job.files)?;
            let paths = job.files.clone();
            let payloads = tokio::task::spawn_blocking(move || read_payloads(&paths))
                .await
                .map_err(|error| Error::protocol(error.to_string()))??;
            let connector = airdrop::interoperable_connector()?;
            let hook = Arc::clone(bytes);
            airdrop::send_tls_reporting(
                addr,
                &job.sender_name,
                &payloads,
                connector,
                progress,
                &|step| hook(step),
            )
            .await?;
            Ok(total)
        }
        _ => Err(Error::protocol("unknown device")),
    }
}

fn whoosh_trust(addr: SocketAddr, job: &SendJob) -> Result<Trust> {
    if let Some(text) = &job.fingerprint {
        let fingerprint = trust::parse_fingerprint(text)
            .ok_or_else(|| Error::protocol("fingerprint is invalid"))?;
        return Ok(Trust::Fingerprint(Some(fingerprint)));
    }
    if trust::remembered(&addr).is_none() && !job.trust {
        return Err(Error::protocol("trust this device before sending"));
    }
    trust::trust_for(&addr, job.trust)
}

fn file_bytes(paths: &[PathBuf]) -> Result<u64> {
    let mut total = 0u64;
    for path in paths {
        total = total.saturating_add(std::fs::metadata(path)?.len());
    }
    Ok(total)
}

fn read_payloads(paths: &[PathBuf]) -> Result<Vec<(String, Vec<u8>)>> {
    let mut payloads = Vec::with_capacity(paths.len());
    for path in paths {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or(Error::UnsafeName)?;
        crate::sanitize::safe_file_name(name)?;
        payloads.push((name.to_string(), std::fs::read(path)?));
    }
    Ok(payloads)
}

/// How long a device stays after mDNS says it left. A resolver often removes a
/// service for a moment while an address record refreshes, then finds it again.
const PEER_HOLD_SECS: u32 = 8;

async fn discover_loop(cancel: CancellationToken, filter: Arc<Mutex<Filter>>, emit: Emit) {
    let mut previous = String::new();
    let mut roster = Roster::default();
    let mut allow_empty = false;
    let mut quiet_ticks = 0u32;
    loop {
        if cancel.is_cancelled() {
            break;
        }
        let mut browser = match PeerBrowser::open(&[
            (native::SERVICE_TYPE, "whoosh"),
            (QUICKSHARE, "quickshare"),
            (AIRDROP, "airdrop"),
            (crate::airdrop::ALT_SERVICE_TYPE, "airdrop"),
        ]) {
            Ok(browser) => browser,
            Err(error) => {
                tracing::warn!(%error, "nearby discovery did not start");
                if !pause(&cancel, Duration::from_secs(2)).await {
                    break;
                }
                continue;
            }
        };
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return,
                update = browser.recv() => {
                    let Some(update) = update else {
                        tracing::warn!("nearby discovery stopped; starting it again");
                        roster.depart_all();
                        break;
                    };
                    apply_update(&mut roster, &filter, update);
                    if !roster.visible().is_empty() {
                        allow_empty = true;
                    }
                    publish_peers(&mut previous, &roster, &emit, allow_empty);
                    quiet_ticks = 0;
                }
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    let current = current_filter(&filter);
                    roster.tick(|peer| current.hides(peer));
                    quiet_ticks = quiet_ticks.saturating_add(1);
                    if quiet_ticks >= 2 {
                        allow_empty = true;
                    }
                    publish_peers(&mut previous, &roster, &emit, allow_empty);
                }
            }
        }
        if !pause(&cancel, Duration::from_secs(1)).await {
            break;
        }
    }
}

async fn pause(cancel: &CancellationToken, duration: Duration) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(duration) => true,
    }
}

fn current_filter(filter: &Mutex<Filter>) -> Filter {
    filter
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
}

fn apply_update(roster: &mut Roster, filter: &Mutex<Filter>, update: PeerUpdate) {
    let current = current_filter(filter);
    match update {
        PeerUpdate::Resolved { via, peer } => {
            let fullname = peer.fullname.clone();
            let listed = list_found(via, peer);
            let hidden = current.hides(&listed);
            roster.resolve(&fullname, listed, hidden);
        }
        PeerUpdate::Removed { fullname } => roster.depart(&fullname),
    }
}

fn publish_peers(previous: &mut String, roster: &Roster, emit: &Emit, allow_empty: bool) {
    let peers = roster.visible();
    if peers.is_empty() && !allow_empty {
        return;
    }
    let body = Value::Array(peers.iter().map(listed_json).collect());
    let encoded = body.to_string();
    if encoded != *previous {
        *previous = encoded;
        emit.event(json!({"ev": "peers", "peers": body}));
    }
}

fn list_found(via: &str, peer: FoundPeer) -> Listed {
    match via {
        "quickshare" => list_quickshare(peer),
        "airdrop" => list_airdrop(peer),
        _ => list_native(peer),
    }
}

#[derive(Default)]
struct Roster {
    entries: Vec<RosterEntry>,
}

struct RosterEntry {
    key: String,
    peer: Listed,
    departing: bool,
    gone_for: u32,
}

impl Roster {
    fn resolve(&mut self, fullname: &str, peer: Listed, hidden: bool) {
        let key = roster_key(fullname, &peer.id);
        if hidden {
            self.entries.retain(|entry| {
                entry.key != key && entry.peer.id != peer.id && !same_device(&entry.peer, &peer)
            });
            return;
        }
        if let Some(entry) = self.entries.iter_mut().find(|entry| {
            entry.key == key || entry.peer.id == peer.id || same_device(&entry.peer, &peer)
        }) {
            // A phone starts a fresh advertisement when a transfer begins. The
            // old record is the one that just went stale, so keep its row and
            // point it at the new record instead of showing both.
            let departing = entry.departing;
            entry.key = key.clone();
            merge_listed(&mut entry.peer, peer, departing);
            entry.departing = false;
            entry.gone_for = 0;
        } else {
            self.entries.push(RosterEntry {
                key: key.clone(),
                peer,
                departing: false,
                gone_for: 0,
            });
        }
        let Some(primary) = self.entries.iter().position(|entry| entry.key == key) else {
            return;
        };
        let peer = self.entries[primary].peer.clone();
        self.entries
            .retain(|entry| entry.key == key || !same_device(&entry.peer, &peer));
    }

    fn depart(&mut self, fullname: &str) {
        let key = dns_key(fullname);
        if key.is_empty() {
            return;
        }
        let Some(index) = self.entries.iter().position(|entry| entry.key == key) else {
            return;
        };
        let peer = self.entries[index].peer.clone();
        let still_advertised = self.entries.iter().enumerate().any(|(other, entry)| {
            other != index && !entry.departing && same_device(&entry.peer, &peer)
        });
        if still_advertised {
            self.entries.remove(index);
        } else {
            self.entries[index].departing = true;
        }
    }

    fn depart_all(&mut self) {
        for entry in &mut self.entries {
            entry.departing = true;
        }
    }

    fn tick(&mut self, mut hidden: impl FnMut(&Listed) -> bool) {
        self.entries.retain(|entry| !hidden(&entry.peer));
        for entry in &mut self.entries {
            if entry.departing {
                entry.gone_for = entry.gone_for.saturating_add(1);
            }
        }
        self.entries.retain(|entry| entry.gone_for < PEER_HOLD_SECS);
    }

    fn visible(&self) -> Vec<Listed> {
        let mut peers: Vec<Listed> = self
            .entries
            .iter()
            .map(|entry| entry.peer.clone())
            .collect();
        peers.sort_by(|left, right| {
            via_rank(left.via)
                .cmp(&via_rank(right.via))
                .then(left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                .then(left.id.cmp(&right.id))
        });
        peers
    }
}

fn same_device(left: &Listed, right: &Listed) -> bool {
    if left.via != right.via {
        return false;
    }
    if !left.instance.is_empty() && left.instance.eq_ignore_ascii_case(&right.instance) {
        return true;
    }
    if let (Some(left_host), Some(right_host)) = (host_of(&left.address), host_of(&right.address)) {
        if left_host == right_host {
            return true;
        }
    }
    if left.via == "whoosh" {
        return match (&left.fingerprint, &right.fingerprint) {
            (Some(left_fp), Some(right_fp)) => {
                !left_fp.is_empty() && left_fp.eq_ignore_ascii_case(right_fp)
            }
            _ => false,
        };
    }
    let left_name = display_key(&left.name);
    let right_name = display_key(&right.name);
    left_name.is_some() && left_name == right_name
}

fn host_of(address: &str) -> Option<IpAddr> {
    discover::parse_peer_addr(address).map(|addr| addr.ip())
}

fn display_key(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed == "Quick Share device" || trimmed == "AirDrop device" {
        None
    } else {
        Some(trimmed.to_lowercase())
    }
}

fn merge_listed(stored: &mut Listed, fresh: Listed, take_fresh_address: bool) {
    // An iPhone is reachable on its AWDL address. A later Wi-Fi record for the
    // same service must not replace that scoped link-local address.
    let replace_address = if scoped_addr(&stored.address) && !scoped_addr(&fresh.address) {
        false
    } else if scoped_addr(&fresh.address) && !scoped_addr(&stored.address) {
        true
    } else {
        take_fresh_address || addr_rank_str(&fresh.address) <= addr_rank_str(&stored.address)
    };
    let placeholder = fresh.name.is_empty()
        || fresh.name == "Quick Share device"
        || fresh.name == "AirDrop device";
    if !placeholder {
        stored.name = fresh.name;
    }
    stored.trusted = fresh.trusted;
    if fresh.fingerprint.is_some() {
        stored.fingerprint = fresh.fingerprint;
    }
    if !fresh.instance.is_empty() {
        stored.instance = fresh.instance;
    }
    if replace_address {
        stored.address = fresh.address;
        stored.airdrop_target = fresh.airdrop_target;
    }
    stored.detail = if stored.via == "whoosh" {
        stored
            .fingerprint
            .as_deref()
            .map(short_fp)
            .unwrap_or_else(|| stored.address.clone())
    } else if replace_address {
        fresh.detail
    } else {
        stored.detail.clone()
    };
}

fn addr_rank_str(address: &str) -> u8 {
    discover::parse_peer_addr(address)
        .map(|addr| discover::addr_rank(addr.ip()))
        .unwrap_or(u8::MAX)
}

fn scoped_addr(address: &str) -> bool {
    matches!(
        discover::parse_peer_addr(address),
        Some(SocketAddr::V6(addr)) if addr.scope_id() != 0
    )
}

fn roster_key(fullname: &str, id: &str) -> String {
    let key = dns_key(fullname);
    if key.is_empty() {
        id.to_string()
    } else {
        key
    }
}

fn dns_key(name: &str) -> String {
    name.trim_matches('.').to_ascii_lowercase()
}

fn list_native(peer: FoundPeer) -> Listed {
    let name = peer
        .txt
        .get("n")
        .cloned()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| peer.instance.clone());
    let fingerprint = peer
        .txt
        .get("fp")
        .cloned()
        .filter(|value| trust::parse_fingerprint(value).is_some());
    let trusted = trust::remembered(&peer.addr).is_some();
    let address = peer.addr.to_string();
    let detail = fingerprint
        .as_deref()
        .map(short_fp)
        .unwrap_or_else(|| address.clone());
    Listed {
        id: peer_id("whoosh", &peer.instance, fingerprint.as_deref(), &address),
        via: "whoosh",
        name,
        detail,
        trusted,
        fingerprint,
        address,
        instance: peer.instance,
        airdrop_target: None,
    }
}

fn list_quickshare(peer: FoundPeer) -> Listed {
    let name = quickshare::nearby_label(
        &peer.txt,
        peer.txt_raw.get("n").map(Vec::as_slice),
        &peer.instance,
        &peer.hostname,
    );
    let address = peer.addr.to_string();
    // The Nearby endpoint id changes when a transfer starts. The display name
    // is what stays put, so the row does not split into two.
    let id = if name == "Quick Share device" {
        peer_id("quickshare", &peer.instance, None, &address)
    } else {
        format!("quickshare|{}", name.to_lowercase())
    };
    Listed {
        id,
        via: "quickshare",
        name,
        detail: address.clone(),
        address,
        fingerprint: None,
        trusted: false,
        instance: peer.instance,
        airdrop_target: None,
    }
}

fn list_airdrop(peer: FoundPeer) -> Listed {
    let target = json!({
        "service_name": peer.instance,
        "host_name": peer.hostname,
        "port": peer.addr.port(),
        "flags": peer.txt.get("flags").and_then(|value| value.parse::<u64>().ok()),
    });
    let recorded = peer
        .txt
        .get("name")
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    let name = recorded.unwrap_or_else(|| {
        if discover::unnamed_airdrop_instance(&peer.instance) {
            "AirDrop device".into()
        } else {
            peer.instance.clone()
        }
    });
    let address = peer.addr.to_string();
    Listed {
        id: peer_id("airdrop", &peer.instance, None, &address),
        via: "airdrop",
        name,
        detail: address.clone(),
        address,
        fingerprint: None,
        trusted: false,
        instance: peer.instance,
        airdrop_target: Some(target),
    }
}

fn peer_id(via: &str, instance: &str, fingerprint: Option<&str>, address: &str) -> String {
    let token = if via == "whoosh" {
        fingerprint
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase())
    } else {
        None
    };
    let token = token.unwrap_or_else(|| {
        let instance = instance.trim();
        if instance.is_empty() {
            address.to_string()
        } else {
            instance.to_ascii_lowercase()
        }
    });
    format!("{via}|{token}")
}

fn listed_json(peer: &Listed) -> Value {
    json!({
        "id": peer.id,
        "via": peer.via,
        "name": peer.name,
        "detail": peer.detail,
        "address": peer.address,
        "fingerprint": peer.fingerprint,
        "trusted": peer.trusted,
        "airdrop_target": peer.airdrop_target,
    })
}

impl Filter {
    fn hides(&self, peer: &Listed) -> bool {
        let Some(addr) = discover::parse_peer_addr(&peer.address) else {
            return false;
        };
        let local = is_local(addr.ip());
        match peer.via {
            "whoosh" => {
                let same_cert = peer.fingerprint.is_some() && self.fingerprint == peer.fingerprint;
                let same_socket = self.native_port == Some(addr.port()) && local;
                same_cert || same_socket
            }
            "quickshare" => self.quickshare_port == Some(addr.port()) && local,
            "airdrop" => {
                let same_instance = !peer.instance.is_empty()
                    && self.airdrop_instance.as_ref() == Some(&peer.instance);
                let same_socket = self.airdrop_port == Some(addr.port()) && local;
                same_instance || same_socket
            }
            _ => false,
        }
    }
}

impl Host {
    fn stop_runtime(&mut self) {
        self.offers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clear();
        self.incoming
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clear();
        let Some(runtime) = self.runtime.take() else {
            *self
                .filter
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = Filter::default();
            return;
        };
        runtime.cancel.cancel();
        for task in runtime.tasks {
            task.abort();
        }
        for advert in runtime.adverts {
            advert.shutdown();
        }
        drop(runtime.radio);
        *self
            .filter
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Filter::default();
        self.warnings.clear();
    }
}

fn status(host: &Host) -> Value {
    let runtime = host.runtime.as_ref();
    json!({
        "ev": "status",
        "receiving": runtime.is_some(),
        "name": host.config.name,
        "dir": host.config.dir.display().to_string(),
        "fingerprint": host.fingerprint,
        "pin": runtime.and_then(|runtime| runtime.pin.clone()),
        "native": host.config.native,
        "quickshare": host.config.quickshare,
        "airdrop": host.config.airdrop,
        "sort_media": host.config.sort_media,
        "require_pin": host.config.require_pin,
        "visibility": net::visibility_report(),
        "address": net::lan_address().map(|lan| lan.ip.to_string()),
        "native_port": runtime.and_then(|runtime| runtime.native_port),
        "warnings": host.warnings,
    })
}

fn activity(
    emit: &Emit,
    id: &str,
    direction: &str,
    state: &str,
    title: &str,
    detail: &str,
    peer: &str,
    via: &str,
    bytes: Option<u64>,
    total: Option<u64>,
    paths: Option<Vec<String>>,
) {
    emit.event(json!({
        "ev": "activity",
        "id": id,
        "direction": direction,
        "state": state,
        "title": title,
        "detail": detail,
        "peer": peer,
        "via": via,
        "bytes": bytes,
        "total": total,
        "paths": paths.unwrap_or_default(),
    }));
}

async fn advertise(
    service_type: &str,
    instance: &str,
    port: u16,
    txt: Vec<(String, String)>,
) -> Result<Advertisement> {
    let service_type = service_type.to_string();
    let instance = instance.to_string();
    tokio::task::spawn_blocking(move || {
        let pairs: Vec<(&str, &str)> = txt
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        Advertisement::start(&service_type, &instance, port, &pairs)
    })
    .await
    .map_err(|error| Error::protocol(error.to_string()))?
}

fn load_or_create_identity(dir: &Path) -> Result<(Vec<u8>, Vec<u8>, String)> {
    std::fs::create_dir_all(dir)?;
    let cert_path = dir.join("device-cert.der");
    let key_path = dir.join("device-key.der");
    if let (Ok(cert), Ok(key)) = (std::fs::read(&cert_path), std::fs::read(&key_path)) {
        if !cert.is_empty() && !key.is_empty() {
            let identity = identity_from_der(cert.clone(), key.clone());
            return Ok((cert, key, fingerprint_hex(&identity.fingerprint)));
        }
    }
    let created = generate_identity()?;
    let cert = created.cert_der.as_ref().to_vec();
    let key = created.key_der.secret_der().to_vec();
    let fingerprint = fingerprint_hex(&created.fingerprint);
    std::fs::write(&cert_path, &cert)?;
    write_secret(&key_path, &key)?;
    Ok((cert, key, fingerprint))
}

fn write_secret(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

fn read_commands(tx: mpsc::UnboundedSender<Incoming>) {
    let stdin = std::io::stdin();
    let mut locked = stdin.lock();
    let mut line = String::new();
    loop {
        line.clear();
        match BufRead::read_line(&mut locked, &mut line) {
            Ok(0) => {
                let _ = tx.send(Incoming::Closed);
                break;
            }
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<Command>(trimmed) {
                    Ok(command) => {
                        let _ = tx.send(Incoming::Command(command));
                    }
                    Err(error) => {
                        let _ = tx.send(Incoming::Invalid(error.to_string()));
                    }
                }
            }
            Err(error) => {
                let _ = tx.send(Incoming::Invalid(error.to_string()));
                let _ = tx.send(Incoming::Closed);
                break;
            }
        }
    }
}

async fn interrupted() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
        ) {
            (Ok(mut terminate), Ok(mut interrupt)) => {
                tokio::select! {
                    _ = terminate.recv() => {}
                    _ = interrupt.recv() => {}
                }
            }
            _ => std::future::pending::<()>().await,
        }
    }
    #[cfg(not(unix))]
    {
        // Console Ctrl+C ends the process through the default handler.
        // A desktop app stops this loop by closing stdin or sending `shutdown`.
        // Installing a Ctrl+C handler here races with processes that have no console.
        std::future::pending::<()>().await;
    }
}

fn default_name() -> String {
    airdrop::macos_computer_name()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(device_name)
}

fn default_dir() -> PathBuf {
    home_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("Whoosh")
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| Error::protocol("home directory is not set"))
}

fn clean_name(raw: &str) -> String {
    let name: String = raw
        .chars()
        .filter(|ch| *ch != '\n' && *ch != '\r' && *ch != '\0')
        .take(64)
        .collect();
    let name = name.trim().to_string();
    if name.is_empty() {
        default_name()
    } else {
        name
    }
}

fn clean_dir(raw: &str) -> Result<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().any(|ch| ch == '\0') {
        return Err(Error::protocol("choose a folder"));
    }
    let path = if trimmed == "~" {
        home_dir()?
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        home_dir()?.join(rest)
    } else {
        PathBuf::from(trimmed)
    };
    if !path.is_absolute() {
        return Err(Error::protocol("save folder must be an absolute path"));
    }
    Ok(path)
}

fn assign_bool(slot: &mut bool, next: Option<bool>) -> bool {
    if let Some(next) = next {
        if *slot != next {
            *slot = next;
            return true;
        }
    }
    false
}

fn files_phrase(count: usize, verb: &str) -> String {
    if count == 1 {
        format!("{verb} 1 file")
    } else {
        format!("{verb} {count} files")
    }
}

fn protocol_label(via: &str) -> &'static str {
    match via {
        "whoosh" => "Whoosh",
        "quickshare" => "Quick Share",
        "airdrop" => "AirDrop",
        _ => "Device",
    }
}

fn kind_name(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Photo => "photo",
        MediaKind::Video => "video",
        MediaKind::Audio => "audio",
        MediaKind::Other => "file",
    }
}

fn short_fp(fingerprint: &str) -> String {
    let compact: String = fingerprint
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .take(8)
        .collect();
    if compact.len() == 8 {
        format!("{} {}", &compact[..4], &compact[4..])
    } else {
        compact
    }
}

fn via_rank(via: &str) -> u8 {
    match via {
        "whoosh" => 0,
        "quickshare" => 1,
        "airdrop" => 2,
        _ => 3,
    }
}

fn is_local(addr: IpAddr) -> bool {
    if addr.is_loopback() {
        return true;
    }
    if_addrs::get_if_addrs()
        .map(|list| list.into_iter().any(|iface| iface.ip() == addr))
        .unwrap_or(false)
}

fn random_pin() -> String {
    use rand::Rng;
    format!("{:04}", rand::thread_rng().gen_range(0..10_000))
}

fn random_id() -> String {
    hex::encode(random_bytes::<8>())
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes
}

fn detach_stdout() -> Result<File> {
    #[cfg(unix)]
    {
        detach_stdout_unix()
    }
    #[cfg(windows)]
    {
        detach_stdout_windows()
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(Error::protocol(
            "the Whoosh app control channel is not available on this platform",
        ))
    }
}

#[cfg(unix)]
fn detach_stdout_unix() -> Result<File> {
    use std::os::unix::io::{AsRawFd, FromRawFd};
    let saved = unsafe { libc::dup(1) };
    if saved < 0 {
        return Err(Error::protocol("could not duplicate stdout"));
    }
    let null = std::fs::OpenOptions::new().write(true).open("/dev/null")?;
    let rc = unsafe { libc::dup2(null.as_raw_fd(), 1) };
    if rc < 0 {
        return Err(Error::protocol("could not detach stdout"));
    }
    Ok(unsafe { File::from_raw_fd(saved) })
}

/// Duplicates the stdout handle the parent is reading, then points both the
/// C runtime and the Win32 standard handle at `NUL`.
#[cfg(windows)]
fn detach_stdout_windows() -> Result<File> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle};

    const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5;
    const DUPLICATE_SAME_ACCESS: u32 = 2;
    const O_BINARY: i32 = 0x8000;

    unsafe extern "system" {
        fn GetStdHandle(n_std_handle: u32) -> *mut core::ffi::c_void;
        fn SetStdHandle(n_std_handle: u32, handle: *mut core::ffi::c_void) -> i32;
        fn GetCurrentProcess() -> *mut core::ffi::c_void;
        fn DuplicateHandle(
            source_process: *mut core::ffi::c_void,
            source: *mut core::ffi::c_void,
            target_process: *mut core::ffi::c_void,
            target: *mut *mut core::ffi::c_void,
            desired_access: u32,
            inherit: i32,
            options: u32,
        ) -> i32;
    }
    unsafe extern "C" {
        fn _dup2(src: i32, dst: i32) -> i32;
        fn _open_osfhandle(osfhandle: isize, flags: i32) -> i32;
        fn _close(fd: i32) -> i32;
    }

    let current = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    let invalid = -1isize as *mut core::ffi::c_void;
    if current.is_null() || current == invalid {
        return Err(Error::protocol("could not read stdout"));
    }
    let process = unsafe { GetCurrentProcess() };
    let mut saved = core::ptr::null_mut();
    let duplicated = unsafe {
        DuplicateHandle(
            process,
            current,
            process,
            &mut saved,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if duplicated == 0 || saved.is_null() {
        return Err(Error::protocol("could not duplicate stdout"));
    }

    let nul = std::fs::OpenOptions::new().write(true).open("NUL")?;
    let nul_handle = nul.as_raw_handle() as isize;
    // `_open_osfhandle` takes ownership of this handle.
    std::mem::forget(nul);
    let nul_fd = unsafe { _open_osfhandle(nul_handle, O_BINARY) };
    if nul_fd < 0 {
        return Err(Error::protocol("could not open NUL"));
    }
    let redirected = unsafe { _dup2(nul_fd, 1) };
    unsafe { _close(nul_fd) };
    if redirected != 0 {
        return Err(Error::protocol("could not detach stdout"));
    }

    // Rust writes stdout with `WriteFile` on the Win32 standard handle, which
    // `_dup2` does not change. Keep a second NUL handle alive for the process.
    let win32_nul = std::fs::OpenOptions::new().write(true).open("NUL")?;
    let win32_handle = win32_nul.as_raw_handle() as *mut core::ffi::c_void;
    let updated = unsafe { SetStdHandle(STD_OUTPUT_HANDLE, win32_handle) };
    std::mem::forget(win32_nul);
    if updated == 0 {
        return Err(Error::protocol("could not detach stdout"));
    }
    Ok(unsafe { File::from_raw_handle(saved) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_configure_command() {
        let command: Command = serde_json::from_str(
            r#"{"id":"7","op":"configure","name":"Studio","dir":"/tmp/Whoosh","native":true,"sort_media":true,"max_mib":512}"#,
        )
        .unwrap();
        assert_eq!(command.id, "7");
        assert_eq!(command.sort_media, Some(true));
        assert_eq!(command.quickshare, None);
        assert_eq!(command.max_mib, Some(512));
    }

    #[tokio::test]
    async fn device_identity_survives_a_reload_and_binds() {
        let dir = tempfile::tempdir().unwrap();
        let (cert, key, fingerprint) = load_or_create_identity(dir.path()).unwrap();
        let again = load_or_create_identity(dir.path()).unwrap();
        assert_eq!(again.0, cert);
        assert_eq!(again.1, key);
        assert_eq!(again.2, fingerprint);
        let listener = NativeListener::bind_with(
            SocketAddr::from(([127, 0, 0, 1], 0)),
            "whoosh",
            None,
            identity_from_der(again.0, again.1),
        )
        .unwrap();
        assert_eq!(fingerprint_hex(&listener.fingerprint), fingerprint);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("device-key.der"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0);
        }
    }

    #[test]
    fn hides_this_macs_listener_only() {
        let filter = Filter {
            native_port: Some(45823),
            quickshare_port: Some(22000),
            airdrop_port: Some(8770),
            fingerprint: Some("ab".repeat(32)),
            airdrop_instance: Some("aabbccddeeff".into()),
        };
        assert!(filter.hides(&sample(
            "whoosh",
            "203.0.113.9:9",
            Some(filter.fingerprint.clone().unwrap()),
            "other",
        )));
        assert!(filter.hides(&sample("whoosh", "127.0.0.1:45823", None, "self")));
        assert!(!filter.hides(&sample(
            "whoosh",
            "203.0.113.9:45823",
            Some("cd".repeat(32)),
            "neighbor",
        )));
        assert!(filter.hides(&sample("quickshare", "127.0.0.1:22000", None, "self",)));
        assert!(!filter.hides(&sample("quickshare", "203.0.113.8:22000", None, "phone",)));
        assert!(filter.hides(&sample("airdrop", "203.0.113.7:9", None, "aabbccddeeff")));
        assert!(!filter.hides(&sample("airdrop", "203.0.113.7:8770", None, "someone")));
    }

    fn sample(
        via: &'static str,
        address: &str,
        fingerprint: Option<String>,
        instance: &str,
    ) -> Listed {
        let address = address.to_string();
        Listed {
            id: peer_id(via, instance, fingerprint.as_deref(), &address),
            via,
            name: "Device".into(),
            detail: address.clone(),
            address,
            fingerprint,
            trusted: false,
            instance: instance.into(),
            airdrop_target: None,
        }
    }

    #[test]
    fn nearby_device_stays_through_a_refresh_gap() {
        let mut roster = Roster::default();
        let phone = sample("quickshare", "192.168.1.10:22000", None, "PixelEndpoint");
        roster.resolve("PixelEndpoint._fc9f._tcp.local.", phone, false);
        assert_eq!(roster.visible().len(), 1);
        roster.depart("PixelEndpoint._fc9f._tcp.local.");
        for _ in 0..(PEER_HOLD_SECS - 1) {
            roster.tick(|_| false);
        }
        assert_eq!(roster.visible().len(), 1);
        roster.tick(|_| false);
        assert!(roster.visible().is_empty());
    }

    #[test]
    fn nearby_device_comes_back_before_it_is_dropped() {
        let mut roster = Roster::default();
        let phone = sample("quickshare", "192.168.1.10:22000", None, "PixelEndpoint");
        let fullname = "PixelEndpoint._fc9f._tcp.local.";
        roster.resolve(fullname, phone.clone(), false);
        roster.depart(fullname);
        roster.tick(|_| false);
        roster.resolve(fullname, phone, false);
        for _ in 0..PEER_HOLD_SECS {
            roster.tick(|_| false);
        }
        assert_eq!(roster.visible().len(), 1);
    }

    #[test]
    fn nearby_device_keeps_its_lan_address() {
        let mut roster = Roster::default();
        let fullname = "aabbccddeeff._airdrop._tcp.local.";
        roster.resolve(
            fullname,
            sample("airdrop", "[fe80::1]:8770", None, "aabbccddeeff"),
            false,
        );
        roster.resolve(
            "AABBCCDDEEFF._airdrop._tcp.local",
            sample("airdrop", "192.168.1.20:8770", None, "aabbccddeeff"),
            false,
        );
        let visible = roster.visible();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].address, "192.168.1.20:8770");
        assert_eq!(
            visible[0].id,
            peer_id("airdrop", "aabbccddeeff", None, "192.168.1.20:8770")
        );
        roster.resolve(
            fullname,
            sample("airdrop", "[fe80::2]:8770", None, "aabbccddeeff"),
            false,
        );
        assert_eq!(roster.visible()[0].address, "192.168.1.20:8770");
    }

    #[test]
    fn an_awdl_address_is_not_replaced_by_wi_fi() {
        let mut roster = Roster::default();
        let fullname = "phone._airdrop._tcp.local.";
        roster.resolve(
            fullname,
            sample("airdrop", "[fe80::1%16]:8770", None, "Harry's iPhone"),
            false,
        );
        roster.resolve(
            fullname,
            sample("airdrop", "192.168.1.40:8770", None, "Harry's iPhone"),
            false,
        );
        assert_eq!(roster.visible()[0].address, "[fe80::1%16]:8770");
        roster.resolve(
            fullname,
            sample("airdrop", "[fe80::2%16]:8770", None, "Harry's iPhone"),
            false,
        );
        assert_eq!(roster.visible()[0].address, "[fe80::2%16]:8770");
    }

    #[test]
    fn airdrop_row_uses_the_name_from_discover() {
        use std::collections::HashMap;
        let mut txt = HashMap::new();
        txt.insert("name".into(), "Harry's iPhone".into());
        txt.insert("flags".into(), "111611".into());
        let peer = FoundPeer {
            instance: "aabbccddeeff".into(),
            fullname: "aabbccddeeff._airdrop._tcp.local.".into(),
            addr: "192.168.1.8:8770".parse().unwrap(),
            txt,
            txt_raw: HashMap::new(),
            hostname: "receiver.local".into(),
        };
        let listed = list_airdrop(peer);
        assert_eq!(listed.name, "Harry's iPhone");
        assert_eq!(
            listed_json(&listed)["airdrop_target"],
            json!({
                "service_name": "aabbccddeeff",
                "host_name": "receiver.local",
                "port": 8770,
                "flags": 111611,
            })
        );
    }

    #[test]
    fn airdrop_target_metadata_stays_with_the_selected_address() {
        let mut stored = sample("airdrop", "[fe80::1%16]:8770", None, "same-device");
        stored.airdrop_target = Some(json!({"host_name": "awdl.local", "port": 8770}));
        let mut lan = sample("airdrop", "192.168.1.5:8771", None, "same-device");
        lan.airdrop_target = Some(json!({"host_name": "lan.local", "port": 8771}));
        merge_listed(&mut stored, lan, true);
        assert_eq!(
            stored.airdrop_target.as_ref().unwrap()["host_name"],
            "awdl.local"
        );
        let mut fresh = sample("airdrop", "[fe80::2%16]:8772", None, "same-device");
        fresh.airdrop_target = Some(json!({"host_name": "fresh.local", "port": 8772}));
        merge_listed(&mut stored, fresh, true);
        assert_eq!(
            stored.airdrop_target.as_ref().unwrap()["host_name"],
            "fresh.local"
        );
        assert_eq!(stored.address, "[fe80::2%16]:8772");
    }

    #[test]
    fn distinct_nearby_devices_stay_separate() {
        let mut roster = Roster::default();
        let mut first = sample("airdrop", "192.168.1.8:8770", None, "one");
        first.name = "Kitchen".into();
        let mut second = sample("airdrop", "192.168.1.9:8770", None, "two");
        second.name = "Office".into();
        roster.resolve("one._airdrop._tcp.local.", first, false);
        roster.resolve("two._airdrop._tcp.local.", second, false);
        assert_eq!(roster.visible().len(), 2);
    }

    #[test]
    fn a_transfer_does_not_list_the_same_phone_twice() {
        let mut roster = Roster::default();
        let mut phone = sample("quickshare", "192.168.1.10:22000", None, "endpoint-a");
        phone.name = "Pixel".into();
        roster.resolve("endpoint-a._fc9f._tcp.local.", phone, false);
        let mut again = sample("quickshare", "192.168.1.10:43110", None, "endpoint-b");
        again.name = "Pixel".into();
        roster.resolve("endpoint-b._fc9f._tcp.local.", again, false);
        let visible = roster.visible();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].address, "192.168.1.10:43110");
        assert_eq!(
            visible[0].id,
            peer_id("quickshare", "endpoint-a", None, "192.168.1.10:22000")
        );
        roster.depart("endpoint-a._fc9f._tcp.local.");
        assert_eq!(roster.visible().len(), 1);
        assert!(!roster.entries[0].departing);
    }

    #[test]
    fn two_unnamed_airdrop_hosts_stay_separate() {
        let mut roster = Roster::default();
        let mut first = sample("airdrop", "192.168.1.8:8770", None, "aaaabbbbcccc");
        first.name = "AirDrop device".into();
        let mut second = sample("airdrop", "192.168.1.9:8770", None, "ddddeeeeffff");
        second.name = "AirDrop device".into();
        roster.resolve("aaaabbbbcccc._airdrop._tcp.local.", first, false);
        roster.resolve("ddddeeeeffff._airdrop._tcp.local.", second, false);
        assert_eq!(roster.visible().len(), 2);
    }

    #[test]
    fn the_same_host_is_one_row_when_it_advertises_twice() {
        let mut roster = Roster::default();
        let mut first = sample("airdrop", "192.168.1.8:8770", None, "aaaabbbbcccc");
        first.name = "AirDrop device".into();
        let mut second = sample("airdrop", "192.168.1.8:9000", None, "111122223333");
        second.name = "AirDrop device".into();
        roster.resolve("aaaabbbbcccc._airdrop._tcp.local.", first, false);
        roster.resolve("111122223333._airdrop._tcp.local.", second, false);
        let visible = roster.visible();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].address, "192.168.1.8:9000");
    }

    #[test]
    fn two_whoosh_macs_with_the_same_name_stay_separate() {
        let mut roster = Roster::default();
        let mut first = sample("whoosh", "192.168.1.5:1", Some("ab".repeat(32)), "aaaa");
        first.name = "MacBook Pro".into();
        let mut second = sample("whoosh", "192.168.1.6:1", Some("cd".repeat(32)), "bbbb");
        second.name = "MacBook Pro".into();
        roster.resolve("aaaa._whoosh._udp.local.", first, false);
        roster.resolve("bbbb._whoosh._udp.local.", second, false);
        assert_eq!(roster.visible().len(), 2);
    }

    #[test]
    fn this_mac_is_left_out_of_nearby() {
        let mut roster = Roster::default();
        roster.resolve(
            "abcd._whoosh._udp.local.",
            sample("whoosh", "192.168.1.7:1", Some("ab".repeat(32)), "abcd"),
            true,
        );
        assert!(roster.visible().is_empty());
    }

    #[tokio::test]
    async fn receives_a_file_after_the_app_accepts() {
        let inbox = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let file = source.path().join("note.txt");
        std::fs::write(&file, b"hello from whoosh").unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let emit = Emit::memory(events.clone());
        let (tx, rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(serve(
            rx,
            emit,
            ServeOptions {
                config_dir: config_dir.path().to_path_buf(),
                watch: false,
            },
        ));
        tx.send(Incoming::Command(
            serde_json::from_value(json!({
                "id": "1",
                "op": "configure",
                "name": "Tester",
                "dir": inbox.path(),
                "native": true,
                "quickshare": false,
                "airdrop": false,
                "sort_media": false,
                "require_pin": false,
            }))
            .unwrap(),
        ))
        .unwrap();
        tx.send(Incoming::Command(
            serde_json::from_value(json!({"id": "2", "op": "receive", "enabled": true})).unwrap(),
        ))
        .unwrap();
        let port = wait_for(&events, |event| {
            event.get("ev").and_then(Value::as_str) == Some("status")
                && event.get("receiving").and_then(Value::as_bool) == Some(true)
                && event.get("native_port").and_then(Value::as_u64).is_some()
        })
        .await
        .get("native_port")
        .and_then(Value::as_u64)
        .unwrap() as u16;

        let send = tokio::spawn(async move {
            native::send_files(
                SocketAddr::from(([127, 0, 0, 1], port)),
                "Other",
                &[file],
                None,
                Trust::Fingerprint(None),
            )
            .await
        });
        let offer = wait_for(&events, |event| {
            event.get("ev").and_then(Value::as_str) == Some("offer")
        })
        .await;
        let offer_id = offer.get("id").and_then(Value::as_str).unwrap().to_string();
        tx.send(Incoming::Command(
            serde_json::from_value(json!({
                "id": "3",
                "op": "decide",
                "offer": offer_id,
                "accept": true,
            }))
            .unwrap(),
        ))
        .unwrap();
        let report = tokio::time::timeout(Duration::from_secs(20), send)
            .await
            .expect("send timed out")
            .unwrap()
            .expect("send failed");
        let file_len = b"hello from whoosh".len() as u64;
        assert_eq!(report.bytes, file_len);
        let progress = events
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .iter()
            .filter(|event| {
                event.get("ev").and_then(Value::as_str) == Some("activity")
                    && event.get("direction").and_then(Value::as_str) == Some("in")
                    && event.get("state").and_then(Value::as_str) == Some("working")
                    && event.get("total").and_then(Value::as_u64) == Some(file_len)
            })
            .filter_map(|event| event.get("bytes").and_then(Value::as_u64))
            .max();
        assert_eq!(progress, Some(file_len));
        let saved_event = wait_for(&events, |event| {
            event.get("ev").and_then(Value::as_str) == Some("activity")
                && event.get("direction").and_then(Value::as_str) == Some("in")
                && event.get("state").and_then(Value::as_str) == Some("done")
        })
        .await;
        assert_eq!(
            saved_event.get("bytes").and_then(Value::as_u64),
            Some(file_len)
        );
        assert_eq!(
            saved_event.get("id").and_then(Value::as_str),
            Some(offer_id.as_str())
        );
        let saved = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if std::fs::read(inbox.path().join("note.txt")).ok().as_deref()
                    == Some(b"hello from whoosh".as_slice())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(saved.is_ok(), "file was not saved");
        tx.send(Incoming::Command(
            serde_json::from_value(json!({"id": "4", "op": "shutdown"})).unwrap(),
        ))
        .unwrap();
        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .expect("shutdown timed out")
            .unwrap()
            .unwrap();
    }

    async fn wait_for(events: &Arc<Mutex<Vec<Value>>>, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(found) = events
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .iter()
                .find(|event| pred(event))
                .cloned()
            {
                return found;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!(
                    "timed out waiting for an event: {:?}",
                    events.lock().unwrap_or_else(|poison| poison.into_inner())
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}
