use std::net::SocketAddr;
use std::path::PathBuf;

use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::airdrop::{
    airdrop_visibility_line, macos_computer_name, server_acceptor, AirdropConfig, AirdropReceiver,
    ALT_SERVICE_TYPE, MDNS_FLAGS, SERVICE_TYPE as AIRDROP_SERVICE,
};
use crate::approve::{approve_all, Approval};
use crate::discover::Advertisement;
use crate::error::Result;
use crate::native::{
    fingerprint_hex, NativeListener, ReceiveOptions, SERVICE_TYPE as NATIVE_SERVICE,
};
use crate::net::{awdl_listeners, visibility_report};
use crate::quickshare::{
    random_endpoint_id, serve as serve_quickshare, service_instance_name, EndpointInfo,
    QuickshareConfig, DEVICE_LAPTOP, SERVICE_TYPE as QUICKSHARE_SERVICE,
};
use crate::radio::Radio;

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
    println!("{}", visibility_report());
    let _radio = (config.quickshare || config.airdrop).then(|| Radio::start(config.name.clone()));

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
        let fingerprint = fingerprint_hex(&listener.fingerprint);
        if let Some(advert) = advertise(
            "native",
            NATIVE_SERVICE,
            &instance,
            listener.local_addr.port(),
            vec![
                ("n".into(), config.name.clone()),
                ("v".into(), "1".into()),
                ("fp".into(), fingerprint),
            ],
        )
        .await
        {
            println!("native     {}  ({instance})", advert.detail());
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
        if let Some(advert) = advertise(
            "quickshare",
            QUICKSHARE_SERVICE,
            &instance,
            port,
            vec![("n".into(), txt_name)],
        )
        .await
        {
            println!(
                "quickshare {}  name \"{}\"  ({instance})",
                advert.detail(),
                config.name
            );
            println!(
                "quickshare open Quick Share, set it to Everyone, and use this Wi-Fi. The phone lists this Mac only while that sheet is open."
            );
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
        let computer = macos_computer_name().unwrap_or_else(|| "this Mac".into());
        println!(
            "airdrop    {}",
            airdrop_visibility_line(&config.name, &computer)
        );
        let (acceptor, _) = server_acceptor()?;
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
        let awdl = awdl_listeners(port);
        if awdl.is_empty() && cfg!(target_os = "macos") {
            println!("airdrop    awdl0 has no IPv6 address, so an iPhone cannot connect");
        }
        for addr in awdl {
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
                Err(error) => {
                    println!("airdrop    {addr}  awdl not bound: {error}");
                }
            }
        }
        let instance = hex::encode(&pin_bytes());
        let flags = vec![("flags".into(), MDNS_FLAGS.into())];
        if let Some(advert) = advertise(
            "airdrop",
            AIRDROP_SERVICE,
            &instance[..12],
            port,
            flags.clone(),
        )
        .await
        {
            println!(
                "airdrop    {}  legacy {}  ({})",
                advert.detail(),
                config.name,
                &instance[..12]
            );
            adverts.push(advert);
        }
        let alt_name = airdrop_instance_name(&config.name);
        if let Some(advert) = advertise("airdrop", ALT_SERVICE_TYPE, &alt_name, port, flags).await {
            println!("airdrop    {}  alt \"{alt_name}\"", advert.detail());
            adverts.push(advert);
        }
    }

    println!("phones list this Mac only while their share sheet is open.");
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

async fn advertise(
    label: &str,
    service_type: &str,
    instance: &str,
    port: u16,
    txt: Vec<(String, String)>,
) -> Option<Advertisement> {
    let service_type = service_type.to_string();
    let instance = instance.to_string();
    let label = label.to_string();
    let joined = tokio::task::spawn_blocking(move || {
        let pairs: Vec<(&str, &str)> = txt
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        Advertisement::start(&service_type, &instance, port, &pairs)
    })
    .await;
    match joined {
        Ok(Ok(advert)) => Some(advert),
        Ok(Err(error)) => {
            println!("{label} is not visible on the network: {error}");
            None
        }
        Err(error) => {
            println!("{label} is not visible on the network: {error}");
            None
        }
    }
}

fn airdrop_instance_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if out.len() >= 63 {
            break;
        }
        if ch.is_control() {
            continue;
        }
        let next = ch.len_utf8();
        if out.len() + next > 63 {
            break;
        }
        out.push(ch);
    }
    if out.is_empty() {
        "Whoosh".into()
    } else {
        out
    }
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
