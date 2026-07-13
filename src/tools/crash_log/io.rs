use std::path::Path;

pub const MAX_LOG_BYTES: usize = 512 * 1024;
pub const LLM_LOG_BYTES: usize = 24_000;

pub fn truncate_log_text(text: &str) -> (String, bool) {
    truncate_log_bytes(text.as_bytes(), MAX_LOG_BYTES)
}

pub fn truncate_for_llm(text: &str) -> String {
    truncate_log_bytes(text.as_bytes(), LLM_LOG_BYTES).0
}

fn truncate_log_bytes(bytes: &[u8], max_bytes: usize) -> (String, bool) {
    if bytes.len() <= max_bytes {
        return (String::from_utf8_lossy(bytes).into_owned(), false);
    }
    let slice = &bytes[bytes.len().saturating_sub(max_bytes)..];
    let start = slice
        .iter()
        .position(|b| *b < 128 || *b >= 192)
        .unwrap_or(0);
    (String::from_utf8_lossy(&slice[start..]).into_owned(), true)
}

pub fn read_log_file(path: &Path) -> Result<(String, bool), String> {
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom};

    let mut file =
        File::open(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let len = file
        .metadata()
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?
        .len() as usize;
    if len <= MAX_LOG_BYTES {
        let mut bytes = Vec::with_capacity(len);
        file.read_to_end(&mut bytes)
            .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
        return Ok(truncate_log_bytes(&bytes, MAX_LOG_BYTES));
    }
    file.seek(SeekFrom::End(-(MAX_LOG_BYTES as i64)))
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let mut buf = vec![0u8; MAX_LOG_BYTES];
    file.read_exact(&mut buf)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let (content, _) = truncate_log_bytes(&buf, MAX_LOG_BYTES);
    Ok((content, true))
}

pub fn strip_file_url(path: &str) -> &str {
    path.strip_prefix("file://").unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_for_llm_caps_large_input() {
        let log = "a".repeat(LLM_LOG_BYTES + 500);
        let capped = truncate_for_llm(&log);
        assert!(capped.len() <= LLM_LOG_BYTES + 4);
        assert!(capped.ends_with('a'));
    }

    #[test]
    fn read_log_file_truncates_large_files() {
        let dir = std::env::temp_dir().join(format!("crash-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tmpdir");
        let path = dir.join("big.log");
        let payload = "x".repeat(MAX_LOG_BYTES + 1_000);
        std::fs::write(&path, &payload).expect("write");
        let (content, truncated) = read_log_file(&path).expect("read");
        assert!(truncated);
        assert!(content.len() <= MAX_LOG_BYTES + 4);
        std::fs::remove_dir_all(dir).ok();
    }
}
