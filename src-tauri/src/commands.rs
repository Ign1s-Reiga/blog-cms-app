//! Tauri commands, grouped by the store they act on.
//!
//! A command lives with the furthest-out store it writes: local-only in
//! `local_db`, anything reaching D1 in `d1`, anything reaching R2 in `r2`.
//! `save_post` touches all three and so sits in `r2`.
//!
//! The re-exports are globs by necessity: `#[tauri::command]` emits a
//! companion `__cmd__<name>` macro next to each function, and
//! `generate_handler!` needs both, so naming the functions individually
//! leaves the macros behind.

use sea_orm::DatabaseConnection;
use tauri::Manager;

use crate::cloudflare::{self, CloudflareConfig};
use crate::db;
use crate::entities::post::Model as PostModel;
use crate::entities::post_stage;
use crate::entities::series::Model as SeriesModel;
use crate::error::{AppError, AppResult};

mod d1;
mod local_db;
mod r2;

pub use d1::*;
pub use local_db::*;
pub use r2::*;

/// A post ready to send to D1, with its series reference translated out of
/// local ids.
///
/// **Every path that writes a post to the cloud goes through here** — the
/// editor's publish, the stage toggles, and the raw D1 commands. A local
/// `series_id` is a local primary key and means nothing in D1, so a path that
/// forgets this files the post under whichever unrelated remote series happens
/// to hold that number. Routing them all through one function is what keeps
/// that from depending on nobody forgetting.
///
/// It costs one extra D1 query per post pushed. `sync_posts` is the exception
/// and builds the map once for its whole batch; everywhere else pushes a single
/// post, where one query alongside the write is not worth optimising away.
async fn post_for_cloud(
    conn: &DatabaseConnection,
    client: &reqwest::Client,
    config: &CloudflareConfig,
    mut post: PostModel,
) -> AppResult<PostModel> {
    let remote_series = cloudflare::d1_list::<SeriesModel>(client, config).await?;
    db::SeriesMap::build(conn, &remote_series)
        .await?
        .apply_outbound(&mut post);
    Ok(post)
}

// ─── Shared helpers ───────────────────────────────────────────────────────────
//
// Only helpers with more than one caller across the modules above. Anything
// used by a single module lives in that module instead.

/// A post body written beside its destination, waiting to be moved into place.
///
/// SQLite and the filesystem cannot share a transaction, so a save is a commit
/// *sequence*, and the order is chosen to make the gap between the two stores as
/// small as the platform allows. Writing the bytes is the step that actually
/// fails — a full disk, a read-only volume, an antivirus holding the file — so
/// it happens first, before anything is committed. What remains is a rename
/// within one directory, which is atomic: a reader never observes a half-written
/// body, and the post's old body stays live right up until the new one is whole.
///
/// Shared because restoring a revision replaces a body for the same reasons a
/// save does, and a rollback that could leave a half-written file would defeat
/// the point of having a history at all.
pub(crate) struct StagedBody {
    temp: std::path::PathBuf,
}

impl StagedBody {
    /// Write `body` to a temporary file in `dir`, which must be the directory it
    /// will eventually be renamed into — a rename is only atomic within one
    /// filesystem.
    ///
    /// The name is dotted and uuid-suffixed so a crash between here and the
    /// rename leaves something recognisable as debris rather than something the
    /// editor might list as a post.
    pub(crate) async fn write(dir: &std::path::Path, body: &str) -> AppResult<Self> {
        let temp = dir.join(format!(".save-{}.md.tmp", uuid::Uuid::new_v4().simple()));
        tokio::fs::write(&temp, body)
            .await
            .map_err(|e| AppError::io("Failed to write local markdown", e))?;
        Ok(Self { temp })
    }

    /// Move the staged body onto `dest`, replacing whatever is there. The
    /// temporary file is cleaned up either way.
    pub(crate) async fn commit(self, dest: &std::path::Path) -> AppResult<()> {
        match tokio::fs::rename(&self.temp, dest).await {
            Ok(()) => Ok(()),
            Err(e) => {
                self.discard().await;
                Err(AppError::io("Failed to move the saved markdown into place", e))
            }
        }
    }

    /// Throw the staged body away. Best effort: a leftover temporary file is
    /// untidy, and the failure that led here is the one worth reporting.
    async fn discard(self) {
        if let Err(e) = tokio::fs::remove_file(&self.temp).await {
            log::warn!("Could not remove staged body {}: {e}", self.temp.display());
        }
    }
}

/// Serialises the moment a post's cached Markdown is replaced.
///
/// A body lives in two stores that cannot share a transaction, and `StagedBody`
/// makes each *swap* atomic without making the pair of them consistent. Every
/// writer here follows the same sequence — decide from the database, then move a
/// file into place — and two of those interleaved can leave the file from one
/// with the database of the other.
///
/// The costly case is a read of a post the cloud has moved on from. It decides
/// the cached copy cannot be trusted, then spends a network round trip fetching
/// the published version — and a save landing inside that round trip is a draft
/// the reader is about to write over with an older copy, having decided before
/// the draft existed. Re-asking the question is not enough on its own: the
/// answer has to still be true when the rename happens, which is what holding
/// this across both steps buys.
///
/// One lock for all posts rather than one per slug. These are local file moves
/// measured in milliseconds, an author edits one post at a time, and nothing
/// here is held across network I/O — every holder releases before it uploads.
/// A map of per-slug locks would be more code for contention that does not
/// exist.
static BODY_COMMITS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Take the body-commit lock. See [`BODY_COMMITS`].
///
/// Hold it from the database write through the rename that matches it, and drop
/// it before anything slow. Never acquired twice on one path.
pub(crate) async fn lock_body_commits() -> tokio::sync::MutexGuard<'static, ()> {
    BODY_COMMITS.lock().await
}

/// The directory holding every post's cached Markdown, created if it is not
/// there yet.
async fn posts_dir(app: &tauri::AppHandle) -> AppResult<std::path::PathBuf> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(AppError::AppDataDir)?
        .join("posts");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::io("Failed to create posts dir", e))?;
    Ok(dir)
}

/// Refuse an operation on a post that is in the trash.
///
/// The trash is a deletion as far as the rest of the app is concerned, and the
/// editor can still be pointed at one — a bookmark, browser history, a tab left
/// open when the post was thrown away. Without this, that editor would happily
/// autosave into the copy being kept for recovery, and Publish would put a
/// deleted post on the blog.
///
/// Checked per operation rather than once at load: the post can be trashed from
/// the posts list while an editor is open on it, and the answer that matters is
/// the one at the moment of the write.
async fn refuse_if_trashed(conn: &DatabaseConnection, post: &PostModel) -> AppResult<()> {
    if db::trash_get(conn, post.id).await?.is_some() {
        return Err(AppError::PostInTrash(post.slug.clone()));
    }
    Ok(())
}

/// Current time as a Unix timestamp in seconds (the schema's date encoding).
fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Turn arbitrary text into a URL-safe slug: lowercase alphanumerics, other runs
/// collapsed to single hyphens, no leading/trailing hyphens.
fn slugify(input: &str) -> String {
    let mut slug = String::new();
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_string()
}

/// Encode a comma-separated tag string as a JSON array (the `tags` column shape).
fn tags_to_json(csv: &str) -> String {
    let list: Vec<&str> = csv
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string())
}

fn validate_stage(stage: &str) -> AppResult<()> {
    match stage {
        post_stage::DRAFT | post_stage::PUBLISHED | post_stage::SYNC_FAILED => Ok(()),
        other => Err(AppError::InvalidStage(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::StagedBody;

    /// A scratch directory of this test's own, removed on the way out.
    async fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("blog-cms-{label}-{}", uuid::Uuid::new_v4().simple()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        dir
    }

    /// The ordinary path: the body lands whole and the temporary file is gone.
    #[tokio::test]
    async fn a_staged_body_replaces_the_destination_and_leaves_no_debris() {
        let dir = temp_dir("staged").await;
        let dest = dir.join("post.md");
        tokio::fs::write(&dest, "old body").await.unwrap();

        StagedBody::write(&dir, "new body")
            .await
            .unwrap()
            .commit(&dest)
            .await
            .unwrap();

        assert_eq!(tokio::fs::read_to_string(&dest).await.unwrap(), "new body");
        assert_eq!(remaining(&dir).await, vec!["post.md"]);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    /// The failure this whole sequence exists for. The rename cannot happen, so
    /// the post's old body must still be the one on disk — and the staged copy
    /// must not be left lying around.
    #[tokio::test]
    async fn a_body_that_cannot_be_moved_into_place_leaves_the_old_one_live() {
        let dir = temp_dir("blocked").await;
        // A directory cannot be replaced by a rename on either platform, which
        // is the most portable way to make the move fail.
        let dest = dir.join("post.md");
        tokio::fs::create_dir(&dest).await.unwrap();
        tokio::fs::write(dest.join("marker"), "still here").await.unwrap();

        let staged = StagedBody::write(&dir, "new body").await.unwrap();
        assert!(staged.commit(&dest).await.is_err());

        // The destination is untouched, and only it remains.
        assert_eq!(
            tokio::fs::read_to_string(dest.join("marker")).await.unwrap(),
            "still here"
        );
        assert_eq!(remaining(&dir).await, vec!["post.md"]);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn a_discarded_body_is_removed() {
        let dir = temp_dir("discard").await;
        StagedBody::write(&dir, "unwanted").await.unwrap().discard().await;
        assert!(remaining(&dir).await.is_empty());
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    /// Everything left in `dir`, sorted — the staged body is named so that a
    /// leak shows up here.
    async fn remaining(dir: &Path) -> Vec<String> {
        let mut entries = tokio::fs::read_dir(dir).await.unwrap();
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        names
    }
}
