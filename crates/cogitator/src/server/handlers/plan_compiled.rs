//! The compiled-SQL file for `dbt.queryPlan`: stat and bounded read, off
//! the dispatcher task; the pure driver only ever sees `CompiledInput`.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Read;
use std::path::PathBuf;

use cogitator_analysis::features::compiled_target;
use cogitator_analysis::features::plan::{CompiledInput, Staleness};
use cogitator_analysis::DbtContext;

use super::super::derived_cache::{stamp, FileStamp};

/// The model's compiled path and the stats taken once per request.
pub(super) struct CompiledProbe {
    path: Option<PathBuf>,
    compiled: Option<FileStamp>,
    source: Option<FileStamp>,
}

/// What [`CompiledProbe::read`] found, owning what `CompiledInput` borrows.
pub(super) struct CompiledFile {
    path: Option<String>,
    state: FileState,
}

enum FileState {
    Missing,
    TooLarge { bytes: u64, cap: u64 },
    Unreadable(String),
    Available { text: String, mtime: Staleness },
}

impl CompiledProbe {
    pub(super) fn new(ctx: &DbtContext, uri: &str) -> Self {
        let target = compiled_target(ctx, uri, None);
        let source = target
            .as_ref()
            .and_then(|(model, _)| ctx.model_detail_map.get(model))
            .and_then(|details| stamp(&details.uri));
        let path = target.map(|(_, path)| path);
        CompiledProbe {
            compiled: path.as_deref().and_then(stamp),
            path,
            source,
        }
    }

    /// `revision`, moved whenever either file changes.
    pub(super) fn cache_key(&self, revision: u64) -> u64 {
        let mut hasher = DefaultHasher::new();
        (revision, self.compiled, self.source).hash(&mut hasher);
        // 48 bits like `schema_revision`: exact in double-decoding clients
        hasher.finish() & 0xFFFF_FFFF_FFFF
    }

    /// Reads at most `cap` bytes; a larger file is reported, never loaded.
    pub(super) fn read(&self, cap: u64) -> CompiledFile {
        let Some(path) = self.path.as_deref() else {
            return CompiledFile {
                path: None,
                state: FileState::Missing,
            };
        };
        let display = path.display().to_string();
        let Some((_, _, bytes)) = self.compiled else {
            return CompiledFile {
                path: Some(display),
                state: FileState::Missing,
            };
        };
        if bytes > cap {
            return CompiledFile {
                path: Some(display),
                state: FileState::TooLarge { bytes, cap },
            };
        }
        let state = match read_capped(path, cap) {
            Ok(text) => FileState::Available {
                text,
                mtime: compare_mtimes(self.compiled, self.source),
            },
            Err(err) => FileState::Unreadable(err.to_string()),
        };
        CompiledFile {
            path: Some(display),
            state,
        }
    }
}

impl CompiledFile {
    pub(super) fn input(&self) -> CompiledInput<'_> {
        let Some(path) = self.path.as_deref() else {
            return CompiledInput::NotChecked;
        };
        match &self.state {
            FileState::Missing => CompiledInput::Missing { path },
            FileState::TooLarge { bytes, cap } => CompiledInput::TooLarge {
                path,
                bytes: *bytes,
                cap: *cap,
            },
            FileState::Unreadable(detail) => CompiledInput::Unreadable { path, detail },
            FileState::Available { text, mtime } => CompiledInput::Available {
                text,
                path,
                mtime: *mtime,
            },
        }
    }
}

fn read_capped(path: &std::path::Path, cap: u64) -> std::io::Result<String> {
    let mut text = String::new();
    // A compile racing this read cannot push past the cap.
    std::fs::File::open(path)?
        .take(cap)
        .read_to_string(&mut text)?;
    Ok(text)
}

fn compare_mtimes(compiled: Option<FileStamp>, source: Option<FileStamp>) -> Staleness {
    match (compiled, source) {
        (Some(compiled), Some(source)) if (compiled.0, compiled.1) >= (source.0, source.1) => {
            Staleness::Fresh
        }
        (Some(_), Some(_)) => Staleness::Stale,
        _ => Staleness::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mtimes_compare_to_freshness() {
        assert_eq!(
            compare_mtimes(Some((2, 0, 1)), Some((1, 9, 1))),
            Staleness::Fresh
        );
        assert_eq!(
            compare_mtimes(Some((1, 0, 1)), Some((1, 0, 9))),
            Staleness::Fresh
        );
        assert_eq!(
            compare_mtimes(Some((1, 0, 1)), Some((1, 1, 1))),
            Staleness::Stale
        );
        assert_eq!(compare_mtimes(Some((1, 0, 1)), None), Staleness::Unknown);
        assert_eq!(compare_mtimes(None, Some((1, 0, 1))), Staleness::Unknown);
    }
}
