use std::net::SocketAddr;
use std::path::PathBuf;

use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::airdrop::{
    server_acceptor, AirdropConfig, AirdropReceiver, MDNS_FLAGS, SERVICE_TYPE as AIRDROP_SERVICE,
};
use crate::approve::{approve_all, Approval};
use crate::discover::Advertisement;
use crate::error::Result;
use crate::native::{
    fingerprint_hex, NativeListener, ReceiveOptions, SERVICE_TYPE as NATIVE_SERVICE,
};
use crate::net::awdl_listeners;
use crate::quickshare::{
    random_endpoint_id, serve as serve_quickshare, service_instance_name, EndpointInfo,
    QuickshareConfig, DEVICE_LAPTOP, SERVICE_TYPE as QUICKSHARE_SERVICE,
};

pub struct DaemonConfig {
    pub name: String,
    pub dir: PathBuf,
    pub native: bool,
    pub quickshare: bool,
    pub airdrop: bool,
    pub require_pin: bool,
    pub sort_media: bool,
    pub max_file_bytes: u64,
    pub approve: Approval,
}

impl DaemonConfig {
    pub fn receive(name: String, dir: PathBuf, yes: bool) -> Self {
        Self {
            name,
            dir,
            native: true,
            quickshare: true,
            airdrop: true,
            require_pin: false,
            sort_media: false,
            max_file_bytes: 32 * 1024 * 1024 * 1024,
            approve: if yes {
                approve_all()
            } else {
                interactive_approval()
            },
        }
    }
}

pub async fn run(config: DaemonConfig) -> Result<()> {
    tokio::fs::create_dir_all(&config.dir).await?;
    let cancel = CancellationToken::new();
    let shutdown = cancel.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        shutdown.cancel();
    });

    let pin = config.require_pin.then(random_pin);
    if let Some(pin) = &pin {
        println!("pin {pin}");
    }
    println!("saving files to {}", config.dir.display());

    let mut tasks = Vec::new();
    let mut adverts = Vec::new();

    if config.native {
        let listener = NativeListener::bind(
            SocketAddr::from(([0, 0, 0, 0], 0)),
            config.name.clone(),
            pin.clone(),
        )?;
        println!(
            "native     {}   fingerprint {}",
            listener.local_addr,
            fingerprint_hex(&listener.fingerprint)
        );
        let instance = hex::encode(&listener.fingerprint[..8]);
        if let Ok(advert) = Advertisement::start(
            NATIVE_SERVICE,
            &instance,
            listener.local_addr.port(),
            &[
                ("n", config.name.as_str()),
                ("v", "1"),
                ("fp", &fingerprint_hex(&listener.fingerprint)),
            ],
        ) {
            adverts.push(advert);
        }
        let options = ReceiveOptions {
            dir: config.dir.clone(),
            sort_media: config.sort_media,
            max_file_bytes: config.max_file_bytes,
            approve: config.approve.clone(),
        };
        let cancel = cancel.clone();
        tasks.push(tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = listener.receive_one(options.clone()) => match result {
                        Ok(report) => println!(
                            "received {} file(s), {} bytes, from {}",
                            report.files.len(),
                            report.bytes,
                            report.peer
                        ),
                        Err(error) => tracing::info!(%error, "native transfer did not complete"),
                    },
                    _ = cancel.cancelled() => break,
                }
            }
        }));
    }

    if config.quickshare {
        let listener = TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], 0))).await?;
        let port = listener.local_addr()?.port();
        println!("quickshare 0.0.0.0:{port}");
        let endpoint_id = random_endpoint_id();
        let info = EndpointInfo::visible(&config.name, DEVICE_LAPTOP);
        let instance = service_instance_name(&endpoint_id)?;
        let txt_name = info.encode_txt()?;
        if let Ok(advert) = Advertisement::start(
            QUICKSHARE_SERVICE,
            &instance,
            port,
            &[("n", txt_name.as_str())],
        ) {
            adverts.push(advert);
        }
        let quick = QuickshareConfig {
            dir: config.dir.clone(),
            name: config.name.clone(),
            sort_media: config.sort_media,
            max_file_bytes: config.max_file_bytes,
            approve: config.approve.clone(),
            device_type: DEVICE_LAPTOP,
        };
        let cancel = cancel.clone();
        tasks.push(tokio::spawn(async move {
            let _ = serve_quickshare(listener, quick, cancel).await;
        }));
    }

    if config.airdrop {
        let listener = TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], 0))).await?;
        let port = listener.local_addr()?.port();
        println!("airdrop    0.0.0.0:{port}  https");
        let (acceptor, _) = server_acceptor()?;
        let instance = hex::encode(&pin_bytes());
        if let Ok(advert) = Advertisement::start(
            AIRDROP_SERVICE,
            &instance[..12],
            port,
            &[("flags", MDNS_FLAGS)],
        ) {
            adverts.push(advert);
        }
        let airdrop = AirdropReceiver::new(AirdropConfig {
            dir: config.dir.clone(),
            name: config.name.clone(),
            model: "Whoosh".into(),
            sort_media: config.sort_media,
            max_file_bytes: config.max_file_bytes,
            approve: config.approve.clone(),
        });
        let cancel_air = cancel.clone();
        let primary = airdrop.clone();
        let primary_acceptor = acceptor.clone();
        tasks.push(tokio::spawn(async move {
            let _ = primary
                .serve_tls(listener, primary_acceptor, cancel_air)
                .await;
        }));
        for addr in awdl_listeners(port) {
            match TcpListener::bind(addr).await {
                Ok(extra) => {
                    println!("airdrop    {addr}  awdl");
                    let receiver = airdrop.clone();
                    let acceptor = acceptor.clone();
                    let cancel = cancel.clone();
                    tasks.push(tokio::spawn(async move {
                        let _ = receiver.serve_tls(extra, acceptor, cancel).await;
                    }));
                }
                Err(error) => tracing::debug!(%error, %addr, "awdl listener was not bound"),
            }
        }
    }

    println!("waiting for files. press ctrl-c to stop.");
    cancel.cancelled().await;
    for advert in adverts {
        advert.shutdown();
    }
    for task in tasks {
        task.abort();
    }
    Ok(())
}

pub fn interactive_approval() -> Approval {
    use crate::approve::{approval, TransferOffer};
    use crate::mime::human_size;
    approval(|offer: TransferOffer| async move {
        println!();
        println!("{} from {}", offer.protocol, offer.peer);
        if let Some(pin) = &offer.pin {
            println!("  pin {pin}");
        }
        for file in &offer.files {
            println!("  {}  {}  {}", file.name, human_size(file.bytes), file.mime);
        }
        println!("accept? [y/N]");
        let line = tokio::task::spawn_blocking(|| {
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            line
        })
        .await
        .unwrap_or_default();
        matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
    })
}

fn random_pin() -> String {
    use rand::Rng;
    format!("{:04}", rand::thread_rng().gen_range(0..10_000))
}

fn pin_bytes() -> [u8; 8] {
    let mut bytes = [0u8; 8];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut bytes);
    bytes
}
