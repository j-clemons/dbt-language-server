//! On-disk scan-cache persistence (03 §10, 05 §9): a versioned JSON
//! snapshot at `<root>/target/.cogitator-cache`. Loaded once per cold
//! process, saved after each successful context build — both inside
//! the refresh task's spawn_blocking, never on the async workers.
//! ANY load problem (missing, corrupt, other version) is a trace and
//! a cold start, nothing more.

use std::path::{Path, PathBuf};

use cogitator_analysis::context::persist::PersistedScan;
use serde::{Deserialize, Serialize};

const FORMAT: u32 = 4; // 4: ColumnEntry carries tests; 3: ColumnEntry carries data_type; 2: RawDocsBlock name position

#[derive(Serialize, Deserialize)]
struct PersistedFile {
    format: u32,
    server: String,
    scan: PersistedScan,
}

pub fn cache_path(root: &Path) -> PathBuf {
    root.join("target").join(".cogitator-cache")
}

/// The persisted scan, when the file exists, parses, and matches this
/// build exactly (format AND server version — stale shapes never load).
pub fn load(root: &Path) -> Option<PersistedScan> {
    let path = cache_path(root);
    let text = std::fs::read_to_string(&path).ok()?;
    let parsed: PersistedFile = match serde_json::from_str(&text) {
        Ok(parsed) => parsed,
        Err(err) => {
            tracing::debug!(path = %path.display(), %err, "cache unreadable; cold start");
            return None;
        }
    };
    if parsed.format != FORMAT || parsed.server != crate::VERSION {
        tracing::debug!(
            path = %path.display(),
            format = parsed.format,
            server = %parsed.server,
            "cache from another build; cold start"
        );
        return None;
    }
    tracing::debug!(path = %path.display(), "scan cache loaded");
    Some(parsed.scan)
}

/// Atomic same-directory write (temp + rename — one volume, so the
/// rename is atomic on Windows too). Failures are traces: persistence
/// is an optimization, never a feature the user can lose work to.
pub fn save(root: &Path, scan: PersistedScan) {
    let path = cache_path(root);
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let file = PersistedFile {
        format: FORMAT,
        server: crate::VERSION.to_string(),
        scan,
    };
    let Ok(json) = serde_json::to_string(&file) else {
        return;
    };
    let tmp = path.with_extension("tmp");
    if let Err(err) = std::fs::write(&tmp, json).and_then(|()| std::fs::rename(&tmp, &path)) {
        tracing::debug!(path = %path.display(), %err, "cache save skipped");
    }
}
