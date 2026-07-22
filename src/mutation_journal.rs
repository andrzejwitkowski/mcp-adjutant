//! Job-scoped path snapshot / rollback for mutating agents.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

// ponytail: one journal map per process; keyed by request_uuid (concurrent jobs)
static JOURNALS: OnceLock<Mutex<HashMap<String, MutationJournal>>> = OnceLock::new();

fn journals() -> &'static Mutex<HashMap<String, MutationJournal>> {
    JOURNALS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn current_job_id() -> Option<String> {
    crate::metrics::current_job_context()?.request_uuid
}

#[derive(Debug)]
enum Prior {
    Absent,
    Bytes(Vec<u8>),
}

#[derive(Debug)]
pub struct MutationJournal {
    entries: HashMap<PathBuf, Prior>,
}

impl MutationJournal {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn write_file(&mut self, path: &Path, content: &[u8]) -> Result<(), String> {
        if !self.entries.contains_key(path) {
            let prior = if path.exists() {
                Prior::Bytes(
                    std::fs::read(path)
                        .map_err(|err| format!("snapshot {}: {err}", path.display()))?,
                )
            } else {
                Prior::Absent
            };
            self.entries.insert(path.to_path_buf(), prior);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("create dir {}: {err}", parent.display()))?;
        }
        std::fs::write(path, content).map_err(|err| format!("write {}: {err}", path.display()))
    }

    pub fn rollback(self) -> Result<(), String> {
        let mut errors = Vec::new();
        for (path, prior) in self.entries {
            let result = match prior {
                Prior::Absent => {
                    if path.exists() {
                        std::fs::remove_file(&path).or_else(|_| std::fs::remove_dir_all(&path))
                    } else {
                        Ok(())
                    }
                }
                Prior::Bytes(bytes) => {
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    std::fs::write(&path, bytes)
                }
            };
            if let Err(err) = result {
                errors.push(format!("{}: {err}", path.display()));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(format!("rollback incomplete: {}", errors.join("; ")))
        }
    }

    pub fn diff_summary(&self) -> String {
        let mut paths: Vec<_> = self
            .entries
            .keys()
            .map(|p| p.display().to_string())
            .collect();
        paths.sort();
        if paths.is_empty() {
            "(no files touched)".into()
        } else {
            format!("touched {} file(s):\n- {}", paths.len(), paths.join("\n- "))
        }
    }
}

pub fn begin_job_journal(job_id: impl Into<String>) {
    let id = job_id.into();
    if let Ok(mut map) = journals().lock() {
        map.insert(id, MutationJournal::new());
    }
}

pub fn with_active_journal<R>(f: impl FnOnce(&mut MutationJournal) -> R) -> Option<R> {
    let id = current_job_id()?;
    let mut map = journals().lock().ok()?;
    map.get_mut(&id).map(f)
}

pub fn journaled_write(path: &Path, content: &[u8]) -> Result<(), String> {
    if let Some(id) = current_job_id() {
        let mut map = journals()
            .lock()
            .map_err(|_| "mutation journal lock poisoned".to_string())?;
        if let Some(journal) = map.get_mut(&id) {
            return journal.write_file(path, content);
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    std::fs::write(path, content).map_err(|err| err.to_string())
}

pub fn end_job_journal(job_id: &str, ok: bool) -> Result<Option<String>, String> {
    let Some(journal) = journals()
        .lock()
        .map_err(|_| "mutation journal lock poisoned".to_string())?
        .remove(job_id)
    else {
        return Ok(None);
    };
    let summary = journal.diff_summary();
    if ok {
        Ok(Some(summary))
    } else {
        journal.rollback()?;
        Ok(Some(format!("rolled back {summary}")))
    }
}

pub fn assert_path_under_root(path: &Path, root: &Path) -> Result<(), String> {
    let canon_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let parent = path.parent().unwrap_or(path);
    let canon = if path.exists() {
        path.canonicalize()
            .map_err(|err| format!("canonicalize {}: {err}", path.display()))?
    } else {
        let parent_canon = parent
            .canonicalize()
            .unwrap_or_else(|_| parent.to_path_buf());
        parent_canon.join(path.file_name().unwrap_or_default())
    };
    if canon.starts_with(&canon_root) {
        Ok(())
    } else {
        Err(format!(
            "path {} escapes workspace {}",
            path.display(),
            canon_root.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("adjutant-journal-{label}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rollback_restores_prior_bytes_and_deletes_new_files() {
        let dir = temp_dir("rb");
        let existing = dir.join("existing.txt");
        let created = dir.join("created.txt");
        std::fs::write(&existing, b"old").unwrap();

        let mut journal = MutationJournal::new();
        journal.write_file(&existing, b"new").unwrap();
        journal.write_file(&created, b"fresh").unwrap();
        assert_eq!(std::fs::read(&existing).unwrap(), b"new");
        assert!(created.exists());

        journal.rollback().unwrap();
        assert_eq!(std::fs::read(&existing).unwrap(), b"old");
        assert!(!created.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn end_job_journal_keeps_writes_on_ok() {
        begin_job_journal("job-ok");
        let dir = temp_dir("ok");
        let path = dir.join("a.txt");
        {
            let mut map = journals().lock().unwrap();
            map.get_mut("job-ok")
                .unwrap()
                .write_file(&path, b"kept")
                .unwrap();
        }
        let summary = end_job_journal("job-ok", true).unwrap().unwrap();
        assert!(summary.contains("touched"));
        assert_eq!(std::fs::read(&path).unwrap(), b"kept");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
