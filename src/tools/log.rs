#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
}

#[derive(Debug, Clone)]
pub struct LogEvent {
    pub subject: LogHeadline,
    pub meta: LogMeta,
}

#[derive(Debug, Clone)]
pub struct LogHeadline {
    pub component: String,
    pub summary: String,
}

#[derive(Debug, Clone)]
pub struct LogMeta {
    pub source_module: String,
    pub correlation_id: Option<String>,
}

/// ponytail: stderr only — no subscriber crate for demo/refactor fixture
pub fn adj_log(event: &LogEvent, level: LogLevel) {
    let cid = event
        .meta
        .correlation_id
        .as_deref()
        .map(|id| format!(" cid={id}"))
        .unwrap_or_default();
    eprintln!(
        "[adjutant][{level:?}][{}@{}] {}{}",
        event.subject.component,
        event.meta.source_module,
        event.subject.summary,
        cid
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_level_variants_exist() {
        assert_ne!(LogLevel::Debug, LogLevel::Info);
    }

    #[test]
    fn adj_log_formats_nested_event() {
        let event = LogEvent {
            subject: LogHeadline {
                component: "test".into(),
                summary: "hello".into(),
            },
            meta: LogMeta {
                source_module: "log::tests".into(),
                correlation_id: Some("abc".into()),
            },
        };
        adj_log(&event, LogLevel::Info);
    }
}
