//! Byte progress for a send or receive.
//!
//! Reports are throttled to about ten a second, and also whenever the visible
//! percent changes, so a fast link does not flood the terminal or the apps.
//! `total == 0` means the size is not known yet.

use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::mime::human_size;

const REPORT_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteProgress {
    pub transferred: u64,
    pub total: u64,
}

pub type ProgressHook = Arc<dyn Fn(ByteProgress) + Send + Sync>;
pub type PeerProgressHook = Arc<dyn Fn(&str, ByteProgress) + Send + Sync>;
pub type FailHook = Arc<dyn Fn(&str, &str) + Send + Sync>;

pub fn percent(transferred: u64, total: u64) -> u64 {
    if total == 0 {
        return 0;
    }
    transferred.min(total).saturating_mul(100) / total
}

/// Map bytes of an AirDrop archive back onto the file bytes the person chose.
pub fn scale_bytes(written: u64, archive: u64, file_total: u64) -> u64 {
    if file_total == 0 || archive == 0 || written >= archive {
        return file_total;
    }
    let written = u128::from(written.min(archive));
    let scaled = written.saturating_mul(u128::from(file_total)) / u128::from(archive);
    u64::try_from(scaled).unwrap_or(file_total)
}

pub fn format_progress(verb: &str, peer_phrase: &str, progress: ByteProgress) -> String {
    let tail = if peer_phrase.is_empty() {
        String::new()
    } else {
        format!("  {peer_phrase}")
    };
    if progress.total > 0 {
        let pct = percent(progress.transferred.min(progress.total), progress.total);
        format!(
            "{verb}  {pct:>3}%  {} / {}{tail}",
            human_size(progress.transferred),
            human_size(progress.total)
        )
    } else if progress.transferred > 0 {
        format!("{verb}  {}{tail}", human_size(progress.transferred))
    } else if peer_phrase.is_empty() {
        verb.to_string()
    } else {
        format!("{verb}  {peer_phrase}")
    }
}

#[derive(Clone, Copy)]
struct Sample {
    bytes: u64,
    total: u64,
    at: Instant,
    started: bool,
}

impl Sample {
    fn fresh() -> Self {
        Self {
            bytes: 0,
            total: 0,
            at: Instant::now(),
            started: false,
        }
    }
}

fn should_emit(
    force: bool,
    transferred: u64,
    total: u64,
    prev: &Sample,
    interval: Duration,
) -> bool {
    if prev.started && transferred < prev.bytes {
        return false;
    }
    if prev.started && transferred == prev.bytes && total == prev.total {
        return false;
    }
    if !prev.started || force || total != prev.total {
        return true;
    }
    if total > 0 && transferred >= total {
        return true;
    }
    if percent(transferred, total) != percent(prev.bytes, prev.total) {
        return true;
    }
    prev.at.elapsed() >= interval
}

/// Counts bytes from several streams and reports the combined total.
pub struct ByteMeter {
    transferred: std::sync::atomic::AtomicU64,
    total: std::sync::atomic::AtomicU64,
    interval: Duration,
    sample: Mutex<Sample>,
    hook: ProgressHook,
}

impl ByteMeter {
    pub fn new(total: u64, hook: ProgressHook) -> Arc<Self> {
        Arc::new(Self {
            transferred: std::sync::atomic::AtomicU64::new(0),
            total: std::sync::atomic::AtomicU64::new(total),
            interval: REPORT_INTERVAL,
            sample: Mutex::new(Sample::fresh()),
            hook,
        })
    }

    pub fn set_total(&self, total: u64) {
        self.total
            .store(total, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn start(&self) {
        self.emit(true);
    }

    pub fn add(&self, n: u64) {
        if n == 0 {
            return;
        }
        self.transferred
            .fetch_add(n, std::sync::atomic::Ordering::Relaxed);
        self.emit(false);
    }

    /// `transferred` is an absolute count. A lower value is ignored.
    pub fn observe(&self, transferred: u64, total: u64) {
        if total > 0 {
            self.total
                .store(total, std::sync::atomic::Ordering::Relaxed);
        }
        self.transferred
            .fetch_max(transferred, std::sync::atomic::Ordering::Relaxed);
        self.emit(false);
    }

    pub fn finish(&self) {
        let total = self.total.load(std::sync::atomic::Ordering::Relaxed);
        if total > 0 {
            self.transferred
                .fetch_max(total, std::sync::atomic::Ordering::Relaxed);
        }
        self.emit(true);
    }

    fn emit(&self, force: bool) {
        let mut sample = self
            .sample
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let transferred = self.transferred.load(std::sync::atomic::Ordering::Relaxed);
        let total = self.total.load(std::sync::atomic::Ordering::Relaxed);
        if !should_emit(force, transferred, total, &sample, self.interval) {
            return;
        }
        *sample = Sample {
            bytes: transferred,
            total,
            at: Instant::now(),
            started: true,
        };
        let hook = Arc::clone(&self.hook);
        drop(sample);
        hook(ByteProgress { transferred, total });
    }
}

/// Single-task throttle used while an AirDrop body is written.
pub struct ProgressGate {
    total: u64,
    interval: Duration,
    sample: Sample,
}

impl ProgressGate {
    pub fn new(total: u64) -> Self {
        Self {
            total,
            interval: REPORT_INTERVAL,
            sample: Sample::fresh(),
        }
    }

    pub fn start(&mut self, hook: &dyn Fn(ByteProgress)) {
        self.push(0, true, hook);
    }

    pub fn observe(&mut self, transferred: u64, hook: &dyn Fn(ByteProgress)) {
        self.push(transferred.max(self.sample.bytes), false, hook);
    }

    pub fn finish(&mut self, hook: &dyn Fn(ByteProgress)) {
        let transferred = if self.total > 0 {
            self.total.max(self.sample.bytes)
        } else {
            self.sample.bytes
        };
        self.push(transferred, true, hook);
    }

    fn push(&mut self, transferred: u64, force: bool, hook: &dyn Fn(ByteProgress)) {
        if !should_emit(force, transferred, self.total, &self.sample, self.interval) {
            return;
        }
        self.sample = Sample {
            bytes: transferred,
            total: self.total,
            at: Instant::now(),
            started: true,
        };
        hook(ByteProgress {
            transferred,
            total: self.total,
        });
    }
}

/// One updating line for `whoosh send`. A terminal rewrites the line in place.
pub struct SendBar {
    phrase: String,
    tty: bool,
    state: Mutex<BarState>,
}

struct BarState {
    len: usize,
    bucket: Option<u64>,
}

impl SendBar {
    pub fn new(phrase: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            phrase: phrase.into(),
            tty: std::io::stderr().is_terminal(),
            state: Mutex::new(BarState {
                len: 0,
                bucket: None,
            }),
        })
    }

    pub fn hook(self: &Arc<Self>) -> ProgressHook {
        let bar = Arc::clone(self);
        Arc::new(move |progress| bar.show(progress))
    }

    pub fn note(&self, text: &str) {
        if self.tty {
            self.paint(text);
        } else {
            eprintln!("{text}");
        }
    }

    pub fn show(&self, progress: ByteProgress) {
        let line = format_progress("sending", &self.phrase, progress);
        if self.tty {
            self.paint(&line);
            return;
        }
        if !self.advance(progress) {
            return;
        }
        eprintln!("{line}");
    }

    pub fn finish(&self) {
        if !self.tty {
            return;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if state.len > 0 {
            eprintln!();
            state.len = 0;
        }
    }

    fn advance(&self, progress: ByteProgress) -> bool {
        let bucket = if progress.total > 0 {
            let pct = percent(progress.transferred.min(progress.total), progress.total);
            if pct == 0 {
                return false;
            }
            let bucket = pct / 5 * 5;
            if bucket == 0 {
                return false;
            }
            bucket
        } else if progress.transferred == 0 {
            return false;
        } else {
            progress.transferred / (256 * 1024)
        };
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if state.bucket == Some(bucket) {
            return false;
        }
        state.bucket = Some(bucket);
        true
    }

    fn paint(&self, line: &str) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let width = state.len.max(line.len());
        let mut stderr = std::io::stderr();
        let _ = write!(stderr, "\r{line:<width$}");
        let _ = stderr.flush();
        state.len = width;
    }
}

impl Drop for SendBar {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Prints receive progress as whole lines so it can sit among the receiver's other logs.
pub struct ReceiveLog {
    last: Mutex<HashMap<String, u64>>,
}

impl ReceiveLog {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            last: Mutex::new(HashMap::new()),
        })
    }

    pub fn hook(self: &Arc<Self>, via: &str) -> PeerProgressHook {
        let log = Arc::clone(self);
        let via = via.to_string();
        Arc::new(move |peer, progress| log.show(&via, peer, progress))
    }

    fn show(&self, via: &str, peer: &str, progress: ByteProgress) {
        let bucket = if progress.total > 0 {
            let pct = percent(progress.transferred.min(progress.total), progress.total);
            if pct == 0 {
                return;
            }
            let bucket = pct / 5 * 5;
            if bucket == 0 {
                return;
            }
            bucket
        } else if progress.transferred == 0 {
            return;
        } else {
            progress.transferred / (256 * 1024)
        };
        let key = format!("{via}\0{peer}");
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if last.get(&key) == Some(&bucket) {
            return;
        }
        last.insert(key, bucket);
        drop(last);
        let phrase = if peer.is_empty() {
            String::new()
        } else {
            format!("from {peer}")
        };
        println!("{}", format_progress("receiving", &phrase, progress));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        format_progress, percent, scale_bytes, should_emit, ByteMeter, ByteProgress, Sample,
    };
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[test]
    fn formats_known_and_unknown_sizes() {
        assert_eq!(percent(50, 200), 25);
        assert_eq!(percent(0, 0), 0);
        let known = format_progress(
            "sending",
            "to Alice",
            ByteProgress {
                transferred: 512,
                total: 1024,
            },
        );
        assert!(known.contains("50%"), "{known}");
        assert!(known.contains("to Alice"), "{known}");
        let unknown = format_progress(
            "receiving",
            "from Bob",
            ByteProgress {
                transferred: 2048,
                total: 0,
            },
        );
        assert!(unknown.starts_with("receiving"), "{unknown}");
        assert!(unknown.contains("from Bob"), "{unknown}");
        assert!(!unknown.contains('%'), "{unknown}");
    }

    #[test]
    fn scales_archive_bytes_onto_file_bytes() {
        assert_eq!(scale_bytes(0, 200, 100), 0);
        assert_eq!(scale_bytes(100, 200, 100), 50);
        assert_eq!(scale_bytes(200, 200, 100), 100);
        assert_eq!(scale_bytes(50, 0, 100), 100);
    }

    #[test]
    fn meter_is_monotonic_and_reaches_the_total() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let meter = ByteMeter::new(
            1_000,
            Arc::new(move |progress| log.lock().unwrap().push(progress)),
        );
        meter.start();
        meter.add(100);
        meter.add(400);
        meter.observe(50, 1_000);
        meter.add(500);
        meter.finish();
        meter.finish();
        let seen = seen.lock().unwrap().clone();
        assert!(seen.len() >= 2, "{seen:?}");
        assert_eq!(seen.first().unwrap().transferred, 0);
        assert_eq!(seen.last().unwrap().transferred, 1_000);
        assert_eq!(seen.last().unwrap().total, 1_000);
        assert!(seen
            .windows(2)
            .all(|pair| pair[0].transferred <= pair[1].transferred));
    }

    #[test]
    fn same_percent_inside_the_interval_is_one_report() {
        let prev = Sample {
            bytes: 10,
            total: 10_000,
            at: std::time::Instant::now(),
            started: true,
        };
        assert!(!should_emit(
            false,
            20,
            10_000,
            &prev,
            Duration::from_secs(60)
        ));
        assert!(should_emit(
            false,
            200,
            10_000,
            &prev,
            Duration::from_secs(60)
        ));
        assert!(should_emit(
            true,
            20,
            10_000,
            &prev,
            Duration::from_secs(60)
        ));
        assert!(!should_emit(
            true,
            10,
            10_000,
            &prev,
            Duration::from_secs(60)
        ));
    }
}
