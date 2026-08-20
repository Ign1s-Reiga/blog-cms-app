//! Cloudflare credential storage + the app "session".
//!
//! Credentials are held in a process-global so the cloud commands can reach
//! them without threading state through every call. For persistence they're
//! split by sensitivity:
//!   - the **API token** goes to the OS keychain via `keyring-core` — the
//!     Windows Credential Manager;
//!   - the **non-secret** fields (account id, R2 bucket, D1 database id) are
//!     mirrored to `<app_data>/credentials.json`.
//!
//! A keychain that refuses the token gets the JSON file instead, so the app
//! still works — see [`init_keystore`]. That fallback is about *saving*: the
//! file deliberately omits a token the keychain accepted, so one already in a
//! store that later cannot be opened is not recoverable from disk, and the app
//! asks for credentials again.
//!
//! The session is "authenticated" when credentials are configured; token
//! validity surfaces when the app talks to R2/D1.

use std::path::PathBuf;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::cloudflare::CloudflareConfig;
use crate::error::{AppError, AppResult};

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
/// The store is the Windows Credential Manager. If it cannot be opened, reads
/// and writes both fail — but only the write has somewhere else to go: the token
/// is saved to the JSON file instead. A token stored successfully on an earlier
/// run is in the keychain alone, so the same failure at read time leaves nothing
/// to load.
pub fn init_keystore() {
    #[cfg(windows)]
    match windows_native_keyring_store::Store::new() {
        Ok(store) => keyring_core::set_default_store(store),
        Err(e) => log::warn!("OS keychain unavailable ({e}); token will use file fallback"),
    }
}

/// A stored value, or the default when the file predates the field.
fn or_default(stored: String, default: &str) -> String {
    if stored.trim().is_empty() { default.to_string() } else { stored }
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
/// when no keychain took it (`api_token` absent when the keychain holds it).
#[derive(Serialize, Deserialize)]
struct StoredCreds {
    account_id: String,
    r2_bucket: String,
    d1_database_id: String,
    /// Absent in files written before publishing needed a public base URL;
    /// defaults to empty and is reported at publish time rather than here, so
    /// an existing sign-in keeps working for everything else.
    #[serde(default)]
    r2_public_url: String,
    /// Empty in files written before the layout was configurable; the defaults
    /// are applied on load so an existing sign-in keeps its current behaviour.
    #[serde(default)]
    thumbnail_key_pattern: String,
    #[serde(default)]
    media_key_pattern: String,
    /// Absent in files written before readership could be read. Empty means no
    /// site has been chosen, which the Analytics route reports rather than
    /// treating as no traffic.
    #[serde(default)]
    web_analytics_site_tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    api_token: Option<String>,
}

fn creds_path(app: &tauri::AppHandle) -> AppResult<PathBuf> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(AppError::AppDataDir)?
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
        r2_public_url: stored.r2_public_url,
        thumbnail_key_pattern: or_default(
            stored.thumbnail_key_pattern,
            crate::media_keys::DEFAULT_THUMBNAIL_PATTERN,
        ),
        media_key_pattern: or_default(
            stored.media_key_pattern,
            crate::media_keys::DEFAULT_MEDIA_PATTERN,
        ),
        web_analytics_site_tag: stored.web_analytics_site_tag,
    })
}

fn save_to_disk(app: &tauri::AppHandle, config: &CloudflareConfig) -> AppResult<()> {
    // Keep the token out of the file whenever the keychain accepts it.
    let in_keyring = keyring_set_token(&config.api_token);
    let stored = StoredCreds {
        account_id: config.account_id.clone(),
        r2_bucket: config.r2_bucket.clone(),
        d1_database_id: config.d1_database_id.clone(),
        r2_public_url: config.r2_public_url.clone(),
        thumbnail_key_pattern: config.thumbnail_key_pattern.clone(),
        media_key_pattern: config.media_key_pattern.clone(),
        web_analytics_site_tag: config.web_analytics_site_tag.clone(),
        api_token: (!in_keyring).then(|| config.api_token.clone()),
    };

    let path = creds_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::io("Failed to create data dir", e))?;
    }
    let data = serde_json::to_string_pretty(&stored)
        .map_err(|e| AppError::json("Serialize failed", e))?;
    std::fs::write(&path, data).map_err(|e| AppError::io("Failed to write credentials", e))
}

fn clear_disk(app: &tauri::AppHandle) -> AppResult<()> {
    keyring_delete_token();
    match std::fs::remove_file(creds_path(app)?) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(AppError::io("Failed to remove credentials", e)),
    }
}

// ─── Commands ──────────────────────────────────────────────────────────────────

/// Non-secret credential fields, for display/pre-fill (never the token).
#[derive(Serialize)]
pub struct PublicCreds {
    pub account_id: String,
    pub r2_bucket: String,
    pub d1_database_id: String,
    pub r2_public_url: String,
    pub thumbnail_key_pattern: String,
    pub media_key_pattern: String,
    pub web_analytics_site_tag: String,
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
    r2_public_url: String,
) -> AppResult<()> {
    // Trim stray whitespace/newlines that sneak in when pasting values.
    let config = CloudflareConfig {
        account_id: account_id.trim().to_string(),
        api_token: api_token.trim().to_string(),
        r2_bucket: r2_bucket.trim().to_string(),
        d1_database_id: d1_database_id.trim().to_string(),
        // Trailing slashes would double up when joined with an object key.
        r2_public_url: r2_public_url.trim().trim_end_matches('/').to_string(),
        thumbnail_key_pattern: crate::media_keys::DEFAULT_THUMBNAIL_PATTERN.to_string(),
        media_key_pattern: crate::media_keys::DEFAULT_MEDIA_PATTERN.to_string(),
        // Chosen later, on the Analytics route — signing in does not need it.
        web_analytics_site_tag: String::new(),
    };
    save_to_disk(&app, &config)?;
    set_creds(Some(config));
    Ok(())
}

/// Update the settings editable from the Settings screen, leaving the account
/// and API token untouched — the token lives in the keychain and should not
/// have to be re-pasted to change a URL.
///
/// Patterns are validated here rather than only in the UI, because a bad one
/// does not fail at publish: it writes objects to the wrong keys, and the blog
/// simply 404s.
#[tauri::command]
pub fn save_settings(
    app: tauri::AppHandle,
    r2_public_url: String,
    thumbnail_key_pattern: String,
    media_key_pattern: String,
    web_analytics_site_tag: Option<String>,
) -> AppResult<()> {
    use crate::media_keys::{validate_pattern, PatternKind};

    let mut config = get_creds().ok_or(AppError::NotConfigured)?;
    let public_url = r2_public_url.trim().trim_end_matches('/').to_string();
    if !public_url.is_empty() && !public_url.starts_with("http") {
        return Err(AppError::InvalidPublicUrl);
    }

    let thumbnail = thumbnail_key_pattern.trim().to_string();
    let media = media_key_pattern.trim().to_string();
    validate_pattern(&thumbnail, PatternKind::Thumbnail)?;
    validate_pattern(&media, PatternKind::Media)?;

    config.r2_public_url = public_url;
    config.thumbnail_key_pattern = thumbnail;
    config.media_key_pattern = media;
    // Absent leaves the stored tag alone. The Settings screen does not carry
    // the site picker — the Analytics route does — so a save from Settings
    // must not read as "no site chosen" and clear it.
    if let Some(tag) = web_analytics_site_tag {
        config.web_analytics_site_tag = tag.trim().to_string();
    }

    save_to_disk(&app, &config)?;
    set_creds(Some(config));
    Ok(())
}

#[tauri::command]
pub fn clear_credentials(app: tauri::AppHandle) -> AppResult<()> {
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
        r2_public_url: c.r2_public_url,
        thumbnail_key_pattern: c.thumbnail_key_pattern,
        media_key_pattern: c.media_key_pattern,
        web_analytics_site_tag: c.web_analytics_site_tag,
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
