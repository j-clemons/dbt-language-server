//! The cogitator.yml watcher task (shell around `config_file`): the
//! dispatcher hands it the workspace root at initialize; from then on a
//! 2 s stamp poll, a didSave/didOpen fast-path poke and dbt.reloadConfig
//! each trigger a reload pass. No OS watcher dependency and no dynamic
//! registration — matching the spec's posture (05 §6.2).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio::sync::{mpsc, watch};

use super::config_file::{self, Baseline, FileConfig};
use super::{client_link::ClientLink, derived_cache, ProfilesConfig, SharedLimits, SharedToggles};

/// What `dbt.status` reports about the config file.
#[derive(Debug, Default, Clone, Serialize)]
pub struct ConfigStatus {
    /// The file currently loaded (absent: no file at the root).
    pub path: Option<String>,
    pub loaded: bool,
    /// The last parse failure, while the file stays broken.
    pub error: Option<String>,
}

/// The file names accepted at the workspace root, in lookup order.
pub const FILE_NAMES: &[&str] = &["cogitator.yml", ".cogitator.yml"];

pub struct ConfigWatcher {
    /// Set once by the dispatcher at initialize.
    pub root_rx: watch::Receiver<Option<String>>,
    /// Fast-path triggers: didSave/didOpen of the file, dbt.reloadConfig.
    pub poke_rx: mpsc::UnboundedReceiver<()>,
    pub link: ClientLink,
    pub refresh_tx: mpsc::UnboundedSender<String>,
    pub profiles: Arc<std::sync::Mutex<ProfilesConfig>>,
    pub limits: Arc<SharedLimits>,
    pub toggles: Arc<SharedToggles>,
    pub status: Arc<std::sync::Mutex<ConfigStatus>>,
}

/// One stat+read pass, off the async runtime (05 §6.1).
fn read_current(root: &str) -> Option<(PathBuf, derived_cache::FileStamp, String)> {
    for name in FILE_NAMES {
        let path = std::path::Path::new(root).join(name);
        if let Some(stamp) = derived_cache::stamp(&path) {
            let text = std::fs::read_to_string(&path).ok()?;
            return Some((path, stamp, text));
        }
    }
    None
}

impl ConfigWatcher {
    pub async fn run(mut self) {
        let Some(root) = self.root().await else {
            return; // server shut down before initialize
        };
        // Launch values (defaults + --config + CLI + initializationOptions,
        // all applied before initialize answers): what removed keys revert to.
        let baseline = Baseline {
            profiles: self.profiles.lock().expect("profiles config lock").clone(),
            compiled_cap_mib: self.limits.compiled_cap_bytes.load(Ordering::Relaxed)
                / (1024 * 1024),
            debounce_ms: self.limits.refresh_debounce_ms.load(Ordering::Relaxed),
            artifact_mib: self.limits.artifact_cap_bytes.load(Ordering::Relaxed) / (1024 * 1024),
            lint_flags: self.toggles.lints.load(Ordering::Relaxed),
            warehouse_columns: self.toggles.warehouse_columns.load(Ordering::Relaxed),
            client_log_level: crate::client_log::current_level(),
            perf: self
                .toggles
                .perf
                .read()
                .expect("perf settings lock")
                .as_ref()
                .clone(),
        };
        let mut last = FileConfig::default();
        let mut last_stamp: Option<(PathBuf, derived_cache::FileStamp)> = None;
        let mut first_load = true;
        let mut warned: HashSet<String> = HashSet::new();
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            let forced = tokio::select! {
                _ = tick.tick() => false,
                poke = self.poke_rx.recv() => {
                    if poke.is_none() {
                        return; // dispatcher dropped: shutdown
                    }
                    true
                }
            };
            let current = {
                let root = root.clone();
                tokio::task::spawn_blocking(move || read_current(&root)).await
            };
            let Ok(current) = current else { continue };
            match current {
                Some((path, stamp, text)) => {
                    let unchanged = last_stamp
                        .as_ref()
                        .is_some_and(|(p, s)| *p == path && *s == stamp);
                    if unchanged && !forced {
                        continue;
                    }
                    last_stamp = Some((path.clone(), stamp));
                    self.load(
                        &root,
                        &path,
                        &text,
                        &baseline,
                        &mut last,
                        &mut first_load,
                        &mut warned,
                    );
                }
                None => {
                    last_stamp = None;
                    self.file_gone(&root, &baseline, &mut last, &mut first_load);
                }
            }
        }
    }

    async fn root(&mut self) -> Option<String> {
        loop {
            if let Some(root) = self.root_rx.borrow().clone() {
                return (!root.is_empty()).then_some(root);
            }
            self.root_rx.changed().await.ok()?;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn load(
        &self,
        root: &str,
        path: &std::path::Path,
        text: &str,
        baseline: &Baseline,
        last: &mut FileConfig,
        first_load: &mut bool,
        warned: &mut HashSet<String>,
    ) {
        let new = match config_file::parse(text, path) {
            Ok(new) => new,
            Err(guidance) => {
                let message = guidance.to_message();
                self.status.lock().expect("config status lock").error = Some(message.clone());
                self.link
                    .show_message(cogitator_lsp::message_type::ERROR, message);
                return; // previous configuration stays active
            }
        };
        for warning in config_file::unknown_key_warnings(text, path) {
            let message = warning.to_message();
            if warned.insert(message.clone()) {
                self.link
                    .show_message(cogitator_lsp::message_type::WARNING, message);
            }
        }
        if !new.startup.is_empty() {
            let message = cogitator_error::guidance::Guidance::new(
                "cogitator.yml 'startup:' keys cannot be hot-applied.",
            )
            .at(path.display().to_string())
            .because("the tracing subscriber and parser backend are install-once.".to_string())
            .fix("pass them at launch via --config <path> or the CLI flags, then restart.")
            .to_message();
            if warned.insert(message.clone()) {
                self.link
                    .show_message(cogitator_lsp::message_type::WARNING, message);
            }
        }
        let mut changes = config_file::diff(last, &new, baseline);
        if std::mem::take(first_load) {
            changes = config_file::launch_filter(changes, baseline);
        }
        let applied = config_file::apply(&changes, &self.profiles, &self.limits, &self.toggles);
        *last = new;
        {
            let mut status = self.status.lock().expect("config status lock");
            status.path = Some(path.display().to_string());
            status.loaded = true;
            status.error = None;
        }
        if !applied.is_empty() {
            self.link.show_message(
                cogitator_lsp::message_type::INFO,
                format!("cogitator.yml applied: {}.", applied.join(", ")),
            );
            let _ = self.refresh_tx.send(root.to_string());
        }
    }

    fn file_gone(
        &self,
        root: &str,
        baseline: &Baseline,
        last: &mut FileConfig,
        first_load: &mut bool,
    ) {
        {
            let mut status = self.status.lock().expect("config status lock");
            status.path = None;
            status.loaded = false;
            status.error = None;
        }
        if std::mem::take(first_load) || *last == FileConfig::default() {
            *last = FileConfig::default();
            return; // nothing was ever applied
        }
        let changes = config_file::diff(last, &FileConfig::default(), baseline);
        let reverted = config_file::apply(&changes, &self.profiles, &self.limits, &self.toggles);
        *last = FileConfig::default();
        if !reverted.is_empty() {
            self.link.show_message(
                cogitator_lsp::message_type::INFO,
                format!(
                    "cogitator.yml removed; reverted to launch values: {}.",
                    reverted.join(", ")
                ),
            );
            let _ = self.refresh_tx.send(root.to_string());
        }
    }
}
