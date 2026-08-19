//! The backend's error type.
//!
//! Every fallible path in the crate returns [`AppError`], so `?` composes across
//! the layers — R2/D1 in `cloudflare`, SQLite in `db`, the OS in `auth` — without
//! a `map_err` at each hop.
//!
//! ## What reaches the frontend
//!
//! Tauri requires a command's error type to be `Serialize`, and the frontend
//! reads what it gets with `String(err)`. [`AppError`] therefore serializes as
//! its `Display` text rather than as a struct: the typing is internal to Rust,
//! and the wire format is exactly the string the UI already renders.
//!
//! One string is load-bearing. [`AppError::Cancelled`] must display as precisely
//! `cancelled`, because `posts/page.tsx` and `media/page.tsx` compare against
//! that to tell a dismissed file picker from a real failure — a dismissed dialog
//! is not worth an error banner.
//!
//! ## Context on wrapped errors
//!
//! `std::io::Error` says *what* went wrong but never *which* file, and
//! `sea_orm::DbErr` the same for statements. Where the surrounding operation is
//! what makes the message useful, the variant carries a `context` string
//! ([`AppError::io`], [`AppError::json`]) rather than deriving `#[from]` and
//! reporting a bare "The system cannot find the path specified."
//!
//! Analytics is the one module that does not use this type: `AnalyticsError`
//! serializes structurally, because the UI switches on its `kind` to tell a
//! missing token permission from a real outage.

use serde::{Serialize, Serializer};

use crate::mcp::publish::PublishState;

/// A `Result` over [`AppError`], the return type of everything fallible here.
pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    // ─── Dialogs ──────────────────────────────────────────────────────────────
    /// A native file picker was dismissed.
    ///
    /// The exact text is a contract with the frontend — see the module docs.
    #[error("cancelled")]
    Cancelled,

    #[error("Unsupported path format on this platform")]
    UnsupportedPathFormat,

    // ─── Session / configuration ──────────────────────────────────────────────
    #[error("Not signed in to Cloudflare")]
    NotConfigured,

    #[error("Environment variable `{0}` is not set")]
    MissingEnv(&'static str),

    #[error("R2 Public URL must start with http:// or https://")]
    InvalidPublicUrl,

    /// A key pattern the Settings screen rejected. The message is the reason,
    /// phrased for display.
    #[error("{0}")]
    InvalidPattern(&'static str),

    #[error(
        "No R2 public URL is configured, so image links cannot be written. \
         Sign out and sign in again to set it."
    )]
    NoPublicUrl,

    // ─── Cloudflare ───────────────────────────────────────────────────────────
    /// The request never completed — offline, DNS, TLS.
    #[error("Cloudflare request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// A non-success HTTP status from R2 or D1. `service` is `"R2"` or `"D1"`,
    /// `op` the operation that drew it (`"upload"`, `"list"`, `"HTTP"`, …).
    #[error("{service} {op} error ({status}): {body}")]
    Cloudflare {
        service: &'static str,
        op: &'static str,
        status: reqwest::StatusCode,
        body: String,
    },

    /// HTTP 200 with `success: false` — Cloudflare's own error array.
    #[error("{service} {op} failed: {message}")]
    CloudflareApi {
        service: &'static str,
        op: &'static str,
        message: String,
    },

    #[error("R2 object is not valid UTF-8: {0}")]
    NotUtf8(#[from] std::string::FromUtf8Error),

    // ─── Storage ──────────────────────────────────────────────────────────────
    #[error(transparent)]
    Db(#[from] sea_orm::DbErr),

    /// Opening the local database or creating its tables, where naming the step
    /// is the whole diagnostic.
    #[error("{context}: {source}")]
    DbInit {
        context: &'static str,
        #[source]
        source: sea_orm::DbErr,
    },

    #[error("{context}: {source}")]
    Io {
        context: &'static str,
        #[source]
        source: std::io::Error,
    },

    #[error("{context}: {source}")]
    Json {
        context: &'static str,
        #[source]
        source: serde_json::Error,
    },

    #[error("Cannot resolve app data dir: {0}")]
    AppDataDir(#[source] tauri::Error),

    /// A `spawn_blocking` task that panicked (file dialogs, image encoding).
    #[error("{context}: {source}")]
    Join {
        context: &'static str,
        #[source]
        source: tokio::task::JoinError,
    },

    // ─── Posts ────────────────────────────────────────────────────────────────
    #[error("post {0} not found")]
    PostNotFound(i32),

    #[error("Invalid post slug: {0}")]
    InvalidSlug(String),

    /// The post's Markdown is not cached on this machine and there are no
    /// credentials to fetch it with — so its body is unknown, which is a
    /// different fact from it being empty.
    #[error(
        "The text of `{0}` is not on this machine and Cloudflare is not configured, so it \
         cannot be read. Sign in and try again."
    )]
    BodyUnavailable(String),

    #[error("Invalid stage `{0}` (expected `draft`, `published`, or `sync_failed`)")]
    InvalidStage(String),

    /// The local save succeeded and the cloud push did not, so the post exists
    /// locally and is staged `sync_failed`. The cause is kept as the source.
    #[error("post saved locally but publish sync failed: {0}")]
    PublishSyncFailed(#[source] Box<AppError>),

    #[error("post updated locally but cloud sync failed: {0}")]
    CloudSyncFailed(#[source] Box<AppError>),

    #[error("synced {synced}, {failed} failed to sync")]
    PartialSync { synced: usize, failed: usize },

    // ─── Media ────────────────────────────────────────────────────────────────
    #[error("Unsupported media type")]
    UnsupportedMedia,

    #[error("Unsupported image type: {0}")]
    UnsupportedImage(String),

    #[error("Unsupported thumbnail type")]
    UnsupportedThumbnail,

    #[error("Not a media library key: {0}")]
    NotAMediaKey(String),

    #[error("Media object has no extension: {0}")]
    MediaKeyHasNoExtension(String),

    #[error("Media object not found: {0}")]
    MediaNotFound(String),

    /// A deletion refused because posts still point at the object — or because
    /// some post's Markdown is not on this machine, so nothing can prove they
    /// do not. The counts are in the message so a caller that ignores the
    /// structure still says something useful.
    #[error(
        "{key} is used by {posts} post(s), and {unchecked_posts} post(s) could not be \
         checked; deleting it may break them"
    )]
    MediaInUse {
        key: String,
        posts: usize,
        unchecked_posts: usize,
    },

    #[error("{context}: {source}")]
    Image {
        context: &'static str,
        #[source]
        source: image::ImageError,
    },

    // ─── MCP endpoint ─────────────────────────────────────────────────────────
    #[error("Port must be 1024 or above")]
    PortTooLow,

    #[error("Cannot listen on 127.0.0.1:{port}: {source}")]
    Bind {
        port: u16,
        #[source]
        source: std::io::Error,
    },

    #[error("No publish request {0}")]
    NoPublishRequest(String),

    /// A claim or a rejection arrived for a request that is no longer waiting on
    /// a human — already publishing, or already finished.
    #[error("Publish request {id} is already {state:?}")]
    PublishRequestNotPending { id: String, state: PublishState },

    #[error("Publish request {0} vanished")]
    PublishRequestVanished(String),

    #[error("Post {0} no longer exists")]
    PostVanished(i32),

    #[error("Post `{0}` is in the trash, so it cannot be published")]
    PostInTrash(String),

    /// Permanent deletion asked for a post that is not in the trash — most
    /// likely restored between the click and the confirmation.
    #[error("Post `{0}` is not in the trash, so it was not deleted")]
    PostNotInTrash(String),

    #[error("Post `{0}` no longer exists in the cloud, so there is no cloud version to keep")]
    RemotePostGone(String),

    #[error("Post `{0}` is not in Cloudflare D1, so there was no cloud row to update")]
    RemotePostMissing(String),

    #[error("Post {0} is not in conflict")]
    NotConflicted(i32),

    // ─── Scheduled publishing ─────────────────────────────────────────────────
    #[error("Post `{0}` is already published, so there is nothing to schedule")]
    AlreadyPublished(String),

    #[error("A publication cannot be scheduled for a time that has already passed")]
    ScheduleInThePast(i64),

    #[error("Post `{0}` has no schedule")]
    NotScheduled(String),

    #[error(
        "Post `{0}` is no longer waiting to be published — it is being published now, \
         or already has been. Refresh to see where it stands."
    )]
    ScheduleNotPending(String),

    #[error(
        "Post `{0}` is scheduled to be published. Cancel the schedule first — \
         it runs in Cloudflare, so deleting the post here would not stop it."
    )]
    ScheduledPostCannotBeTrashed(String),

    // ─── Revisions ────────────────────────────────────────────────────────────
    /// A snapshot that is no longer in the table — most likely pruned by
    /// [`crate::db::REVISIONS_PER_POST`] while its row sat on screen.
    #[error("revision {0} not found")]
    RevisionNotFound(i32),

    // ─── Updater ──────────────────────────────────────────────────────────────
    #[error("Updater unavailable: {0}")]
    UpdaterUnavailable(#[source] tauri_plugin_updater::Error),

    #[error("Could not reach GitHub Releases: {0}")]
    UpdateCheck(#[source] tauri_plugin_updater::Error),

    #[error("Update failed: {0}")]
    UpdateFailed(#[source] tauri_plugin_updater::Error),

    #[error("Update state is poisoned")]
    UpdateStatePoisoned,

    #[error("No update is pending — run a check first")]
    NoPendingUpdate,
}

impl AppError {
    /// An I/O failure, labelled with the operation that caused it.
    pub fn io(context: &'static str, source: std::io::Error) -> Self {
        Self::Io { context, source }
    }

    /// A JSON encode/decode failure, labelled with what was being handled.
    pub fn json(context: &'static str, source: serde_json::Error) -> Self {
        Self::Json { context, source }
    }

    /// A database failure during startup, labelled with the step.
    pub fn db_init(context: &'static str, source: sea_orm::DbErr) -> Self {
        Self::DbInit { context, source }
    }

    /// A panic in a `spawn_blocking` task, labelled with what it was running.
    pub fn join(context: &'static str, source: tokio::task::JoinError) -> Self {
        Self::Join { context, source }
    }

    /// An image decode/encode failure, labelled with the direction.
    pub fn image(context: &'static str, source: image::ImageError) -> Self {
        Self::Image { context, source }
    }
}

/// Serialized as the `Display` text, so `String(err)` on the frontend reads the
/// same message it did when every command returned `Result<_, String>`.
impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frontend compares this message to decide whether a dismissed file
    /// picker deserves an error banner. Changing the text silently turns every
    /// cancelled dialog into a visible failure.
    #[test]
    fn cancelled_displays_as_the_string_the_frontend_matches() {
        assert_eq!(AppError::Cancelled.to_string(), "cancelled");
        assert_eq!(
            serde_json::to_string(&AppError::Cancelled).unwrap(),
            "\"cancelled\""
        );
    }

    /// Commands serialize as a bare JSON string, not as a tagged object — the
    /// shape the frontend has always received.
    #[test]
    fn errors_serialize_as_their_message() {
        let err = AppError::PostNotFound(7);
        assert_eq!(err.to_string(), "post 7 not found");
        assert_eq!(serde_json::to_string(&err).unwrap(), "\"post 7 not found\"");
    }

    /// A failed publish keeps the underlying cause in its message, so the banner
    /// still says what actually went wrong.
    #[test]
    fn wrapped_causes_stay_in_the_message() {
        let inner = AppError::Cloudflare {
            service: "R2",
            op: "upload",
            status: reqwest::StatusCode::FORBIDDEN,
            body: "denied".into(),
        };
        let err = AppError::PublishSyncFailed(Box::new(inner));
        assert_eq!(
            err.to_string(),
            "post saved locally but publish sync failed: R2 upload error (403 Forbidden): denied"
        );
    }

    /// Context is what makes an I/O message useful; the bare OS text does not
    /// say which file it was.
    #[test]
    fn io_errors_carry_the_operation() {
        let err = AppError::io(
            "Failed to write local markdown",
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
        );
        assert_eq!(err.to_string(), "Failed to write local markdown: denied");
    }
}
