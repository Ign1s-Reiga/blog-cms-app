//! Cloudflare credential storage + the app "session".
//!
//! Credentials are held in a process-global so the cloud commands can reach
//! them without threading state through every call, and mirrored to
//! `<app_data>/credentials.json` so they persist across launches. The session
//! is "authenticated" when a stored API token verifies against Cloudflare.
//!
//! Note: the token is stored in plaintext in the app data dir (user-scoped).
//! An OS keychain would be a better home — a worthwhile follow-up.

use std::path::PathBuf;
use std::sync::RwLock;

use serde::Serialize;
use tauri::Manager;

use crate::cloudflare::CloudflareConfig;

static CREDS: RwLock<Option<CloudflareConfig>> = RwLock::new(None);

/// Replace the in-memory credentials (used at startup and on login/logout).
pub fn set_creds(config: Option<CloudflareConfig>) {
    if let Ok(mut guard) = CREDS.write() {
        *guard = config;
    }
}

/// The current credentials, if signed in.
pub fn get_creds() -> Option<CloudflareConfig> {
    CREDS.read().ok()?.clone()
}

// ─── Disk persistence ─────────────────────────────────────────────────────────

fn creds_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Cannot resolve app data dir: {e}"))?
        .join("credentials.json"))
}

/// Load persisted credentials, if any (returns `None` on a missing/invalid file).
pub fn load_from_disk(app: &tauri::AppHandle) -> Option<CloudflareConfig> {
    let data = std::fs::read_to_string(creds_path(app).ok()?).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_to_disk(app: &tauri::AppHandle, config: &CloudflareConfig) -> Result<(), String> {
    let path = creds_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create data dir: {e}"))?;
    }
    let data = serde_json::to_string_pretty(config).map_err(|e| format!("Serialize failed: {e}"))?;
    std::fs::write(&path, data).map_err(|e| format!("Failed to write credentials: {e}"))
}

fn clear_disk(app: &tauri::AppHandle) -> Result<(), String> {
    match std::fs::remove_file(creds_path(app)?) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Failed to remove credentials: {e}")),
    }
}

// ─── Commands ──────────────────────────────────────────────────────────────────

/// Non-secret credential fields, for display/pre-fill (never the token).
#[derive(Serialize)]
pub struct PublicCreds {
    pub account_id: String,
    pub r2_bucket: String,
    pub d1_database_id: String,
}

/// Whether the app is signed in, plus whether credentials are stored at all.
#[derive(Serialize)]
pub struct SessionStatus {
    pub authenticated: bool,
    pub configured: bool,
    pub account_id: Option<String>,
}

#[tauri::command]
pub fn save_credentials(
    app: tauri::AppHandle,
    account_id: String,
    api_token: String,
    r2_bucket: String,
    d1_database_id: String,
) -> Result<(), String> {
    let config = CloudflareConfig { account_id, api_token, r2_bucket, d1_database_id };
    save_to_disk(&app, &config)?;
    set_creds(Some(config));
    Ok(())
}

#[tauri::command]
pub fn clear_credentials(app: tauri::AppHandle) -> Result<(), String> {
    clear_disk(&app)?;
    set_creds(None);
    Ok(())
}

#[tauri::command]
pub fn get_credentials() -> Option<PublicCreds> {
    get_creds().map(|c| PublicCreds {
        account_id: c.account_id,
        r2_bucket: c.r2_bucket,
        d1_database_id: c.d1_database_id,
    })
}

/// The "Checking session" call: verifies the stored API token against Cloudflare.
#[tauri::command]
pub async fn session_status() -> Result<SessionStatus, String> {
    match get_creds() {
        None => Ok(SessionStatus { authenticated: false, configured: false, account_id: None }),
        Some(c) => {
            let authenticated = crate::cloudflare::verify_token(&reqwest::Client::new(), &c.api_token)
                .await
                .unwrap_or(false);
            Ok(SessionStatus { authenticated, configured: true, account_id: Some(c.account_id) })
        }
    }
}
