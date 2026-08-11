//! In-app upgrades, backed by GitHub Releases.
//!
//! The release workflow publishes a signed `latest.json` manifest next to the
//! installers on every GitHub Release; `tauri.conf.json` points the updater at
//! that release's asset URL. Checking for a newer version is therefore a plain
//! HTTPS fetch of that manifest — no GitHub token, no API rate limit — and the
//! bundle's minisign signature is verified against the configured public key
//! before anything is written to disk.
//!
//! The flow is split into three commands so the UI can drive it step by step:
//!   1. [`check_for_update`] — fetch the manifest and compare versions;
//!   2. [`install_update`]  — download the bundle, emitting progress events;
//!   3. [`restart_app`]     — relaunch into the new version.
//!
//! Between (1) and (2) the resolved [`Update`] is parked in [`PendingUpdate`]
//! so the download reuses exactly the release the user was shown.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Emitted repeatedly while the bundle downloads.
const EVENT_PROGRESS: &str = "update://download-progress";
/// Emitted once the bundle is fully downloaded, before the installer runs.
const EVENT_FINISHED: &str = "update://download-finished";

/// The update resolved by the last [`check_for_update`], awaiting install.
#[derive(Default)]
pub struct PendingUpdate(Mutex<Option<Update>>);

// ─── Payloads ───────────────────────────────────────────────────────────────

/// Result of an update check. `available` is false when the app is current, in
/// which case the version fields are absent.
#[derive(Serialize)]
pub struct UpdateStatus {
    pub available: bool,
    pub current_version: String,
    pub version: Option<String>,
    pub notes: Option<String>,
    pub date: Option<String>,
}

#[derive(Clone, Serialize)]
struct DownloadProgress {
    downloaded: u64,
    /// Total bytes, when the server sends a content length.
    total: Option<u64>,
}

// ─── Commands ───────────────────────────────────────────────────────────────

/// Ask GitHub whether a newer release exists.
///
/// Returns `available: false` both when the app is up to date and when the
/// latest release predates the updater setup (no `latest.json` asset yet) — a
/// missing manifest is "nothing to update to", not an error worth showing.
#[tauri::command]
pub async fn check_for_update(
    app: AppHandle,
    pending: State<'_, PendingUpdate>,
) -> Result<UpdateStatus, String> {
    let current_version = app.package_info().version.to_string();

    let updater = app
        .updater()
        .map_err(|e| format!("Updater unavailable: {e}"))?;

    let found = match updater.check().await {
        Ok(found) => found,
        Err(tauri_plugin_updater::Error::ReleaseNotFound) => None,
        Err(e) => return Err(format!("Could not reach GitHub Releases: {e}")),
    };

    let Some(update) = found else {
        clear_pending(&pending);
        return Ok(UpdateStatus {
            available: false,
            current_version,
            version: None,
            notes: None,
            date: None,
        });
    };

    let status = UpdateStatus {
        available: true,
        current_version,
        version: Some(update.version.clone()),
        notes: update.body.clone(),
        // Just the calendar day — the manifest's timestamp is more precision
        // than a "released on" line needs.
        date: update.date.map(|d| d.date().to_string()),
    };

    if let Ok(mut slot) = pending.0.lock() {
        *slot = Some(update);
    }
    Ok(status)
}

/// Download and install the update found by [`check_for_update`].
///
/// Progress is reported on the [`EVENT_PROGRESS`] event. On Windows the
/// installer takes over once the download completes and terminates this
/// process, so callers should not expect this to return on that platform.
#[tauri::command]
pub async fn install_update(
    app: AppHandle,
    pending: State<'_, PendingUpdate>,
) -> Result<(), String> {
    // Clone the update out rather than borrowing it: the lock must not be held
    // across the download's await points. Leaving it parked also means a failed
    // download can be retried without running another check.
    let update = pending
        .0
        .lock()
        .map_err(|_| "Update state is poisoned".to_string())?
        .clone()
        .ok_or("No update is pending — run a check first")?;

    let mut downloaded: u64 = 0;
    let progress_app = app.clone();
    let finished_app = app.clone();

    let result = update
        .download_and_install(
            move |chunk, total| {
                downloaded += chunk as u64;
                let _ = progress_app.emit(EVENT_PROGRESS, DownloadProgress { downloaded, total });
            },
            move || {
                let _ = finished_app.emit(EVENT_FINISHED, ());
            },
        )
        .await;

    result.map_err(|e| format!("Update failed: {e}"))
}

/// Relaunch the app so the freshly installed version takes over.
#[tauri::command]
pub fn restart_app(app: AppHandle) {
    app.restart();
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn clear_pending(pending: &State<'_, PendingUpdate>) {
    if let Ok(mut slot) = pending.0.lock() {
        *slot = None;
    }
}
