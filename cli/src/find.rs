//! Helpers for locating a recorded run by `run_id` across all tracked
//! files and versions in a `SessionStore`.

#![allow(dead_code)]

use anyhow::{anyhow, Context, Result};

use request_pilot_core::sessions::{SessionRecord, SessionStore};

/// A run located on disk, plus enough metadata to render exports.
pub struct FoundRun {
    pub file_id: String,
    pub sha256: String,
    pub run_id: String,
    /// Human-friendly file alias (from the file manifest's display name).
    pub file_alias: String,
    /// Raw `source.http` content of the version this run was recorded from.
    pub source: String,
    pub record: SessionRecord,
}

/// Walk every `files/<file>/versions/<sha>/sessions/<run>/run.json` under
/// the store and return the first match for `run_id`.
pub fn find_run_by_id(store: &SessionStore, run_id: &str) -> Result<FoundRun> {
    let files = store
        .list_files()
        .map_err(|e| anyhow!("failed to list tracked files: {}", e))?;

    for f in files {
        let versions = match store.list_versions(&f.file_id) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for v in versions {
            let sessions = match store.list_sessions(&f.file_id, &v.sha256) {
                Ok(s) => s,
                Err(_) => continue,
            };
            if sessions.iter().any(|s| s.run_id == run_id) {
                let (source, record) = store
                    .load_session(&f.file_id, &v.sha256, run_id)
                    .with_context(|| {
                        format!("failed to load run {} ({}/{})", run_id, f.file_id, v.sha256)
                    })?;
                return Ok(FoundRun {
                    file_id: f.file_id.clone(),
                    sha256: v.sha256.clone(),
                    run_id: record.run_id.clone(),
                    file_alias: f.display_name.clone(),
                    source,
                    record,
                });
            }
        }
    }

    Err(anyhow!("run not found: {}", run_id))
}
