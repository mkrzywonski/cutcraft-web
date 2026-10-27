//! A detailed log of every cut, for troubleshooting afterwards: the job's settings, each write
//! to the cutter and each reply with its timing and any error, the driver's phases and pace,
//! and the outcome. One JSON object per line in `cut-logs/` beside the settings, newest kept.
//! Logs stay on this computer: they hold sizes and timings, never the design's name or path.

use std::cell::RefCell;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cutcraft_device::transport::{Transport, TransportError};
use serde_json::{Value, json};

/// How many cut logs are kept.
const KEEP: usize = 30;
/// Lines written between flushes (errors and the outcome flush at once).
const FLUSH_EVERY: usize = 20;

pub(crate) struct CutLog {
    out: Option<BufWriter<File>>,
    path: Option<PathBuf>,
    start: Instant,
    unflushed: usize,
}

impl CutLog {
    /// A new log in `dir` (none for no log), starting with `header`.
    pub(crate) fn create(dir: Option<&Path>, name: &str, header: Value) -> Self {
        let mut log = Self {
            out: None,
            path: None,
            start: Instant::now(),
            unflushed: 0,
        };
        let Some(dir) = dir else {
            return log;
        };
        if std::fs::create_dir_all(dir).is_err() {
            return log;
        }
        prune(dir);
        let unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let path = dir.join(format!("cut-{unix}-{name}.jsonl"));
        if let Ok(file) = File::create(&path) {
            log.out = Some(BufWriter::new(file));
            log.path = Some(path);
            let mut header = header;
            if let Value::Object(map) = &mut header {
                map.insert("ev".into(), json!("start"));
                map.insert("unix_time".into(), json!(unix));
            }
            log.line(header, true);
        }
        log
    }

    pub(crate) fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// One event, stamped with seconds since the log began.
    pub(crate) fn event(&mut self, mut event: Value, urgent: bool) {
        if let Value::Object(map) = &mut event {
            map.insert(
                "t".into(),
                json!((self.start.elapsed().as_secs_f64() * 1000.0).round() / 1000.0),
            );
        }
        self.line(event, urgent);
    }

    fn line(&mut self, value: Value, flush: bool) {
        let Some(out) = &mut self.out else {
            return;
        };
        if writeln!(out, "{value}").is_err() {
            self.out = None;
            return;
        }
        self.unflushed += 1;
        if flush || self.unflushed >= FLUSH_EVERY {
            let _ = out.flush();
            self.unflushed = 0;
        }
    }
}

/// Keep the newest logs only, leaving room for the next.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("cut-") && n.ends_with(".jsonl"))
        })
        .collect();
    // Names start with the Unix time, so they sort oldest first.
    logs.sort();
    let excess = (logs.len() + 1).saturating_sub(KEEP);
    for old in logs.into_iter().take(excess) {
        let _ = std::fs::remove_file(old);
    }
}

fn millis(d: Duration) -> f64 {
    (d.as_secs_f64() * 10_000.0).round() / 10.0
}

fn error_text(error: &TransportError) -> String {
    error.to_string()
}

/// A transport that logs every transfer: writes with their size, how much was accepted and how
/// long they waited; replies with their bytes (status and firmware text, never cut geometry).
pub(crate) struct Recording<'a, T> {
    pub(crate) inner: &'a mut T,
    pub(crate) log: &'a RefCell<CutLog>,
}

impl<T: Transport> Transport for Recording<'_, T> {
    fn write(&mut self, data: &[u8], timeout: Duration) -> Result<usize, TransportError> {
        let begun = Instant::now();
        let result = self.inner.write(data, timeout);
        let waited = begun.elapsed();
        let mut event = json!({
            "ev": "write",
            "len": data.len(),
            "ms": millis(waited),
            "timeout_s": timeout.as_secs_f64(),
        });
        // Escape sequences (status, initialise) are named; cut commands only by size.
        if data.first() == Some(&0x1b) && data.len() <= 4 {
            event["escape"] = json!(format!("{:02x?}", data));
        }
        let urgent = result.is_err();
        match &result {
            Ok(n) => event["accepted"] = json!(n),
            Err(error) => event["error"] = json!(error_text(error)),
        }
        self.log.borrow_mut().event(event, urgent);
        result
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, TransportError> {
        let begun = Instant::now();
        let result = self.inner.read(buf, timeout);
        let mut event = json!({"ev": "read", "ms": millis(begun.elapsed())});
        let urgent = result.is_err();
        match &result {
            Ok(n) => event["bytes"] = json!(String::from_utf8_lossy(&buf[..*n])),
            Err(error) => event["error"] = json!(error_text(error)),
        }
        self.log.borrow_mut().event(event, urgent);
        result
    }

    fn clear_halt(&mut self) -> Result<(), TransportError> {
        let result = self.inner.clear_halt();
        let event = match &result {
            Ok(()) => json!({"ev": "clear-halt"}),
            Err(error) => json!({"ev": "clear-halt", "error": error_text(error)}),
        };
        self.log.borrow_mut().event(event, true);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    impl Transport for Echo {
        fn write(&mut self, data: &[u8], _: Duration) -> Result<usize, TransportError> {
            if data.len() > 10 {
                return Err(TransportError::Timeout);
            }
            Ok(data.len())
        }
        fn read(&mut self, buf: &mut [u8], _: Duration) -> Result<usize, TransportError> {
            buf[0] = b'0';
            Ok(1)
        }
        fn clear_halt(&mut self) -> Result<(), TransportError> {
            Ok(())
        }
    }

    #[test]
    fn every_transfer_is_logged_with_its_timing_and_old_logs_are_pruned() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..KEEP + 3 {
            std::fs::write(dir.path().join(format!("cut-{:04}-old.jsonl", i)), "").unwrap();
        }
        let log = RefCell::new(CutLog::create(
            Some(dir.path()),
            "sheet-1",
            json!({"device": "test"}),
        ));
        let mut echo = Echo;
        let mut recording = Recording {
            inner: &mut echo,
            log: &log,
        };
        recording
            .write(&[0x1b, 0x05], Duration::from_secs(1))
            .unwrap();
        let mut byte = [0u8; 1];
        recording.read(&mut byte, Duration::from_secs(1)).unwrap();
        assert!(
            recording
                .write(&[b'D'; 20], Duration::from_secs(1))
                .is_err()
        );
        log.borrow_mut()
            .event(json!({"ev": "outcome", "outcome": "interrupted"}), true);
        let path = log.borrow().path().unwrap().to_owned();
        let lines: Vec<Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines[0]["ev"], "start");
        assert_eq!(lines[1]["escape"], "[1b, 05]");
        assert_eq!(lines[2]["bytes"], "0");
        assert_eq!(
            lines[3]["error"],
            "transfer timed out; delivery is uncertain"
        );
        assert_eq!(lines[3]["len"], 20);
        assert!(
            lines[3].get("escape").is_none(),
            "cut commands only by size"
        );
        assert_eq!(lines[4]["ev"], "outcome");
        let kept = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(kept, KEEP, "the oldest went; this one was added");
    }
}
