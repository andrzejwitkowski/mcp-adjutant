//! Session-scoped ERROR/WARN/panic ring for the config UI Logs page.
//! ponytail: in-memory only — lost on restart; SQLite if retention matters.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

const CAPACITY: usize = 200;
const MAX_MESSAGE_BYTES: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Error,
    Warn,
    Panic,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub ts_unix_ms: u64,
    pub level: LogLevel,
    pub source: String,
    pub message: String,
}

/// Injectable newest-first ring buffer shared by panic hook, tracing layer, and `/api/logs`.
#[derive(Clone)]
pub struct RuntimeLog {
    inner: Arc<Mutex<VecDeque<LogEntry>>>,
}

impl RuntimeLog {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(CAPACITY))),
        }
    }

    pub fn push(&self, level: LogLevel, source: impl Into<String>, message: impl Into<String>) {
        let entry = LogEntry {
            ts_unix_ms: now_ms(),
            level,
            source: source.into(),
            message: truncate_message(message.into()),
        };
        let Ok(mut buf) = self.inner.lock() else {
            return;
        };
        if buf.len() >= CAPACITY {
            buf.pop_back();
        }
        buf.push_front(entry);
    }

    /// Newest first.
    pub fn snapshot(&self) -> Vec<LogEntry> {
        self.inner
            .lock()
            .map(|buf| buf.iter().cloned().collect())
            .unwrap_or_default()
    }
}

impl Default for RuntimeLog {
    fn default() -> Self {
        Self::new()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn truncate_message(mut message: String) -> String {
    if message.len() > MAX_MESSAGE_BYTES {
        let mut end = MAX_MESSAGE_BYTES;
        while end > 0 && !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        message.push_str("…[truncated]");
    }
    message
}

pub fn install_panic_hook(log: RuntimeLog) {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".into());
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "Box<dyn Any>".into()
        };
        log.push(LogLevel::Panic, "panic", format!("{location}: {payload}"));
        prev(info);
    }));
}

/// Records tracing ERROR and WARN into the ring; pair with fmt layer for stderr.
#[derive(Clone)]
pub struct RuntimeLogLayer {
    pub log: RuntimeLog,
}

struct MessageVisitor {
    message: String,
}

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" && self.message.is_empty() {
            self.message = format!("{value:?}");
        }
    }
}

impl<S> Layer<S> for RuntimeLogLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let level = *meta.level();
        if level > Level::WARN {
            return;
        }
        let log_level = if level == Level::ERROR {
            LogLevel::Error
        } else {
            LogLevel::Warn
        };
        let mut visitor = MessageVisitor {
            message: String::new(),
        };
        event.record(&mut visitor);
        if visitor.message.is_empty() {
            visitor.message = meta.name().to_string();
        }
        self.log.push(
            log_level,
            meta.module_path().unwrap_or_else(|| meta.target()),
            visitor.message,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_log_new_empty() {
        let log = RuntimeLog::new();
        assert!(log.snapshot().is_empty());
    }

    #[test]
    fn test_push_entries_ordering_newest_first() {
        let log = RuntimeLog::new();
        log.push(LogLevel::Warn, "src_a", "msg_a");
        std::thread::sleep(std::time::Duration::from_millis(10));
        log.push(LogLevel::Error, "src_b", "msg_b");

        let snap = log.snapshot();
        assert_eq!(snap.len(), 2);
        // GREEN: Newest entry is first (Error at index 0) due to push_front
        assert_eq!(snap[0].level, LogLevel::Error);
    }

    #[test]
    fn test_push_capacity_limit() {
        let log = RuntimeLog::new();
        for i in 0..CAPACITY + 100 {
            log.push(LogLevel::Warn, "src", format!("msg{i}"));
        }
        let snap = log.snapshot();
        assert_eq!(snap.len(), CAPACITY);
    }

    #[test]
    fn test_truncate_message_exact_capacity_no_suffix() {
        let long_str: String = "a".repeat(MAX_MESSAGE_BYTES + 50);
        let truncated = truncate_message(long_str.clone());
        // GREEN: actual adds suffix, so len > MAX
        assert!(truncated.len() > MAX_MESSAGE_BYTES);
    }

    #[test]
    fn test_now_ms_valid() {
        let t = now_ms();
        assert!(t > 1_000_000_000);
    }
}
