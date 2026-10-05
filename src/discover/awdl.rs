//! AirDrop browse through `dns-sd -includeAWDL`.
//!
//! An iPhone announces `_airdrop._tcp` on AWDL, and only after a sender asks.
//! Apple's Sharing framework does that ask. Its callbacks arrive on the
//! process main queue, so the browser has to be started there. A userspace
//! mDNS socket never receives the answer, and `dns-sd` fully buffers its
//! browse output unless stdout is a terminal, so the browser is started on a
//! pseudo-terminal.

#![allow(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::net::{IpAddr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{dns_sd_type, FoundPeer};
use crate::error::{Error, Result};

pub fn uses_system_browse(service_type: &str) -> bool {
    cfg!(target_os = "macos") && service_type.contains("_airdrop")
}

pub fn unnamed_instance(instance: &str) -> bool {
    let instance = instance.trim();
    instance.is_empty()
        || (instance.len() >= 8 && instance.chars().all(|ch| ch.is_ascii_hexdigit()))
}

pub fn parse_peer_addr(value: &str) -> Option<SocketAddr> {
    if let Ok(addr) = value.parse() {
        return Some(addr);
    }
    parse_scoped_addr(value)
}

/// `[fe80::1%16]:8770` or `[fe80::1%awdl0]:8770`.
pub fn parse_scoped_addr(value: &str) -> Option<SocketAddr> {
    let rest = value.trim().strip_prefix('[')?;
    let (ip_part, after) = rest.split_once(']')?;
    let port: u16 = after.strip_prefix(':')?.parse().ok()?;
    let (ip_text, zone) = ip_part.split_once('%')?;
    let ip: Ipv6Addr = ip_text.parse().ok()?;
    let scope = zone_index(zone)?;
    Some(SocketAddr::V6(SocketAddrV6::new(ip, port, 0, scope)))
}

pub fn interrupt(pid: u32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
    #[cfg(not(unix))]
    let _ = pid;
}

pub struct SystemBrowse {
    pub pid: u32,
    pub task: tokio::task::JoinHandle<()>,
}

#[cfg(not(target_os = "macos"))]
pub fn start_browse(
    service_type: &str,
    via: &'static str,
    tx: tokio::sync::mpsc::UnboundedSender<super::PeerUpdate>,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<SystemBrowse> {
    let _ = (service_type, via, tx, cancel);
    Err(Error::protocol(
        "AirDrop AWDL browse is only available on macOS",
    ))
}

#[cfg(not(target_os = "macos"))]
pub fn collect(_service_type: &str, _wait: Duration) -> Vec<FoundPeer> {
    Vec::new()
}

#[cfg(target_os = "macos")]
pub fn start_browse(
    service_type: &str,
    via: &'static str,
    tx: tokio::sync::mpsc::UnboundedSender<super::PeerUpdate>,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<SystemBrowse> {
    let reg_type = dns_sd_type(service_type)?;
    let mut session = PtySession::spawn(&[
        "-includeAWDL".to_string(),
        "-B".to_string(),
        reg_type.clone(),
        "local.".to_string(),
    ])?;
    let pid = session.child.id();
    let (line_tx, mut lines) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let reader = BufReader::new(session.master);
        for line in reader.lines() {
            let Ok(line) = line else {
                break;
            };
            if line_tx.send(line).is_err() {
                break;
            }
        }
        let _ = session.child.kill();
        let _ = session.child.wait();
    });
    let wake = WakeGuard::hold();
    let task = tokio::spawn(async move {
        let _wake = wake;
        let state = Arc::new(Mutex::new(HashMap::<String, Slot>::new()));
        while let Some(line) = lines.recv().await {
            let Some(event) = parse_browse_line(&line) else {
                continue;
            };
            if !event.service.contains("_airdrop") {
                continue;
            }
            match event.kind {
                BrowseKind::Add => {
                    let start = {
                        let mut guard = state.lock().unwrap_or_else(|poison| poison.into_inner());
                        let slot = guard.entry(event.instance.clone()).or_default();
                        slot.interfaces.insert(event.interface);
                        if slot.running {
                            slot.again = true;
                            false
                        } else {
                            slot.running = true;
                            true
                        }
                    };
                    if start {
                        let state = Arc::clone(&state);
                        let instance = event.instance;
                        let reg_type = reg_type.clone();
                        let tx = tx.clone();
                        let cancel = cancel.clone();
                        tokio::spawn(async move {
                            resolve_until_settled(state, instance, reg_type, via, tx, cancel).await;
                        });
                    }
                }
                BrowseKind::Remove => {
                    let gone = {
                        let mut guard = state.lock().unwrap_or_else(|poison| poison.into_inner());
                        let Some(slot) = guard.get_mut(&event.instance) else {
                            continue;
                        };
                        slot.interfaces.remove(&event.interface);
                        if !slot.interfaces.is_empty() {
                            false
                        } else {
                            guard.remove(&event.instance);
                            true
                        }
                    };
                    if gone {
                        let _ = tx.send(super::PeerUpdate::Removed {
                            fullname: full_name(&event.instance, &reg_type),
                        });
                    }
                }
            }
        }
    });
    Ok(SystemBrowse { pid, task })
}

#[cfg(target_os = "macos")]
async fn resolve_until_settled(
    state: Arc<Mutex<HashMap<String, Slot>>>,
    instance: String,
    reg_type: String,
    via: &'static str,
    tx: tokio::sync::mpsc::UnboundedSender<super::PeerUpdate>,
    cancel: tokio_util::sync::CancellationToken,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(Duration::from_millis(350)) => {}
        }
        let present = {
            let guard = state.lock().unwrap_or_else(|poison| poison.into_inner());
            guard
                .get(&instance)
                .is_some_and(|slot| !slot.interfaces.is_empty())
        };
        if !present {
            return;
        }
        let instance_for_resolve = instance.clone();
        let reg_for_resolve = reg_type.clone();
        let resolved = tokio::task::spawn_blocking(move || {
            resolve_peer(&reg_for_resolve, &instance_for_resolve)
        })
        .await
        .ok()
        .flatten();
        if cancel.is_cancelled() {
            return;
        }
        if let Some(mut peer) = resolved {
            let still = {
                let guard = state.lock().unwrap_or_else(|poison| poison.into_inner());
                guard
                    .get(&instance)
                    .is_some_and(|slot| !slot.interfaces.is_empty())
            };
            if !still {
                return;
            }
            tracing::info!(instance = %peer.instance, addr = %peer.addr, "found an AirDrop device");
            let addr = peer.addr;
            let unnamed = unnamed_instance(&peer.instance);
            let _ = tx.send(super::PeerUpdate::Resolved {
                via,
                peer: peer.clone(),
            });
            if unnamed {
                if let Some(name) = crate::airdrop::discover_receiver_name(addr).await {
                    tracing::info!(instance = %peer.instance, %name, "AirDrop device name");
                    peer.txt.insert("name".into(), name);
                    let _ = tx.send(super::PeerUpdate::Resolved { via, peer });
                }
            }
        }
        let mut guard = state.lock().unwrap_or_else(|poison| poison.into_inner());
        let Some(slot) = guard.get_mut(&instance) else {
            return;
        };
        if slot.interfaces.is_empty() {
            return;
        }
        if slot.again {
            slot.again = false;
            continue;
        }
        slot.running = false;
        return;
    }
}

#[cfg(target_os = "macos")]
pub fn collect(service_type: &str, wait: Duration) -> Vec<FoundPeer> {
    let _wake = WakeGuard::hold();
    let Ok(reg_type) = dns_sd_type(service_type) else {
        return Vec::new();
    };
    let Ok(mut session) = PtySession::spawn(&[
        "-includeAWDL".to_string(),
        "-B".to_string(),
        reg_type.clone(),
        "local.".to_string(),
    ]) else {
        tracing::warn!("dns-sd browse did not start");
        return Vec::new();
    };
    let lines = read_available(&mut session, wait, |_| false);
    let _ = session.child.kill();
    let _ = session.child.wait();
    let mut instances = Vec::new();
    for line in &lines {
        let Some(event) = parse_browse_line(line) else {
            continue;
        };
        if event.kind == BrowseKind::Add && !instances.iter().any(|have| have == &event.instance) {
            instances.push(event.instance);
        }
    }
    instances
        .into_iter()
        .filter_map(|instance| resolve_peer(&reg_type, &instance))
        .collect()
}

#[cfg(target_os = "macos")]
fn resolve_peer(reg_type: &str, instance: &str) -> Option<FoundPeer> {
    let awdl = awdl_index();
    let mut lookup = PtySession::spawn(&[
        "-includeAWDL".to_string(),
        "-L".to_string(),
        instance.to_string(),
        reg_type.to_string(),
        "local.".to_string(),
    ])
    .ok()?;
    let lookup_lines = read_available(&mut lookup, Duration::from_secs(3), |lines| {
        reachable(lines).is_some_and(|item| item.interface == Some(awdl) && awdl != 0)
            && lines.iter().any(|line| line.contains('='))
    });
    let _ = lookup.child.kill();
    let _ = lookup.child.wait();
    let found = reachable(&lookup_lines)?;
    let txt = txt_records(&lookup_lines);
    let mut query = PtySession::spawn(&[
        "-includeAWDL".to_string(),
        "-G".to_string(),
        "v4v6".to_string(),
        found.host.clone(),
    ])
    .ok()?;
    let address_lines = read_available(&mut query, Duration::from_secs(2), |lines| {
        host_addresses(lines)
            .iter()
            .any(|addr| addr.interface == awdl && awdl != 0)
    });
    let _ = query.child.kill();
    let _ = query.child.wait();
    let chosen = pick_host_addr(&host_addresses(&address_lines), awdl)?;
    let addr = socket_addr(chosen, found.port);
    tracing::debug!(instance, %addr, host = %found.host, "resolved AirDrop");
    Some(FoundPeer {
        instance: instance.to_string(),
        fullname: full_name(instance, reg_type),
        addr,
        txt,
        txt_raw: HashMap::new(),
        hostname: found.host,
    })
}

#[derive(Default)]
struct Slot {
    interfaces: HashSet<u32>,
    running: bool,
    again: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BrowseKind {
    Add,
    Remove,
}

struct BrowseEvent {
    kind: BrowseKind,
    interface: u32,
    service: String,
    instance: String,
}

#[derive(Clone)]
struct Reachable {
    host: String,
    port: u16,
    interface: Option<u32>,
}

struct HostAddress {
    interface: u32,
    ip: IpAddr,
    zone: Option<String>,
}

fn parse_browse_line(line: &str) -> Option<BrowseEvent> {
    let line = line.trim().trim_end_matches('\r');
    let (kind, after) = if let Some((_, after)) = line.split_once(" Add ") {
        (BrowseKind::Add, after)
    } else if let Some((_, after)) = line.split_once(" Rmv ") {
        (BrowseKind::Remove, after)
    } else {
        return None;
    };
    let mut parts = after.split_whitespace();
    let _flags = parts.next()?;
    let interface: u32 = parts.next()?.parse().ok()?;
    let domain = parts.next()?;
    if !domain.trim_end_matches('.').eq_ignore_ascii_case("local") {
        return None;
    }
    let service = parts.next()?.trim_end_matches('.').to_string();
    let service_at = after.find(service.as_str())?;
    let instance = after[service_at + service.len()..]
        .trim()
        .trim_start_matches('.')
        .trim()
        .to_string();
    if instance.is_empty() {
        return None;
    }
    Some(BrowseEvent {
        kind,
        interface,
        service,
        instance,
    })
}

fn reachable(lines: &[String]) -> Option<Reachable> {
    let mut found = Vec::new();
    for line in lines {
        if let Some(item) = parse_reachable(line) {
            found.push(item);
        }
    }
    let awdl = awdl_index();
    found
        .iter()
        .find(|item| item.interface == Some(awdl) && awdl != 0)
        .or_else(|| found.first())
        .cloned()
}

fn parse_reachable(line: &str) -> Option<Reachable> {
    let rest = line.split_once("can be reached at ")?.1.trim();
    let (target, extra) = rest.split_once(' ').unwrap_or((rest, ""));
    let (host, port) = target.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let host = host.trim().trim_end_matches('.').to_string();
    if host.is_empty() {
        return None;
    }
    Some(Reachable {
        host,
        port,
        interface: interface_after(extra),
    })
}

fn interface_after(extra: &str) -> Option<u32> {
    let after = extra.split("interface").nth(1)?;
    let digits: String = after
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

fn txt_records(lines: &[String]) -> HashMap<String, String> {
    let mut txt = HashMap::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.contains("can be reached") || !trimmed.contains('=') {
            continue;
        }
        for token in trimmed.split_whitespace() {
            let Some((key, value)) = token.split_once('=') else {
                continue;
            };
            if key.is_empty() || !key.chars().all(|ch| ch.is_ascii_alphanumeric()) {
                continue;
            }
            txt.insert(key.to_ascii_lowercase(), value.to_string());
        }
    }
    txt
}

fn host_addresses(lines: &[String]) -> Vec<HostAddress> {
    lines
        .iter()
        .filter_map(|line| parse_host_addr(line))
        .collect()
}

fn parse_host_addr(line: &str) -> Option<HostAddress> {
    let after = line.split_once(" Add ")?.1;
    let mut parts = after.split_whitespace();
    let _flags = parts.next()?;
    let interface: u32 = parts.next()?.parse().ok()?;
    let host = parts.next()?;
    if !host.contains('.') && host != "Hostname" {
        return None;
    }
    if host.eq_ignore_ascii_case("hostname") {
        return None;
    }
    let token = parts.next()?;
    let (ip_text, zone) = match token.split_once('%') {
        Some((ip, zone)) => (ip, Some(zone.to_string())),
        None => (token, None),
    };
    let ip: IpAddr = ip_text.parse().ok()?;
    if !usable_ip(ip) {
        return None;
    }
    Some(HostAddress {
        interface,
        ip,
        zone,
    })
}

fn pick_host_addr(addrs: &[HostAddress], awdl: u32) -> Option<HostAddress> {
    if awdl != 0 {
        if let Some(addr) = addrs.iter().find(|addr| addr.interface == awdl) {
            return Some(clone_addr(addr));
        }
    }
    if let Some(addr) = addrs
        .iter()
        .find(|addr| addr.zone.as_deref() == Some("awdl0"))
    {
        return Some(clone_addr(addr));
    }
    addrs
        .iter()
        .min_by_key(|addr| super::addr_rank(addr.ip))
        .map(clone_addr)
}

fn clone_addr(addr: &HostAddress) -> HostAddress {
    HostAddress {
        interface: addr.interface,
        ip: addr.ip,
        zone: addr.zone.clone(),
    }
}

fn socket_addr(addr: HostAddress, port: u16) -> SocketAddr {
    match addr.ip {
        IpAddr::V4(ip) => SocketAddr::V4(SocketAddrV4::new(ip, port)),
        IpAddr::V6(ip) => {
            let scope = if ip.is_unicast_link_local() {
                if addr.interface != 0 {
                    addr.interface
                } else {
                    addr.zone.as_deref().and_then(zone_index).unwrap_or(0)
                }
            } else {
                0
            };
            SocketAddr::V6(SocketAddrV6::new(ip, port, 0, scope))
        }
    }
}

fn usable_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => !ip.is_unspecified() && !ip.is_loopback() && !ip.is_multicast(),
        IpAddr::V6(ip) => !ip.is_unspecified() && !ip.is_loopback() && !ip.is_multicast(),
    }
}

fn full_name(instance: &str, reg_type: &str) -> String {
    format!("{instance}.{reg_type}.local.")
}

fn awdl_index() -> u32 {
    zone_index("awdl0").unwrap_or(0)
}

fn zone_index(zone: &str) -> Option<u32> {
    let zone = zone.trim_matches(|ch: char| ch == '<' || ch == '>');
    if zone.is_empty() || zone == "0" {
        return None;
    }
    if let Ok(index) = zone.parse::<u32>() {
        return Some(index);
    }
    #[cfg(unix)]
    {
        let name = std::ffi::CString::new(zone).ok()?;
        let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
        if index == 0 {
            None
        } else {
            Some(index)
        }
    }
    #[cfg(not(unix))]
    {
        let _ = zone;
        None
    }
}

#[cfg(target_os = "macos")]
use std::os::unix::io::{AsRawFd, FromRawFd};
#[cfg(target_os = "macos")]
use std::os::unix::process::CommandExt;

#[cfg(target_os = "macos")]
struct PtySession {
    child: crate::children::TrackedChild,
    master: std::fs::File,
}

#[cfg(target_os = "macos")]
impl PtySession {
    fn spawn(args: &[String]) -> Result<Self> {
        let mut master_fd: libc::c_int = -1;
        let mut slave_fd: libc::c_int = -1;
        let opened = unsafe {
            libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if opened != 0 {
            return Err(Error::protocol("could not open a terminal for dns-sd"));
        }
        let _ = quiet_echo(slave_fd);
        let mut command = Command::new("dns-sd");
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            command.pre_exec(move || {
                libc::close(master_fd);
                let _ = libc::setsid();
                let _ = libc::ioctl(slave_fd, libc::TIOCSCTTY as libc::c_ulong, 0);
                if libc::dup2(slave_fd, 0) < 0
                    || libc::dup2(slave_fd, 1) < 0
                    || libc::dup2(slave_fd, 2) < 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                if slave_fd > 2 {
                    libc::close(slave_fd);
                }
                Ok(())
            });
        }
        let child = match command.spawn() {
            Ok(child) => crate::children::TrackedChild::new(child),
            Err(error) => {
                unsafe {
                    libc::close(master_fd);
                    libc::close(slave_fd);
                }
                if error.kind() == std::io::ErrorKind::NotFound {
                    return Err(Error::protocol("dns-sd is not installed"));
                }
                return Err(error.into());
            }
        };
        unsafe { libc::close(slave_fd) };
        let master = unsafe { std::fs::File::from_raw_fd(master_fd) };
        Ok(Self { child, master })
    }
}

#[cfg(target_os = "macos")]
fn quiet_echo(fd: libc::c_int) -> std::io::Result<()> {
    let mut term: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut term) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    term.c_lflag &= !libc::ECHO;
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &term) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn read_available(
    session: &mut PtySession,
    timeout: Duration,
    done: impl Fn(&[String]) -> bool,
) -> Vec<String> {
    let fd = session.master.as_raw_fd();
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
    let mut pending = Vec::new();
    let mut lines = Vec::new();
    let mut buf = [0u8; 2048];
    let started = Instant::now();
    while started.elapsed() < timeout {
        match session.master.read(&mut buf) {
            Ok(0) => break,
            Ok(size) => {
                pending.extend_from_slice(&buf[..size]);
                while let Some(pos) = pending.iter().position(|byte| *byte == b'\n') {
                    let raw: Vec<u8> = pending.drain(..=pos).collect();
                    if let Ok(text) = String::from_utf8(raw) {
                        lines.push(text.trim_end_matches(['\r', '\n']).to_string());
                    }
                }
                if done(&lines) {
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if session.child.try_wait().ok().flatten().is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(30));
            }
            Err(_) => break,
        }
    }
    lines
}

/// Keeps Apple's AirDrop browser running so a nearby iPhone publishes
/// `_airdrop._tcp`. Dropped when the last browse stops.
#[cfg(target_os = "macos")]
pub(super) struct WakeGuard;

#[cfg(target_os = "macos")]
impl WakeGuard {
    pub(super) fn hold() -> Self {
        hold_system_browser();
        Self
    }
}

#[cfg(target_os = "macos")]
impl Drop for WakeGuard {
    fn drop(&mut self) {
        release_system_browser();
    }
}

#[cfg(target_os = "macos")]
fn users_lock() -> std::sync::MutexGuard<'static, usize> {
    static USERS: Mutex<usize> = Mutex::new(0);
    USERS.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Objective-C objects in here are created and released on the main thread.
#[cfg(target_os = "macos")]
struct MainBrowser(Option<AppleBrowser>);

#[cfg(target_os = "macos")]
unsafe impl Send for MainBrowser {}

#[cfg(target_os = "macos")]
fn browser_slot() -> std::sync::MutexGuard<'static, MainBrowser> {
    static SLOT: Mutex<MainBrowser> = Mutex::new(MainBrowser(None));
    SLOT.lock().unwrap_or_else(|poison| poison.into_inner())
}

#[cfg(target_os = "macos")]
fn main_is_pumping() -> &'static std::sync::atomic::AtomicBool {
    static PUMP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    &PUMP
}

#[cfg(target_os = "macos")]
fn hold_system_browser() {
    *users_lock() += 1;
    schedule_browser_sync();
}

#[cfg(target_os = "macos")]
fn release_system_browser() {
    let mut users = users_lock();
    *users = users.saturating_sub(1);
    drop(users);
    schedule_browser_sync();
}

#[cfg(target_os = "macos")]
fn schedule_browser_sync() {
    unsafe {
        if pthread_main_np() != 0 {
            sync_browser();
            return;
        }
        // The browser is opened between run-loop slices, not inside one.
        // Unit tests never pump, so a queued block would never run.
        if !main_is_pumping().load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        CFRunLoopStop(CFRunLoopGetMain());
    }
}

#[cfg(target_os = "macos")]
fn sync_browser() {
    let wanted = *users_lock() > 0;
    let mut slot = browser_slot();
    if wanted && slot.0.is_none() {
        slot.0 = unsafe { AppleBrowser::open() };
        if slot.0.is_some() {
            tracing::info!("asking nearby iPhones to announce AirDrop");
        }
    } else if !wanted {
        if let Some(browser) = slot.0.take() {
            unsafe { browser.close() };
        }
    }
}

/// Runs the main-thread run loop so AirDrop discovery callbacks can fire.
///
/// No effect off the main thread. The process entry point calls this.
#[cfg(target_os = "macos")]
pub(crate) fn pump_main_run_loop(seconds: f64) {
    unsafe {
        if pthread_main_np() == 0 {
            return;
        }
        ensure_outer_pool();
        main_is_pumping().store(true, std::sync::atomic::Ordering::SeqCst);
        sync_browser();
        // NSRunLoop, not CFRunLoopRunInMode. A bare process does not deliver
        // the main-queue callbacks this browser needs until NSRunLoop runs.
        if !run_cocoa_loop(seconds) {
            let _ = CFRunLoopRunInMode(kCFRunLoopDefaultMode, seconds, 0);
        }
    }
}

#[cfg(target_os = "macos")]
unsafe fn run_cocoa_loop(seconds: f64) -> bool {
    let run_class = objc_getClass(c"NSRunLoop".as_ptr());
    let date_class = objc_getClass(c"NSDate".as_ptr());
    if run_class.is_null() || date_class.is_null() {
        return false;
    }
    let current = sel_registerName(c"currentRunLoop".as_ptr());
    let run_loop = msg_id(run_class, current);
    let dated = sel_registerName(c"dateWithTimeIntervalSinceNow:".as_ptr());
    let date = msg_f64(date_class, dated, seconds);
    if run_loop.is_null() || date.is_null() {
        return false;
    }
    let run_mode = sel_registerName(c"runMode:beforeDate:".as_ptr());
    let _ = msg_mode(run_loop, run_mode, kCFRunLoopDefaultMode, date);
    true
}

#[cfg(target_os = "macos")]
unsafe fn msg_id(
    receiver: *mut std::ffi::c_void,
    selector: *mut std::ffi::c_void,
) -> *mut std::ffi::c_void {
    type Send =
        unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    let send: Send = std::mem::transmute(objc_msgSend as *const ());
    send(receiver, selector)
}

#[cfg(target_os = "macos")]
unsafe fn msg_f64(
    receiver: *mut std::ffi::c_void,
    selector: *mut std::ffi::c_void,
    value: f64,
) -> *mut std::ffi::c_void {
    type Send = unsafe extern "C" fn(
        *mut std::ffi::c_void,
        *mut std::ffi::c_void,
        f64,
    ) -> *mut std::ffi::c_void;
    let send: Send = std::mem::transmute(objc_msgSend as *const ());
    send(receiver, selector, value)
}

#[cfg(target_os = "macos")]
unsafe fn msg_mode(
    receiver: *mut std::ffi::c_void,
    selector: *mut std::ffi::c_void,
    mode: *const std::ffi::c_void,
    date: *mut std::ffi::c_void,
) -> u8 {
    type Send = unsafe extern "C" fn(
        *mut std::ffi::c_void,
        *mut std::ffi::c_void,
        *const std::ffi::c_void,
        *mut std::ffi::c_void,
    ) -> u8;
    let send: Send = std::mem::transmute(objc_msgSend as *const ());
    send(receiver, selector, mode, date)
}

#[cfg(target_os = "macos")]
fn ensure_outer_pool() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        // Lives for the process, outside every run-loop slice. AirDrop's
        // browser is created into this pool, as it is in a Cocoa main.
        let _ = objc_autoreleasePoolPush();
    });
}

/// Interrupts [`pump_main_run_loop`] so the process can exit.
#[cfg(target_os = "macos")]
pub(crate) fn stop_main_run_loop() {
    unsafe {
        if pthread_main_np() != 0 {
            CFRunLoopStop(CFRunLoopGetMain());
            return;
        }
        // NSRunLoop ignores a bare CFRunLoopStop from another thread, so the
        // main thread can stay inside a two-second slice after shutdown.
        let queue = std::ptr::addr_of!(_dispatch_main_q).cast_mut().cast();
        dispatch_async_f(queue, std::ptr::null_mut(), stop_on_main);
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" fn stop_on_main(_context: *mut std::ffi::c_void) {
    CFRunLoopStop(CFRunLoopGetMain());
}

#[cfg(target_os = "macos")]
struct AppleBrowser {
    browser: *mut std::ffi::c_void,
    delegate: *mut std::ffi::c_void,
}

#[cfg(target_os = "macos")]
impl AppleBrowser {
    unsafe fn open() -> Option<Self> {
        let path =
            std::ffi::CString::new("/System/Library/PrivateFrameworks/Sharing.framework/Sharing")
                .ok()?;
        if libc::dlopen(path.as_ptr(), libc::RTLD_LAZY).is_null() {
            tracing::warn!("AirDrop discovery framework did not load");
            return None;
        }
        let browser_class = objc_getClass(c"SFAirDropBrowser".as_ptr());
        if browser_class.is_null() {
            tracing::warn!("AirDrop discovery framework has no browser");
            return None;
        }
        let browser = alloc(browser_class).and_then(|object| call0(object, c"init".as_ptr()));
        let class = delegate_class();
        let delegate = if class.is_null() {
            None
        } else {
            alloc(class).and_then(|object| call0(object, c"init".as_ptr()))
        };
        let (Some(browser), Some(delegate)) = (browser, delegate) else {
            if let Some(browser) = browser {
                objc_release(browser);
            }
            if let Some(delegate) = delegate {
                objc_release(delegate);
            }
            return None;
        };
        let _ = objc_retain(delegate);
        call1(browser, c"setDelegate:".as_ptr(), delegate);
        call_void(browser, c"start".as_ptr());
        Some(Self { browser, delegate })
    }

    unsafe fn close(self) {
        call_void(self.browser, c"stop".as_ptr());
        objc_release(self.delegate);
        objc_release(self.browser);
    }
}

#[cfg(target_os = "macos")]
fn delegate_class() -> *mut std::ffi::c_void {
    use std::sync::OnceLock;
    static CLASS: OnceLock<usize> = OnceLock::new();
    let ptr = *CLASS.get_or_init(|| unsafe { make_delegate_class() as usize });
    ptr as *mut std::ffi::c_void
}

#[cfg(target_os = "macos")]
unsafe fn make_delegate_class() -> *mut std::ffi::c_void {
    let name = c"WhooshAirDropWakeDelegate".as_ptr();
    let existing = objc_getClass(name);
    if !existing.is_null() {
        return existing;
    }
    let superclass = objc_getClass(c"NSObject".as_ptr());
    if superclass.is_null() {
        return std::ptr::null_mut();
    }
    let class = objc_allocateClassPair(superclass, name, 0);
    if class.is_null() {
        return std::ptr::null_mut();
    }
    let empty = note as *const () as *const std::ffi::c_void;
    class_addMethod(
        class,
        sel_registerName(c"browserWillChangePeople:".as_ptr()),
        empty,
        c"v@:@".as_ptr(),
    );
    class_addMethod(
        class,
        sel_registerName(c"browserDidChangePeople:".as_ptr()),
        empty,
        c"v@:@".as_ptr(),
    );
    class_replaceMethod(
        class,
        sel_registerName(c"methodSignatureForSelector:".as_ptr()),
        method_signature as *const () as *const std::ffi::c_void,
        c"@@::".as_ptr(),
    );
    class_replaceMethod(
        class,
        sel_registerName(c"forwardInvocation:".as_ptr()),
        forward_invocation as *const () as *const std::ffi::c_void,
        c"v@:@".as_ptr(),
    );
    objc_registerClassPair(class);
    class
}

#[cfg(target_os = "macos")]
unsafe extern "C" fn note(
    _this: *mut std::ffi::c_void,
    _cmd: *mut std::ffi::c_void,
    _browser: *mut std::ffi::c_void,
) {
}

#[cfg(target_os = "macos")]
unsafe extern "C" fn forward_invocation(
    _this: *mut std::ffi::c_void,
    _cmd: *mut std::ffi::c_void,
    _invocation: *mut std::ffi::c_void,
) {
}

#[cfg(target_os = "macos")]
unsafe extern "C" fn method_signature(
    _this: *mut std::ffi::c_void,
    _cmd: *mut std::ffi::c_void,
    _selector: *mut std::ffi::c_void,
) -> *mut std::ffi::c_void {
    let class = objc_getClass(c"NSMethodSignature".as_ptr());
    if class.is_null() {
        return std::ptr::null_mut();
    }
    let selector = sel_registerName(c"signatureWithObjCTypes:".as_ptr());
    let method = class_getClassMethod(class, selector);
    if method.is_null() {
        return std::ptr::null_mut();
    }
    type Signature = unsafe extern "C" fn(
        *mut std::ffi::c_void,
        *mut std::ffi::c_void,
        *const std::ffi::c_char,
    ) -> *mut std::ffi::c_void;
    let signature: Signature = std::mem::transmute(method_getImplementation(method));
    signature(class, selector, c"v@:@@@".as_ptr())
}

#[cfg(target_os = "macos")]
unsafe fn alloc(class: *mut std::ffi::c_void) -> Option<*mut std::ffi::c_void> {
    let selector = sel_registerName(c"alloc".as_ptr());
    let method = class_getClassMethod(class, selector);
    if method.is_null() {
        return None;
    }
    type Alloc =
        unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    let alloc: Alloc = std::mem::transmute(method_getImplementation(method));
    let object = alloc(class, selector);
    if object.is_null() {
        None
    } else {
        Some(object)
    }
}

#[cfg(target_os = "macos")]
unsafe fn call0(
    object: *mut std::ffi::c_void,
    name: *const std::ffi::c_char,
) -> Option<*mut std::ffi::c_void> {
    let selector = sel_registerName(name);
    let method = class_getInstanceMethod(object_getClass(object), selector);
    if method.is_null() {
        return None;
    }
    type Call =
        unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    let call: Call = std::mem::transmute(method_getImplementation(method));
    let result = call(object, selector);
    if result.is_null() {
        None
    } else {
        Some(result)
    }
}

#[cfg(target_os = "macos")]
unsafe fn call_void(object: *mut std::ffi::c_void, name: *const std::ffi::c_char) {
    let selector = sel_registerName(name);
    let method = class_getInstanceMethod(object_getClass(object), selector);
    if method.is_null() {
        return;
    }
    type Call = unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void);
    let call: Call = std::mem::transmute(method_getImplementation(method));
    call(object, selector);
}

#[cfg(target_os = "macos")]
unsafe fn call1(
    object: *mut std::ffi::c_void,
    name: *const std::ffi::c_char,
    argument: *mut std::ffi::c_void,
) {
    let selector = sel_registerName(name);
    let method = class_getInstanceMethod(object_getClass(object), selector);
    if method.is_null() {
        return;
    }
    type Call =
        unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void, *mut std::ffi::c_void);
    let call: Call = std::mem::transmute(method_getImplementation(method));
    call(object, selector, argument);
}

#[cfg(target_os = "macos")]
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopDefaultMode: *const std::ffi::c_void;
    fn CFRunLoopGetMain() -> *mut std::ffi::c_void;
    fn CFRunLoopRunInMode(mode: *const std::ffi::c_void, seconds: f64, return_after: u8) -> i32;
    fn CFRunLoopStop(run_loop: *mut std::ffi::c_void);
}

#[cfg(target_os = "macos")]
#[link(name = "System")]
extern "C" {
    fn pthread_main_np() -> i32;
    static _dispatch_main_q: u8;
    fn dispatch_async_f(
        queue: *mut std::ffi::c_void,
        context: *mut std::ffi::c_void,
        work: unsafe extern "C" fn(*mut std::ffi::c_void),
    );
}

#[cfg(target_os = "macos")]
#[link(name = "objc")]
extern "C" {
    fn objc_msgSend();
    fn objc_getClass(name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
    fn sel_registerName(name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
    fn object_getClass(object: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn class_getClassMethod(
        class: *mut std::ffi::c_void,
        selector: *mut std::ffi::c_void,
    ) -> *mut std::ffi::c_void;
    fn class_getInstanceMethod(
        class: *mut std::ffi::c_void,
        selector: *mut std::ffi::c_void,
    ) -> *mut std::ffi::c_void;
    fn method_getImplementation(method: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn objc_allocateClassPair(
        superclass: *mut std::ffi::c_void,
        name: *const std::ffi::c_char,
        extra_bytes: usize,
    ) -> *mut std::ffi::c_void;
    fn objc_registerClassPair(class: *mut std::ffi::c_void);
    fn class_addMethod(
        class: *mut std::ffi::c_void,
        selector: *mut std::ffi::c_void,
        implementation: *const std::ffi::c_void,
        types: *const std::ffi::c_char,
    ) -> u8;
    fn class_replaceMethod(
        class: *mut std::ffi::c_void,
        selector: *mut std::ffi::c_void,
        implementation: *const std::ffi::c_void,
        types: *const std::ffi::c_char,
    ) -> *mut std::ffi::c_void;
    fn objc_retain(object: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    fn objc_release(object: *mut std::ffi::c_void);
    fn objc_autoreleasePoolPush() -> *mut std::ffi::c_void;
}

#[cfg(test)]
mod tests {
    use super::{
        parse_browse_line, parse_host_addr, parse_peer_addr, parse_reachable, pick_host_addr,
        txt_records, unnamed_instance, BrowseKind, HostAddress,
    };
    use std::net::{Ipv6Addr, SocketAddr};

    #[test]
    fn browse_lines_keep_a_spaced_phone_name_and_the_interface() {
        let added = parse_browse_line(
            " 9:51:38.691  Add        2  16 local.               _airdrop._tcp.       Harry's iPhone",
        )
        .unwrap();
        assert_eq!(added.kind, BrowseKind::Add);
        assert_eq!(added.interface, 16);
        assert_eq!(added.service, "_airdrop._tcp");
        assert_eq!(added.instance, "Harry's iPhone");
        let removed = parse_browse_line(
            "10:00:00.001  Rmv        0  15 local.               _airdrop._tcp.       Harry's iPhone",
        )
        .unwrap();
        assert_eq!(removed.kind, BrowseKind::Remove);
        assert_eq!(removed.interface, 15);
        assert!(parse_browse_line("Timestamp     A/R    Flags  if Domain").is_none());
    }

    #[test]
    fn lookup_line_has_the_host_port_and_awdl_interface() {
        let line = " 9:51:39.788  ws0437._airdrop._tcp.local. can be reached at phone.local.:8770 (interface 16)";
        let found = parse_reachable(line).unwrap();
        assert_eq!(found.host, "phone.local");
        assert_eq!(found.port, 8770);
        assert_eq!(found.interface, Some(16));
        let txt = txt_records(&[
            line.to_string(),
            " flags=16524".to_string(),
            "Setting kDNSServiceFlagsIncludeAWDL".to_string(),
        ]);
        assert_eq!(txt.get("flags").map(String::as_str), Some("16524"));
    }

    #[test]
    fn address_query_prefers_the_awdl_interface() {
        let awdl = parse_host_addr(
            " 9:52:21.227  Add  40000003      16  Phone.local.             FE80:0000:0000:0000:C4AA:40FF:FE2F:DBEB%awdl0 4500",
        )
        .unwrap();
        let lan = parse_host_addr(
            " 9:52:21.227  Add  40000002      15  Phone.local.             192.168.1.10                                 4500",
        )
        .unwrap();
        assert!(parse_host_addr(
            " 9:52:21.227  Add  40000003       1  Phone.local.             127.0.0.1                                    4500"
        )
        .is_none());
        let chosen = pick_host_addr(&[lan, awdl], 16).unwrap();
        assert_eq!(chosen.interface, 16);
        assert!(chosen.ip.is_ipv6());
        let socket = super::socket_addr(chosen, 8770);
        match socket {
            SocketAddr::V6(addr) => {
                assert_eq!(addr.scope_id(), 16);
                assert_eq!(addr.port(), 8770);
            }
            SocketAddr::V4(_) => panic!("expected the awdl address"),
        }
    }

    #[test]
    fn scoped_address_round_trips() {
        let parsed = parse_peer_addr("[fe80::1%16]:8770").unwrap();
        match parsed {
            SocketAddr::V6(addr) => {
                assert_eq!(addr.scope_id(), 16);
                assert_eq!(*addr.ip(), Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1));
            }
            SocketAddr::V4(_) => panic!("expected ipv6"),
        }
        assert!(parse_peer_addr("192.168.1.10:8770").unwrap().is_ipv4());
    }

    #[test]
    fn hex_instances_are_unnamed() {
        assert!(unnamed_instance("aabbccddeeff"));
        assert!(!unnamed_instance("Harry's iPhone"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn system_browse_sees_a_service_on_awdl() {
        let instance = format!(
            "ws{:04x}{:04x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
                & 0xFFFF
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let advert = super::super::Advertisement::start(
            "_airdrop._tcp.local.",
            &instance,
            port,
            &[("flags", "16524")],
        )
        .unwrap_or_else(|error| panic!("register: {error}"));
        let peers = super::collect("_airdrop._tcp.local.", std::time::Duration::from_secs(4));
        advert.shutdown();
        drop(listener);
        let peer = peers.iter().find(|peer| peer.instance == instance);
        assert!(
            peer.map(|peer| peer.addr.port()) == Some(port),
            "missing {instance} port {port} in {:?}",
            peers
                .iter()
                .map(|peer| format!("{} {}", peer.instance, peer.addr))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn lan_address_is_used_when_awdl_is_absent() {
        let lan = HostAddress {
            interface: 15,
            ip: "192.168.1.10".parse().unwrap(),
            zone: None,
        };
        let chosen = pick_host_addr(&[lan], 16).unwrap();
        assert_eq!(chosen.interface, 15);
    }
}
