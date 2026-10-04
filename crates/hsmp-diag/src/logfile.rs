//! The log file behind `--log-dir`: `<dir>\<name>.log`, rotated to `<name>.1.log` ..
//! `<name>.<keep>.log` above `max_bytes`.
//!
//! Writes never block the caller: a write queues the line (one `tracing` event) on a bounded
//! channel and a background thread appends it. When the queue is full the line is dropped and
//! counted; the next line written says how many were lost. The file is flushed whenever the
//! queue runs empty, so a crash loses at most what was still queued.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::Duration;

const QUEUE: usize = 4096;

enum Msg {
    Line(Vec<u8>),
    Flush(SyncSender<()>),
}

pub struct LogWriter {
    tx: SyncSender<Msg>,
    dropped: Arc<AtomicU64>,
    // the unfinished line of this handle (a line is queued whole)
    part: Vec<u8>,
}

impl Clone for LogWriter {
    fn clone(&self) -> Self {
        LogWriter { tx: self.tx.clone(), dropped: self.dropped.clone(), part: Vec::new() }
    }
}

impl Drop for LogWriter {
    fn drop(&mut self) {
        if !self.part.is_empty() {
            let b = std::mem::take(&mut self.part);
            self.send(b);
        }
    }
}

impl Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.part.extend_from_slice(buf);
        if let Some(i) = self.part.iter().rposition(|&b| b == b'\n') {
            let rest = self.part.split_off(i + 1);
            let line = std::mem::replace(&mut self.part, rest);
            self.send(line);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl LogWriter {
    fn send(&self, b: Vec<u8>) {
        match self.tx.try_send(Msg::Line(b)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    /// Wait (at most `timeout`) until everything queued so far is on disk.
    pub fn flush_wait(&self, timeout: Duration) -> bool {
        let (ack_tx, ack_rx) = sync_channel(1);
        if self.tx.send(Msg::Flush(ack_tx)).is_err() {
            return false;
        }
        ack_rx.recv_timeout(timeout).is_ok()
    }
}

/// Flushes the file when dropped (end of `main`).
pub struct FlushGuard(LogWriter);

impl Drop for FlushGuard {
    fn drop(&mut self) {
        self.0.flush_wait(Duration::from_secs(1));
    }
}

pub fn log_path(dir: &Path, name: &str, i: usize) -> PathBuf {
    if i == 0 {
        dir.join(format!("{name}.log"))
    } else {
        dir.join(format!("{name}.{i}.log"))
    }
}

fn rotate(dir: &Path, name: &str, keep: usize) {
    let _ = std::fs::remove_file(log_path(dir, name, keep));
    for i in (1..keep).rev() {
        let _ = std::fs::rename(log_path(dir, name, i), log_path(dir, name, i + 1));
    }
    let _ = std::fs::rename(log_path(dir, name, 0), log_path(dir, name, 1));
}

/// How a log file is rotated.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rotation {
    /// `<name>.log`, renamed to `<name>.1.log` .. `<name>.<keep>.log` above `max` bytes
    /// (a session folder: the folder itself is pruned).
    Size { max: u64, keep: usize },
    /// `<name>-<YYYYMMDD>.<ext>` (UTC; `<name>-<YYYYMMDD>.<n>.<ext>` past `max_file` bytes);
    /// files of this name older than `keep_days`, or beyond `max_total` bytes in all (oldest
    /// first), are deleted whenever a new file is opened (a dedicated server).
    Daily { max_file: u64, keep_days: u64, max_total: u64 },
}

/// `<name>-<day>.<ext>` / `<name>-<day>.<n>.<ext>`
pub fn daily_path(dir: &Path, name: &str, ext: &str, day: &str, n: u32) -> PathBuf {
    if n == 0 {
        dir.join(format!("{name}-{day}.{ext}"))
    } else {
        dir.join(format!("{name}-{day}.{n}.{ext}"))
    }
}

/// Delete `<name>-*.<ext>` files in `dir` older than `keep_days` or beyond `max_total` bytes
/// (oldest first), never `current`. Returns the files removed.
pub fn prune_daily(dir: &Path, name: &str, ext: &str, keep_days: u64, max_total: u64, current: Option<&Path>, now_ms: u64) -> Vec<PathBuf> {
    let prefix = format!("{name}-");
    let suffix = format!(".{ext}");
    let mut files: Vec<(PathBuf, u64, u64)> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_name().to_str().is_some_and(|n| n.starts_with(&prefix) && n.ends_with(&suffix) && n[prefix.len()..].bytes().take(8).all(|b| b.is_ascii_digit())))
                .filter_map(|e| {
                    let m = e.metadata().ok()?;
                    Some((e.path(), crate::time::mtime_ms(&e.path()).unwrap_or(0), m.len()))
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort_by_key(|f| f.1);
    let is_current = |p: &Path| current.is_some_and(|c| c == p);
    let mut removed = vec![];
    let cutoff = now_ms.saturating_sub(keep_days * 86_400_000);
    let mut total: u64 = files.iter().map(|f| f.2).sum();
    for (p, t, len) in &files {
        if is_current(p) {
            continue;
        }
        if *t < cutoff || total > max_total {
            if std::fs::remove_file(p).is_ok() {
                total = total.saturating_sub(*len);
                removed.push(p.clone());
            }
        }
    }
    removed
}

struct Sink {
    dir: PathBuf,
    name: String,
    ext: String,
    rot: Rotation,
    file: Option<std::fs::File>,
    path: PathBuf,
    size: u64,
    /// Daily: the UTC day index and the overflow number of the open file.
    day: u64,
    n: u32,
    dropped: Arc<AtomicU64>,
}

impl Sink {
    fn open(&mut self) {
        self.path = match self.rot {
            Rotation::Size { .. } => self.dir.join(format!("{}.{}", self.name, self.ext)),
            Rotation::Daily { max_file, keep_days, max_total } => {
                let now = crate::time::now_ms();
                self.day = now / 86_400_000;
                let day = crate::time::stamp(now)[..8].to_string();
                // the first file of today that still has room (a restart appends)
                let mut p = daily_path(&self.dir, &self.name, &self.ext, &day, self.n);
                while std::fs::metadata(&p).is_ok_and(|m| m.len() >= max_file) {
                    self.n += 1;
                    p = daily_path(&self.dir, &self.name, &self.ext, &day, self.n);
                }
                prune_daily(&self.dir, &self.name, &self.ext, keep_days, max_total, Some(&p), now);
                p
            }
        };
        self.file = std::fs::OpenOptions::new().create(true).append(true).open(&self.path).ok();
        self.size = self.file.as_ref().and_then(|f| f.metadata().ok()).map(|m| m.len()).unwrap_or(0);
    }
    fn write(&mut self, b: &[u8]) {
        let lost = self.dropped.swap(0, Ordering::Relaxed);
        match self.rot {
            Rotation::Size { max, keep } => {
                if self.size > 0 && self.size + b.len() as u64 > max {
                    self.file = None;
                    if self.ext == "log" {
                        rotate(&self.dir, &self.name, keep);
                    } else {
                        let old = self.dir.join(format!("{}.1.{}", self.name, self.ext));
                        let _ = std::fs::rename(&self.path, old);
                    }
                    self.open();
                }
            }
            Rotation::Daily { max_file, .. } => {
                let today = crate::time::now_ms() / 86_400_000;
                if today != self.day {
                    self.n = 0;
                    self.file = None;
                    self.open();
                } else if self.size > 0 && self.size + b.len() as u64 > max_file {
                    self.n += 1;
                    self.file = None;
                    self.open();
                }
            }
        }
        if let Some(f) = self.file.as_mut() {
            if lost > 0 && self.ext == "log" {
                let m = format!("[log: {lost} line(s) dropped, the writer fell behind]\n");
                let _ = f.write_all(m.as_bytes());
                self.size += m.len() as u64;
            }
            if f.write_all(b).is_ok() {
                self.size += b.len() as u64;
            }
        }
    }
    fn flush(&mut self) {
        if let Some(f) = self.file.as_mut() {
            let _ = f.flush();
        }
    }
}

fn run(mut sink: Sink, rx: Receiver<Msg>) {
    loop {
        let m = match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(m) => m,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let mut next = Some(m);
        while let Some(m) = next.take() {
            match m {
                Msg::Line(b) => sink.write(&b),
                Msg::Flush(ack) => {
                    sink.flush();
                    let _ = ack.send(());
                }
            }
            next = rx.try_recv().ok();
        }
        sink.flush();
    }
    sink.flush();
}

/// Open `<dir>\<name>.<ext>` with `rot` (the folder is created) and start the writer thread.
/// `banner`: the first line marks the process start (text logs, not JSON lines).
pub fn open_with(dir: &Path, name: &str, ext: &str, rot: Rotation, banner: bool) -> io::Result<(LogWriter, FlushGuard)> {
    std::fs::create_dir_all(dir)?;
    let dropped = Arc::new(AtomicU64::new(0));
    let rot = match rot {
        Rotation::Size { max, keep } => Rotation::Size { max: max.max(4096), keep: keep.max(1) },
        Rotation::Daily { max_file, keep_days, max_total } => Rotation::Daily { max_file: max_file.max(4096), keep_days: keep_days.max(1), max_total },
    };
    let mut sink = Sink { dir: dir.to_path_buf(), name: name.to_string(), ext: ext.to_string(), rot, file: None, path: PathBuf::new(), size: 0, day: 0, n: 0, dropped: dropped.clone() };
    sink.open();
    if sink.file.is_none() {
        return Err(io::Error::other(format!("cannot open {}", sink.path.display())));
    }
    let (tx, rx) = sync_channel(QUEUE);
    std::thread::Builder::new().name(format!("hsmp-log-{name}")).spawn(move || run(sink, rx))?;
    let mut w = LogWriter { tx, dropped, part: Vec::new() };
    if banner {
        let _ = writeln!(w, "==== {name} {} started, pid {} ====", crate::time::iso(crate::time::now_ms()), std::process::id());
    }
    Ok((w.clone(), FlushGuard(w)))
}

/// `open_with(dir, name, "log", Rotation::Size { max_bytes, keep }, true)`.
pub fn open(dir: &Path, name: &str, max_bytes: u64, keep: usize) -> io::Result<(LogWriter, FlushGuard)> {
    open_with(dir, name, "log", Rotation::Size { max: max_bytes, keep }, true)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_rotates_and_keeps() {
        let d = crate::sessions::tests::tmp("logfile");
        let (mut w, g) = open(&d, "sidecar", 4096, 2).unwrap();
        for i in 0..400 {
            writeln!(w, "line {i:04} {}", "x".repeat(40)).unwrap();
        }
        assert!(w.flush_wait(Duration::from_secs(5)));
        drop(g);
        let cur = std::fs::read_to_string(log_path(&d, "sidecar", 0)).unwrap();
        assert!(cur.contains("line 0399"), "the newest line is in the live file");
        assert!(log_path(&d, "sidecar", 1).is_file() && log_path(&d, "sidecar", 2).is_file());
        assert!(!log_path(&d, "sidecar", 3).exists(), "keep = 2");
        for i in 0..=2 {
            assert!(std::fs::metadata(log_path(&d, "sidecar", i)).unwrap().len() <= 4096 + 64);
        }
        // a second process appends to the live file
        let (mut w2, g2) = open(&d, "sidecar", 1 << 20, 2).unwrap();
        writeln!(w2, "second run").unwrap();
        drop(g2);
        let cur = std::fs::read_to_string(log_path(&d, "sidecar", 0)).unwrap();
        assert!(cur.contains("line 0399") && cur.contains("second run") && cur.matches("started, pid").count() >= 1);
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod daily_tests {
    use super::*;

    #[test]
    fn daily_files_overflow_and_prune() {
        let d = crate::sessions::tests::tmp("daily");
        let rot = Rotation::Daily { max_file: 4096, keep_days: 14, max_total: 1 << 30 };
        let (mut w, g) = open_with(&d, "server", "log", rot, true).unwrap();
        for i in 0..300 {
            writeln!(w, "line {i:04} {}", "y".repeat(40)).unwrap();
        }
        assert!(w.flush_wait(Duration::from_secs(5)));
        drop(g);
        let day = crate::time::stamp(crate::time::now_ms())[..8].to_string();
        assert!(daily_path(&d, "server", "log", &day, 0).is_file());
        assert!(daily_path(&d, "server", "log", &day, 1).is_file(), "past max_file: today's .1 file");
        // old days and the total size cap; other names untouched
        let old = daily_path(&d, "server", "log", "20200101", 0);
        std::fs::write(&old, b"old").unwrap();
        let f = std::fs::File::options().write(true).open(&old).unwrap();
        f.set_modified(std::time::SystemTime::now() - Duration::from_secs(30 * 86_400)).unwrap();
        drop(f);
        std::fs::write(d.join("other-20200101.log"), b"x").unwrap();
        let removed = prune_daily(&d, "server", "log", 14, 1 << 30, None, crate::time::now_ms());
        assert_eq!(removed, vec![old.clone()]);
        assert!(d.join("other-20200101.log").is_file());
        let cur = daily_path(&d, "server", "log", &day, 1);
        let removed = prune_daily(&d, "server", "log", 14, 10, Some(&cur), crate::time::now_ms());
        assert!(!removed.is_empty() && !removed.contains(&cur), "over the total: everything but the current file goes");
        let left: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("server-")).collect();
        assert_eq!(left.len(), 1);
        assert!(cur.is_file());
        // a JSON-lines file without a banner
        let (mut j, g) = open_with(&d, "events", "jsonl", rot, false).unwrap();
        writeln!(j, "{{\"a\":1}}").unwrap();
        drop(g);
        drop(j);
        assert_eq!(std::fs::read_to_string(daily_path(&d, "events", "jsonl", &day, 0)).unwrap(), "{\"a\":1}\n");
        let _ = std::fs::remove_dir_all(&d);
    }
}
