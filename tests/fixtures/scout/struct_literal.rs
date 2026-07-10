// comment mentions LogEvent { — should be ignored by AST
fn demo() {
    adj_log(
        &LogEvent {
            headline: LogHeadline {
                component: "a".into(),
                message: "b".into(),
            },
            meta: LogMeta {
                tags: vec![],
                correlation_id: None,
            },
        },
        LogLevel::Info,
    );
}

fn other() {
    let _ = LogEvent {
        headline: LogHeadline {
            component: "x".into(),
            message: "y".into(),
        },
        meta: LogMeta {
            tags: vec![],
            correlation_id: None,
        },
    };
}
