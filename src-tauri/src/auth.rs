//! Cloudflare credential storage + the app "session".
//!
//! Credentials are held in a process-global so the cloud commands can reach
//! them without threading state through every call. For persistence they're
//! split by sensitivity:
//!   - the **API token** goes to the OS keychain via `keyring-core` (Windows
//!     Credential Manager on Windows);
//!   - the **non-secret** fields (account id, R2 bucket, D1 database id) are
//!     mirrored to `<app_data>/credentials.json`.
//!
//! On platforms without a keychain store configured, the token falls back to
//! the JSON file so the app still works — see [`init_keystore`].
//!
//! The session is "authenticated" when credentials are configured; token
//! validity surfaces when the app talks to R2/D1.

use std::path::PathBuf;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::cloudflare::CloudflareConfig;

static CREDS: RwLock<Option<CloudflareConfig>> = RwLock::new(None);

/// Keychain coordinates for the Cloudflare API token.
const KEYRING_SERVICE: &str = "blog-cms-app";
const KEYRING_USER: &str = "cloudflare-api-token";

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

// ─── OS keychain ────────────────────────────────────────────────────────────

/// Wire up the OS keychain backend. Call once at startup, before loading creds.
///
/// Windows uses the Credential Manager. Other platforms have no store yet, so
/// keychain reads/writes fail and the token falls back to the JSON file.
pub fn init_keystore() {
    #[cfg(windows)]
    match windows_native_keyring_store::Store::new() {
        Ok(store) => keyring_core::set_default_store(store),
        Err(e) => log::warn!("OS keychain unavailable ({e}); token will use file fallback"),
    }
}

fn keyring_entry() -> Option<keyring_core::Entry> {
    keyring_core::Entry::new(KEYRING_SERVICE, KEYRING_USER).ok()
}

/// Store the token in the keychain. Returns `true` on success, `false` when no
/// keychain is available (caller then falls back to the file).
fn keyring_set_token(token: &str) -> bool {
    match keyring_entry().map(|e| e.set_password(token)) {
        Some(Ok(())) => true,
        Some(Err(e)) => {
            log::warn!("Failed to store token in keychain ({e}); using file fallback");
            false
        }
        None => false,
    }
}

fn keyring_get_token() -> Option<String> {
    keyring_entry()?.get_password().ok()
}

fn keyring_delete_token() {
    if let Some(entry) = keyring_entry() {
        let _ = entry.delete_credential();
    }
}

// ─── Disk persistence ─────────────────────────────────────────────────────────

/// The credentials file: non-secret fields, plus the token only as a fallback
/// on platforms without a keychain (`api_token` absent when the keychain holds it).
#[derive(Serialize, Deserialize)]
struct StoredCreds {
    account_id: String,
    r2_bucket: String,
    d1_database_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    api_token: Option<String>,
}

fn creds_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Cannot resolve app data dir: {e}"))?
        .join("credentials.json"))
}

/// Load persisted credentials, if any (returns `None` on a missing/invalid file
/// or when no token can be found in the keychain or file).
pub fn load_from_disk(app: &tauri::AppHandle) -> Option<CloudflareConfig> {
    let data = std::fs::read_to_string(creds_path(app).ok()?).ok()?;
    let stored: StoredCreds = serde_json::from_str(&data).ok()?;
    // Prefer the keychain; fall back to the file's token if the keychain is empty.
    let api_token = keyring_get_token().or(stored.api_token)?;
    Some(CloudflareConfig {
        account_id: stored.account_id,
        api_token,
        r2_bucket: stored.r2_bucket,
        d1_database_id: stored.d1_database_id,
    })
}

fn save_to_disk(app: &tauri::AppHandle, config: &CloudflareConfig) -> Result<(), String> {
    // Keep the token out of the file whenever the keychain accepts it.
    let in_keyring = keyring_set_token(&config.api_token);
    let stored = StoredCreds {
        account_id: config.account_id.clone(),
        r2_bucket: config.r2_bucket.clone(),
        d1_database_id: config.d1_database_id.clone(),
        api_token: (!in_keyring).then(|| config.api_token.clone()),
    };

    let path = creds_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create data dir: {e}"))?;
    }
    let data = serde_json::to_string_pretty(&stored).map_err(|e| format!("Serialize failed: {e}"))?;
    std::fs::write(&path, data).map_err(|e| format!("Failed to write credentials: {e}"))
}

fn clear_disk(app: &tauri::AppHandle) -> Result<(), String> {
    keyring_delete_token();
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
    // Trim stray whitespace/newlines that sneak in when pasting values.
    let config = CloudflareConfig {
        account_id: account_id.trim().to_string(),
        api_token: api_token.trim().to_string(),
        r2_bucket: r2_bucket.trim().to_string(),
        d1_database_id: d1_database_id.trim().to_string(),
    };
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

/// The "Checking session" call. Fast by design: a session simply means
/// credentials are configured. Token validity surfaces when operations run, so
/// startup never blocks on the network.
#[tauri::command]
pub fn session_status() -> SessionStatus {
    match get_creds() {
        Some(c) => SessionStatus { authenticated: true, configured: true, account_id: Some(c.account_id) },
        None => SessionStatus { authenticated: false, configured: false, account_id: None },
    }
}
