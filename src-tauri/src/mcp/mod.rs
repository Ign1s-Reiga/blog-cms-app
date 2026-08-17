//! A Model Context Protocol server exposing this blog to MCP clients.
//!
//! The app hosts a Streamable HTTP endpoint on loopback so an assistant (Claude
//! Desktop, Claude Code, …) can read the post library, draft, and edit — while
//! publishing stays behind a human approval in the app. [`server`] holds the
//! tools; [`publish`] holds the approval queue; this module owns the settings,
//! the listener, and the Tauri commands the Settings screen calls.
//!
//! ## Why loopback plus a bearer token
//!
//! The endpoint speaks for a signed-in app: it can read every draft and stage a
//! publish. Two things keep that from being reachable by anything else on the
//! machine's network:
//!
//! * the listener binds `127.0.0.1`, so it is not routable off-box, and rmcp's
//!   own `allowed_hosts` default rejects `Host` headers other than localhost,
//!   which is what stops a web page from DNS-rebinding onto it;
//! * every request must carry `Authorization: Bearer <token>`, which stops
//!   *other* local software from using it just by knowing the port.
//!
//! The token is generated on first use and kept in the OS keychain, falling back
//! to the settings file where no keychain is configured — the same split
//! [`crate::auth`] uses for the Cloudflare API token.

pub mod publish;
pub mod server;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{Request, State as AxumState};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};

use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};

use crate::commands;
use crate::db;
use crate::error::{AppError, AppResult};
use crate::entities::post::Model as PostModel;

/// Port used until someone changes it. Picked to sit clear of the dev server
/// (3000), Wrangler (8787), and the usual local-service range.
pub const DEFAULT_PORT: u16 = 4127;

/// Path the MCP endpoint is mounted at.
pub const ENDPOINT_PATH: &str = "/mcp";

/// Emitted whenever the publish queue changes, so the Settings screen can
/// refresh without polling.
pub const PUBLISH_EVENT: &str = "mcp://publish-requests-changed";

const KEYRING_SERVICE: &str = "blog-cms-app";
const KEYRING_USER: &str = "mcp-bearer-token";

// ─── Persisted settings ───────────────────────────────────────────────────────

/// `<app_data>/mcp.json`. The token is present only when no keychain accepted it.
#[derive(Default, Serialize, Deserialize)]
struct StoredMcp {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token: Option<String>,
}

fn settings_path(app: &tauri::AppHandle) -> AppResult<PathBuf> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(AppError::AppDataDir)?
        .join("mcp.json"))
}

fn load_stored(app: &tauri::AppHandle) -> StoredMcp {
    settings_path(app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_stored(app: &tauri::AppHandle, stored: &StoredMcp) -> AppResult<()> {
    let path = settings_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::io("Failed to create data dir", e))?;
    }
    let data =
        serde_json::to_string_pretty(stored).map_err(|e| AppError::json("Serialize failed", e))?;
    std::fs::write(&path, data).map_err(|e| AppError::io("Failed to write MCP settings", e))
}

// ─── Bearer token ─────────────────────────────────────────────────────────────

fn keyring_entry() -> Option<keyring_core::Entry> {
    keyring_core::Entry::new(KEYRING_SERVICE, KEYRING_USER).ok()
}

/// Store the token in the keychain. `false` means the caller must keep it in the
/// settings file instead.
fn keyring_set(token: &str) -> bool {
    match keyring_entry().map(|e| e.set_password(token)) {
        Some(Ok(())) => true,
        Some(Err(e)) => {
            log::warn!("Failed to store MCP token in keychain ({e}); using file fallback");
            false
        }
        None => false,
    }
}

/// The token, if one has ever been issued. Never creates one.
///
/// Kept separate from [`ensure_token`] so that merely looking at the Settings
/// screen does not mint a credential: a token exists only once the server has
/// actually been switched on, which is also the first moment it can be used.
fn load_token(app: &tauri::AppHandle) -> Option<String> {
    keyring_entry()
        .and_then(|e| e.get_password().ok())
        .or_else(|| load_stored(app).token)
}

/// The token, issuing and persisting one the first time it is genuinely needed
/// — starting the server, or an explicit rotation.
fn ensure_token(app: &tauri::AppHandle) -> AppResult<String> {
    if let Some(token) = load_token(app) {
        return Ok(token);
    }

    // 122 bits of randomness from the OS CSPRNG, hex-encoded — long enough that
    // guessing it is not a concern even though the endpoint answers fast.
    let token = uuid::Uuid::new_v4().simple().to_string();
    let mut stored = load_stored(app);
    stored.token = (!keyring_set(&token)).then(|| token.clone());
    save_stored(app, &stored)?;
    Ok(token)
}

/// Throw away the current token and issue a new one, invalidating every client
/// config that carried the old one.
fn rotate_token(app: &tauri::AppHandle) -> AppResult<String> {
    if let Some(entry) = keyring_entry() {
        let _ = entry.delete_credential();
    }
    let mut stored = load_stored(app);
    stored.token = None;
    save_stored(app, &stored)?;
    ensure_token(app)
}

// ─── Running server ───────────────────────────────────────────────────────────

struct Running {
    port: u16,
    /// Dropping or firing this ends `axum::serve`'s graceful shutdown future.
    shutdown: tokio::sync::oneshot::Sender<()>,
}

/// Managed state holding the listener, if one is up.
#[derive(Default)]
pub struct McpServer(Mutex<Option<Running>>);

impl McpServer {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Running>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Compare a presented secret without an early return on the first wrong byte.
///
/// The length check leaks only the token's length, which is a fixed constant.
/// An empty expectation never matches: a token that somehow came back blank must
/// fail every request rather than admit one that sent no header at all.
fn secret_eq(presented: &str, expected: &str) -> bool {
    let (a, b) = (presented.as_bytes(), expected.as_bytes());
    !b.is_empty() && a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Reject anything without the right bearer token before it reaches rmcp.
async fn require_bearer(
    AxumState(expected): AxumState<Arc<String>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();

    if secret_eq(presented, &expected) {
        Ok(next.run(request).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// The router the endpoint serves: the MCP service behind the bearer check.
///
/// Split out from [`start`] so `tests/mcp_endpoint.rs` can drive the auth gate
/// against a real HTTP server with a stub handler, which is the only way to
/// prove the layer actually wraps the nested service rather than sitting beside
/// it — a mistake that leaves the endpoint wide open while still compiling.
pub fn build_router<S, M>(service: StreamableHttpService<S, M>, token: Arc<String>) -> axum::Router
where
    S: rmcp::ServerHandler + Send + 'static,
    M: rmcp::transport::streamable_http_server::SessionManager,
{
    axum::Router::new()
        .nest_service(ENDPOINT_PATH, service)
        .layer(axum::middleware::from_fn_with_state(token, require_bearer))
}

/// Bring the endpoint up on `port`. Returns the port actually bound.
pub async fn start(app: &tauri::AppHandle, port: u16) -> AppResult<u16> {
    if let Some(running) = app.state::<McpServer>().lock().as_ref() {
        return Ok(running.port);
    }

    let token = Arc::new(ensure_token(app)?);

    // A fresh handler per session; the handle is all it needs, and it reads the
    // database connection out of managed state per call.
    let handle = app.clone();
    let service = StreamableHttpService::new(
        move || Ok(server::BlogMcp::new(handle.clone())),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    let router = build_router(service, token);

    // Loopback only. Binding 0.0.0.0 here would put every draft on the LAN.
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|source| AppError::Bind { port, source })?;
    let bound = listener
        .local_addr()
        .map_err(|e| AppError::io("Cannot read local address", e))?
        .port();

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tauri::async_runtime::spawn(async move {
        let served = axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = rx.await;
            })
            .await;
        if let Err(e) = served {
            log::error!("MCP server stopped: {e}");
        }
    });

    *app.state::<McpServer>().lock() = Some(Running { port: bound, shutdown: tx });
    log::info!("MCP server listening on http://127.0.0.1:{bound}{ENDPOINT_PATH}");
    Ok(bound)
}

/// Shut the endpoint down. `false` when nothing was running.
pub fn stop(app: &tauri::AppHandle) -> bool {
    let running = app.state::<McpServer>().lock().take();
    match running {
        Some(r) => {
            // The receiver is gone only if the task already exited, which is the
            // same end state, so a failed send is not an error.
            let _ = r.shutdown.send(());
            log::info!("MCP server stopped");
            true
        }
        None => false,
    }
}

/// Start the server at launch when it was left enabled.
pub fn init(app: &tauri::AppHandle) {
    let stored = load_stored(app);
    if !stored.enabled {
        return;
    }
    let app = app.clone();
    let port = stored.port.unwrap_or(DEFAULT_PORT);
    tauri::async_runtime::spawn(async move {
        if let Err(e) = start(&app, port).await {
            log::error!("MCP server failed to start: {e}");
        }
    });
}

/// Tell the frontend the publish queue moved.
pub fn notify_publish_change(app: &tauri::AppHandle) {
    let _ = app.emit(PUBLISH_EVENT, publish::list());
}

// ─── Commands ─────────────────────────────────────────────────────────────────

/// What the Settings screen renders.
#[derive(Serialize)]
pub struct McpStatus {
    /// Whether the server should run (persisted).
    pub enabled: bool,
    /// Whether it is actually listening right now.
    pub running: bool,
    pub port: u16,
    /// Full URL to paste into an MCP client.
    pub endpoint: String,
    /// The bearer token clients must send, or `None` before the server has ever
    /// been started — there is nothing to copy until one is issued.
    pub token: Option<String>,
}

#[tauri::command]
pub fn mcp_status(app: tauri::AppHandle) -> AppResult<McpStatus> {
    let stored = load_stored(&app);
    let state = app.state::<McpServer>();
    let running = state.lock();
    // Report the live port when listening, so a fallback or a since-edited
    // setting cannot make the displayed config point somewhere wrong.
    let port = running.as_ref().map(|r| r.port).or(stored.port).unwrap_or(DEFAULT_PORT);

    Ok(McpStatus {
        enabled: stored.enabled,
        running: running.is_some(),
        port,
        endpoint: format!("http://127.0.0.1:{port}{ENDPOINT_PATH}"),
        // Deliberately a read: opening Settings must not create a credential.
        token: load_token(&app),
    })
}

/// Turn the server on or off and set its port, persisting both.
#[tauri::command]
pub async fn mcp_configure(
    app: tauri::AppHandle,
    enabled: bool,
    port: u16,
) -> AppResult<McpStatus> {
    if port < 1024 {
        return Err(AppError::PortTooLow);
    }

    let mut stored = load_stored(&app);
    stored.enabled = enabled;
    stored.port = Some(port);
    save_stored(&app, &stored)?;

    // Always stop first: this is also how a port change takes effect.
    stop(&app);
    if enabled {
        start(&app, port).await?;
    }
    mcp_status(app)
}

/// Issue a new bearer token. Existing client configs stop working until updated.
#[tauri::command]
pub async fn mcp_regenerate_token(app: tauri::AppHandle) -> AppResult<McpStatus> {
    rotate_token(&app)?;

    // The running service captured the old token, so it has to be rebuilt.
    let stored = load_stored(&app);
    if stop(&app) {
        start(&app, stored.port.unwrap_or(DEFAULT_PORT)).await?;
    }
    mcp_status(app)
}

#[tauri::command]
pub fn mcp_list_publish_requests() -> Vec<publish::PublishRequest> {
    publish::list()
}

/// Approve a queued publish and run it — the only path from MCP to R2 and D1.
#[tauri::command]
pub async fn mcp_approve_publish(
    app: tauri::AppHandle,
    request_id: String,
) -> AppResult<publish::PublishRequest> {
    // Claiming marks the request `Publishing` under the queue's lock, so a
    // second approval arriving while the awaits below are running is refused
    // rather than publishing the same post twice.
    let request = publish::claim_for_publish(&request_id)?;
    notify_publish_change(&app);

    // Read the post fresh rather than trusting what was captured at request
    // time: the human is approving the post as it stands now.
    let outcome = async {
        let conn = app.state::<DatabaseConnection>();
        let post = db::get::<PostModel>(conn.inner(), request.post_id)
            .await?
            .ok_or(AppError::PostVanished(request.post_id))?;

        // A request can outlive the person's mind about the post: it is queued,
        // and the post is thrown away before anyone gets to the queue. Approving
        // it then would publish something that has been deleted here — and the
        // approval screen shows the request, not the trash, so there is nothing
        // on it to give that away.
        if db::trash_get(conn.inner(), post.id).await?.is_some() {
            return Err(AppError::PostInTrash(post.slug));
        }

        let body = commands::read_post_markdown(
            app.clone(),
            app.state::<DatabaseConnection>(),
            post.slug.clone(),
        )
        .await?;
        let tags = post
            .tags
            .as_deref()
            .and_then(|t| serde_json::from_str::<Vec<String>>(t).ok())
            .unwrap_or_default()
            .join(",");

        // The same command the editor's Publish button runs: images and body to
        // R2, metadata upserted to D1, stage recorded.
        commands::save_post(
            app.clone(),
            app.state::<DatabaseConnection>(),
            Some(post.id),
            post.title,
            tags,
            body,
            true,
        )
        .await
        .map(|_| ())
    }
    .await;

    // The queue records the failure as text: `PublishRequest` is a wire type
    // the Settings screen and MCP clients both read.
    let settled = publish::settle(&request_id, outcome.map_err(|e| e.to_string()))
        .ok_or_else(|| AppError::PublishRequestVanished(request_id.clone()))?;
    notify_publish_change(&app);
    Ok(settled)
}

#[tauri::command]
pub fn mcp_reject_publish(
    app: tauri::AppHandle,
    request_id: String,
) -> AppResult<publish::PublishRequest> {
    let rejected = publish::reject(&request_id)?;
    notify_publish_change(&app);
    Ok(rejected)
}

#[cfg(test)]
mod tests {
    use super::secret_eq;

    #[test]
    fn secret_comparison_accepts_only_an_exact_match() {
        assert!(secret_eq("abc123", "abc123"));
        assert!(!secret_eq("abc123", "abc124"));
        assert!(!secret_eq("abc123", "abc1234"));
        assert!(!secret_eq("", "abc123"));
        // An empty expectation must not turn a missing header into a pass.
        assert!(!secret_eq("anything", ""));
        assert!(!secret_eq("", ""));
    }
}
