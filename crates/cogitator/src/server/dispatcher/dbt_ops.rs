//! The per-command shells behind the executeCommand router: dbt
//! invocations, profile switches, lineage/report assembly — each one
//! snapshot-in, offload, Reply-out.

use std::sync::atomic::Ordering;

use cogitator_lsp::ResultResponse;

use super::Dispatcher;

impl Dispatcher {
    pub(super) fn column_lineage(&mut self, id: i64, uri: &str) {
        let snap = self.snapshot_for(uri);
        let derived = self.derived.clone();
        let uri = uri.to_string();
        let artifact_cap = self.limits.artifact_cap_bytes.load(Ordering::Relaxed);
        self.offload_blocking(id, "dbt.columnLineage", move || {
            let lineage = crate::server::handlers::lineage::typed_lineage_for(
                &snap,
                &derived,
                &uri,
                artifact_cap,
            );
            crate::server::handlers::commands::column_lineage(&lineage)
        });
    }

    pub(super) fn open_schema(&mut self, id: i64, uri: &str) {
        let snap = self.snapshot_for(uri);
        let reply = crate::server::handlers::navigation::open_schema(&snap, uri);
        self.transmit(Some(id), reply);
    }

    pub(super) fn show_lineage(&mut self, id: i64, uri: &str) {
        let snap = self.snapshot_for(uri);
        let derived = self.derived.clone();
        let uri = uri.to_string();
        let artifact_cap = self.limits.artifact_cap_bytes.load(Ordering::Relaxed);
        self.offload_blocking(id, "dbt.showLineage", move || {
            let lineage = crate::server::handlers::lineage::typed_lineage_for(
                &snap,
                &derived,
                &uri,
                artifact_cap,
            );
            crate::server::handlers::navigation::show_lineage((*lineage).as_ref())
        });
    }

    /// `dbt.run` / `dbt.test` / `dbt.compile` for one model, asynchronously.
    pub(super) fn run_dbt(&mut self, id: i64, kind: &'static str, uri: &str) {
        self.run_dbt_full(id, kind, uri, None, Vec::new(), None);
    }

    /// `dbt.preview`: `dbt show --select <model> --limit N`; the result
    /// table lands in target/.cogitator/preview/<selector>.txt and opens
    /// via showDocument.
    pub(super) fn preview(&mut self, id: i64, uri: &str, limit: u64) {
        let selector = crate::server::fusion::model_selector_from_uri(uri);
        let capture = self.snapshot().project_root.clone().map(|root| {
            root.join("target/.cogitator/preview")
                .join(format!("{selector}.txt"))
        });
        self.run_dbt_full(
            id,
            "show",
            uri,
            None,
            vec!["--limit".to_string(), limit.to_string()],
            capture,
        );
    }

    /// `selector`: Some overrides the uri-derived model selector
    /// (dbt.runSelection); None keeps the per-model behavior.
    pub(super) fn run_dbt_selected(
        &mut self,
        id: i64,
        kind: &'static str,
        uri: &str,
        selector_override: Option<String>,
    ) {
        self.run_dbt_full(id, kind, uri, selector_override, Vec::new(), None);
    }

    fn run_dbt_full(
        &mut self,
        id: i64,
        kind: &'static str,
        uri: &str,
        selector_override: Option<String>,
        extra_args: Vec<String>,
        capture_to: Option<std::path::PathBuf>,
    ) {
        let ctx = if uri.is_empty() {
            self.snapshot()
        } else {
            self.ctx_for(uri)
        };
        let Some(root) = ctx.project_root.clone() else {
            self.show_message(
                cogitator_lsp::message_type::ERROR,
                cogitator_error::guidance::Guidance::new(format!(
                    "Cannot run dbt {kind}: no dbt project found."
                ))
                .because(format!(
                    "dbt_project.yml was not located from the workspace root '{}'.",
                    self.root_path
                ))
                .fix(
                    "open the dbt project directory (the folder containing \
                     dbt_project.yml) or check the editor's rootUri.",
                )
                .to_message(),
            );
            self.send(&ResultResponse::new(id, serde_json::Value::Null));
            return;
        };
        let binary = if self.fusion_path.is_empty() {
            "dbt".to_string()
        } else {
            self.fusion_path.clone()
        };
        let link = self.link.clone();
        let (target, profiles_dir, profile) = {
            let cfg = self.profiles.lock().expect("profiles config lock");
            (
                (!cfg.target.is_empty()).then(|| cfg.target.clone()),
                (!cfg.dir_override.is_empty()).then(|| cfg.dir_override.clone()),
                (!cfg.profile.is_empty()).then(|| cfg.profile.clone()),
            )
        };
        // parse and source freshness are project-wide (no --select);
        // parse additionally refreshes the context on success
        let project_wide = kind == "parse" || kind == "source freshness";
        let selector = selector_override.or_else(|| {
            (!project_wide).then(|| crate::server::fusion::model_selector_from_uri(uri))
        });
        let refresh = (kind == "parse").then(|| (self.refresh_tx.clone(), self.root_path.clone()));
        let token = crate::server::commands::spawn_dbt(
            link,
            crate::server::commands::DbtInvocation {
                kind,
                binary,
                project_root: root,
                project_name: ctx.project.name.clone(),
                selector,
                model_path: std::path::PathBuf::from(
                    cogitator_lsp::uri_to_fs_path(uri).unwrap_or_else(|| uri.to_string()),
                ),
                target,
                profiles_dir,
                profile,
                refresh,
                extra_args,
                capture_to,
            },
            self.jobs.clone(),
        );
        self.send(&ResultResponse::new(
            id,
            crate::server::jobs::JobStarted { token },
        ));
    }

    pub(super) fn extract_model_yaml(&mut self, id: i64, uri: &str) {
        let snap = self.snapshot_for(uri);
        let derived = self.derived.clone();
        let uri = uri.to_string();
        self.offload_blocking(id, "dbt.extractModelYaml", move || {
            let lineage = crate::server::handlers::commands::lineage_for(
                &derived,
                &snap.ctx,
                snap.doc.as_deref(),
                &uri,
            );
            crate::server::handlers::navigation::extract_model_yaml(
                &snap,
                &uri,
                (*lineage).as_ref(),
            )
        });
    }

    pub(super) fn go_to_tests(&mut self, id: i64, uri: &str) {
        let snap = self.snapshot_for(uri);
        let uri = uri.to_string();
        self.offload_blocking(id, "dbt.goToTests", move || {
            crate::server::handlers::navigation::go_to_tests(&snap, &uri)
        });
    }

    /// `dbt.status`: what the server knows and has been doing — project,
    /// dialect, active profile/target, backend, fusion, refresh stats.
    pub(super) fn status_report(&mut self, id: i64) {
        let ctx = self.snapshot();
        let cfg = self.profiles.lock().expect("profiles config lock").clone();
        let info = self.status.lock().expect("status lock").clone();
        let fusion = if self.fusion_path.is_empty() {
            "off"
        } else if self.fusion_enabled.load(Ordering::Relaxed) {
            "on"
        } else {
            "configured (validating or invalid)"
        };
        let report = crate::server::status::status_report(
            &info,
            &cfg,
            &ctx,
            fusion,
            self.progress_supported.load(Ordering::Relaxed),
            self.derived.report(),
            self.config_status
                .lock()
                .expect("config status lock")
                .clone(),
            self.extra.roots(),
            crate::server::status::PerfStatus::of(&self.perf_settings()),
        );
        self.send(&ResultResponse::new(id, report));
    }

    pub(super) fn profiles_report(&mut self, id: i64) {
        let snap = self.snapshot_for("");
        let derived = self.derived.clone();
        self.offload_blocking(id, "dbt.profiles", move || {
            crate::server::handlers::profiles::report(&snap, &derived)
        });
    }

    pub(super) fn set_profiles_dir(&mut self, id: i64, dir: String) {
        let live_cfg = self.profiles.clone();
        self.offload_blocking(id, "dbt.setProfilesDir", move || {
            let outcome = crate::server::handlers::profiles::set_profiles_dir(dir);
            if let Some(dir) = outcome.apply {
                let mut cfg = live_cfg.lock().expect("profiles config lock");
                cfg.dir_override = dir;
                cfg.profile.clear();
                cfg.target.clear();
            }
            outcome.reply
        });
    }

    pub(super) fn set_target(&mut self, id: i64, target: String, profile: Option<String>) {
        let snap = self.snapshot_for("");
        let derived = self.derived.clone();
        let live_cfg = self.profiles.clone();
        self.offload_blocking(id, "dbt.setTarget", move || {
            let outcome =
                crate::server::handlers::profiles::set_target(&snap, &derived, target, profile);
            if let Some((profile_override, target)) = outcome.apply {
                // tight std lock, no await in scope
                let mut live = live_cfg.lock().expect("profiles config lock");
                live.target = target;
                if let Some(profile) = profile_override {
                    live.profile = profile;
                }
            }
            outcome.reply
        });
    }
}
