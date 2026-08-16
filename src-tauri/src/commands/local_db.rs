//! Commands that touch only the local SQLite cache (and the app's own files).
//!
//! Nothing here reaches the network, so these work offline and without
//! Cloudflare credentials. Anything that also writes to the cloud lives in
//! `d1` or `r2` instead.

use sea_orm::{DatabaseConnection, TransactionTrait};
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;
use crate::db;
use crate::entities::post::Model as PostModel;
use crate::entities::{post_revision, post_stage};
use crate::entities::series::Model as SeriesModel;
use crate::error::{AppError, AppResult};
use crate::media_keys;
use crate::revisions;
use super::*;

// ─── Front matter ─────────────────────────────────────────────────────────────

/// Remove a leading YAML front-matter block, if the file opens with one.
///
/// Nothing reads front matter: the blog takes a post's metadata from D1 and
/// renders the body as given, so a block left in place publishes as a
/// horizontal rule followed by a heading made of the raw `title:`/`tags:`
/// lines. Imported files may still carry one from whatever wrote them, so it is
/// dropped on the way in.
///
/// The block must open on the very first line and close on a line of its own.
/// Without a closing delimiter the document is returned untouched, so a file
/// that merely starts with a `---` rule is not truncated.
fn strip_frontmatter(content: &str) -> &str {
    let Some(after_open) = content.strip_prefix("---") else {
        return content;
    };
    // The opening `---` has to be alone on its line, or it is a rule or a
    // setext heading underline rather than a delimiter.
    let Some(body) = after_open
        .strip_prefix('\n')
        .or_else(|| after_open.strip_prefix("\r\n"))
    else {
        return content;
    };

    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        if matches!(line.trim_end_matches(['\r', '\n']), "---" | "...") {
            return body[offset + line.len()..].trim_start_matches(['\r', '\n']);
        }
        offset += line.len();
    }
    content
}

#[cfg(test)]
mod tests {
    use super::strip_frontmatter;

    #[test]
    fn strips_a_leading_block() {
        let doc = "---\ntitle: My Post\ntags: rust, tauri\n---\n\nReal body.\n";
        assert_eq!(strip_frontmatter(doc), "Real body.\n");
    }

    #[test]
    fn handles_crlf_and_the_dots_terminator() {
        assert_eq!(strip_frontmatter("---\r\ntitle: x\r\n---\r\nBody\r\n"), "Body\r\n");
        assert_eq!(strip_frontmatter("---\ntitle: x\n...\nBody\n"), "Body\n");
    }

    /// A document that merely opens with a rule must survive intact — otherwise
    /// importing it would silently delete everything up to the next `---`.
    #[test]
    fn leaves_documents_without_a_closing_delimiter_alone() {
        let rule_only = "---\n\nJust a rule, then prose.\n";
        assert_eq!(strip_frontmatter(rule_only), rule_only);
    }

    #[test]
    fn leaves_ordinary_documents_alone() {
        for doc in ["# Heading\n\nBody\n", "Body only\n", "", "----\nnot a delimiter\n"] {
            assert_eq!(strip_frontmatter(doc), doc);
        }
    }

    #[test]
    fn an_empty_block_still_goes() {
        assert_eq!(strip_frontmatter("---\n---\nBody\n"), "Body\n");
    }
}

// ─── Slugs ────────────────────────────────────────────────────────────────────

/// A slug no post is using yet: `base`, else `base-2`, `base-3`, and so on.
///
/// Two files can perfectly well share a name — `notes.md` from two folders, or
/// the same document imported twice after an edit — and a slug is derived from
/// that name. Without this the second import writes `posts/<slug>.md` over the
/// first post's body and only *then* fails on the unique index, having already
/// destroyed the thing it collided with.
///
/// The search terminates: every taken candidate is a distinct existing row, so
/// it runs out after at most one step per post in the library.
async fn unique_slug(db: &DatabaseConnection, base: &str) -> AppResult<String> {
    if db::post_by_slug(db, base).await?.is_none() {
        return Ok(base.to_string());
    }
    // From 2, so the first duplicate reads as the second copy of the post.
    for n in 2.. {
        let candidate = format!("{base}-{n}");
        if db::post_by_slug(db, &candidate).await?.is_none() {
            return Ok(candidate);
        }
    }
    unreachable!("the loop returns once a candidate is free")
}

#[cfg(test)]
mod slug_tests {
    use super::*;

    fn draft(slug: &str) -> PostModel {
        PostModel {
            id: 0,
            slug: slug.to_string(),
            title: slug.to_string(),
            excerpt: None,
            tags: None,
            published: false,
            published_at: None,
            series_id: None,
            series_order: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[tokio::test]
    async fn a_free_slug_is_left_alone() {
        let db = db::connect_in_memory().await.unwrap();
        assert_eq!(unique_slug(&db, "my-post").await.unwrap(), "my-post");
    }

    /// The collision the import path used to resolve by overwriting the post it
    /// collided with.
    #[tokio::test]
    async fn a_taken_slug_moves_to_the_next_free_number() {
        let db = db::connect_in_memory().await.unwrap();

        db::create::<PostModel>(&db, draft("my-post")).await.unwrap();
        assert_eq!(unique_slug(&db, "my-post").await.unwrap(), "my-post-2");

        // Suffixed slugs are themselves posts, so the search steps past them.
        db::create::<PostModel>(&db, draft("my-post-2")).await.unwrap();
        assert_eq!(unique_slug(&db, "my-post").await.unwrap(), "my-post-3");

        // Nothing here touches an unrelated name.
        assert_eq!(unique_slug(&db, "other-post").await.unwrap(), "other-post");
    }

    /// Why the insert now runs before the file write: this is what actually
    /// refuses a duplicate, and it has to get its answer while the post it would
    /// collide with still has its body on disk.
    #[tokio::test]
    async fn the_database_refuses_a_duplicate_slug() {
        let db = db::connect_in_memory().await.unwrap();
        db::create::<PostModel>(&db, draft("taken")).await.unwrap();
        assert!(db::create::<PostModel>(&db, draft("taken")).await.is_err());
    }
}

/// Write a post's Markdown into the local cache, creating the directory.
async fn write_body(app: &tauri::AppHandle, slug: &str, body: &str) -> AppResult<()> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(AppError::AppDataDir)?
        .join("posts");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::io("Failed to create posts dir", e))?;
    tokio::fs::write(dir.join(format!("{slug}.md")), body)
        .await
        .map_err(|e| AppError::io("Failed to write local markdown", e))
}

// ─── Command ──────────────────────────────────────────────────────────────────

/// Open a native file picker and import the selected Markdown file as a draft.
///
/// The post is created locally only — body in the app's cache, metadata in the
/// local database, staged as a draft. Publishing or an explicit push is what
/// sends it to the cloud.
///
/// Returns the post title on success.
/// Returns `Err("cancelled")` when the user dismisses the dialog without
/// choosing a file — the frontend treats this differently from real errors.
#[tauri::command]
pub async fn import_article(
    app: tauri::AppHandle,
    conn: State<'_, DatabaseConnection>,
) -> AppResult<String> {
    // ── 1. File picker ────────────────────────────────────────────────────────
    // `blocking_pick_file` must not run on a tokio thread; use spawn_blocking.
    let app_clone = app.clone();
    let picked = tokio::task::spawn_blocking(move || {
        app_clone
            .dialog()
            .file()
            .add_filter("Markdown", &["md", "markdown"])
            .blocking_pick_file()
    })
    .await
    .map_err(|e| AppError::join("Dialog thread panicked", e))?;

    // Resolve to a PathBuf; return "cancelled" if the dialog was dismissed.
    let file_path = match picked {
        None => return Err(AppError::Cancelled),
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        Some(tauri_plugin_dialog::FilePath::Path(p)) => p,
        #[allow(unreachable_patterns)]
        Some(_) => return Err(AppError::UnsupportedPathFormat),
    };

    // ── 2. Read file ──────────────────────────────────────────────────────────
    let content = tokio::fs::read_to_string(&file_path)
        .await
        .map_err(|e| AppError::io("Failed to read file", e))?;

    // ── 3. Extract metadata ───────────────────────────────────────────────────
    // The file name is the only metadata an imported document carries that the
    // blog can use; tags are added afterwards in the app.
    let body = strip_frontmatter(&content);

    let stem = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled");

    let title = stem.to_string();
    let tags = String::new();

    // ── 4. Derive slug + R2 key ───────────────────────────────────────────────
    // The id is auto-assigned by the DB, so the R2 object key is keyed by slug.
    let now = now_ts();
    let base = {
        let s = slugify(&title);
        let s = if s.is_empty() { slugify(stem) } else { s };
        // Fall back to a unique, non-empty slug (e.g. non-ASCII titles).
        if s.is_empty() { format!("post-{now}") } else { s }
    };
    let slug = unique_slug(conn.inner(), &base).await?;

    // ── 5. Record the metadata locally ───────────────────────────────────────
    // Metadata first, body second. `slug` is unique, so the insert is what
    // ultimately decides whether this import may exist — and running it before
    // the file write means a slug that slipped past `unique_slug` (two imports
    // racing for the same free name) is refused while the post it collided with
    // still has its body.
    let post = PostModel {
        id: 0, // ignored on insert (auto-increment)
        slug,
        title: title.clone(),
        excerpt: None,
        tags: Some(tags_to_json(&tags)),
        published: false,
        published_at: None,
        series_id: None,
        series_order: None,
        created_at: now,
        updated_at: now,
    };
    let created = db::create::<PostModel>(conn.inner(), post).await?;

    // ── 6. Cache the body locally ────────────────────────────────────────────
    // An import lands as a draft, and a draft is local-only — the same rule
    // `save_post` follows. Nothing reaches R2 or D1 until the post is published
    // from the editor or pushed with "Push to cloud", so importing needs no
    // credentials and never puts an unpublished body in the bucket.
    if let Err(e) = write_body(&app, &created.slug, body).await {
        // Take the row back out rather than leave a post whose body never
        // landed: it would open as an empty document indistinguishable from one
        // genuinely written that way.
        let _ = db::delete::<PostModel>(conn.inner(), created.id).await;
        return Err(e);
    }

    // Imported posts start staged as Draft.
    db::stage_set(
        conn.inner(),
        post_stage::Model {
            post_id: created.id,
            stage: post_stage::DRAFT.to_string(),
            staged_at: now,
        },
    )
    .await?;
    // Local content the cloud has never seen — which is what an unsynced
    // fingerprint records.
    db::sync_set_local(
        conn.inner(),
        created.id,
        crate::sync_state::content_hash(&created, body),
    )
    .await?;

    Ok(title)
}

// ── Posts: local SQLite ─────────────────────────────────────────────────────────

#[tauri::command]
pub async fn create_post(
    conn: State<'_, DatabaseConnection>,
    post: PostModel,
) -> AppResult<PostModel> {
    let mut post = post;
    let now = now_ts();
    post.created_at = now;
    post.updated_at = now;
    let created = db::create::<PostModel>(conn.inner(), post).await?;
    // New posts start staged as Draft.
    db::stage_set(
        conn.inner(),
        post_stage::Model {
            post_id: created.id,
            stage: post_stage::DRAFT.to_string(),
            staged_at: now,
        },
    )
    .await?;
    Ok(created)
}

/// The library, trash excluded — what every screen except the trash view means
/// by "the posts".
#[tauri::command]
pub async fn list_posts(conn: State<'_, DatabaseConnection>) -> AppResult<Vec<PostModel>> {
    db::list_active_posts(conn.inner()).await
}

/// One post for the editor, refusing anything in the trash.
///
/// A trashed post is deleted as far as the app is concerned, but the editor can
/// still be pointed at one through a bookmark or browser history. Saying so is
/// better than opening it: an editor on a trashed post writes into the copy
/// being kept for recovery, and its Publish button puts a deleted post on the
/// blog.
#[tauri::command]
pub async fn get_post(
    conn: State<'_, DatabaseConnection>,
    id: i32,
) -> AppResult<Option<PostModel>> {
    let Some(post) = db::get::<PostModel>(conn.inner(), id).await? else {
        return Ok(None);
    };
    refuse_if_trashed(conn.inner(), &post).await?;
    Ok(Some(post))
}

#[tauri::command]
pub async fn update_post(
    conn: State<'_, DatabaseConnection>,
    post: PostModel,
) -> AppResult<PostModel> {
    // Every other write path refuses a trashed post; this one is the raw row
    // update, and leaving it open would make the rule a matter of which command
    // somebody happened to call.
    refuse_if_trashed(conn.inner(), &post).await?;
    let mut post = post;
    post.updated_at = now_ts();
    db::update::<PostModel>(conn.inner(), post).await
}

// ── Trash ───────────────────────────────────────────────────────────────────────

/// Move a post to the trash: it leaves every listing, and nothing else about it
/// changes.
///
/// This is what the delete button does. The body, the staging and sync rows and
/// the entire revision history stay exactly where they are, which is what makes
/// [`restore_post`] a single row deletion rather than a reconstruction.
///
/// **Nothing here reaches the cloud.** A published post that is trashed is still
/// on the blog and stays there; taking it down is `unpublish_post`, deliberately
/// and separately. Deleting a local copy and unpublishing are different
/// intentions, and a delete button that quietly did both would be the more
/// destructive of the two guesses.
#[tauri::command]
pub async fn trash_post(
    conn: State<'_, DatabaseConnection>,
    id: i32,
) -> AppResult<crate::entities::post_trash::Model> {
    // One transaction for the read and the write. A refresh deleting this post
    // — because the cloud no longer has it — checks the trash inside its own
    // transaction, so without this the two interleave: the post is read here,
    // the refresh commits the deletion, and the trash row lands afterwards
    // pointing at nothing. `post_trash` has no foreign key and the trash view
    // lists only rows that still have a post, so the result would be a success
    // message over a post that had been permanently deleted, and an orphan
    // nobody could see.
    let txn = conn.inner().begin().await?;

    // Refuse a post that is not there rather than writing a trash row pointing
    // at nothing, which would be invisible in every listing including the trash.
    db::get::<PostModel>(&txn, id)
        .await?
        .ok_or(AppError::PostNotFound(id))?;

    let trashed = db::trash_set(&txn, id, now_ts()).await?;
    txn.commit().await?;
    Ok(trashed)
}

/// Take a post back out of the trash, with everything it had when it went in.
///
/// Read and clear in one transaction, because Restore and Delete forever sit on
/// the same row. `purge` checks for the trash row inside *its* transaction, so
/// with two statements here the two overlap: purge commits between them,
/// `trash_clear` succeeds as a no-op, and this hands back a post whose row,
/// history and Markdown are gone. One transaction each means exactly one of the
/// two can win.
#[tauri::command]
pub async fn restore_post(conn: State<'_, DatabaseConnection>, id: i32) -> AppResult<PostModel> {
    let txn = conn.inner().begin().await?;
    let post = db::get::<PostModel>(&txn, id)
        .await?
        .ok_or(AppError::PostNotFound(id))?;
    db::trash_clear(&txn, id).await?;
    txn.commit().await?;
    Ok(post)
}

/// The trash, most recently thrown away first.
#[tauri::command]
pub async fn list_trashed_posts(
    conn: State<'_, DatabaseConnection>,
) -> AppResult<Vec<TrashedPost>> {
    Ok(db::list_trashed_posts(conn.inner())
        .await?
        .into_iter()
        .map(|(post, trash)| TrashedPost { trashed_at: trash.trashed_at, post })
        .collect())
}

/// A post in the trash, as the trash view reads it.
#[derive(serde::Serialize)]
pub struct TrashedPost {
    #[serde(flatten)]
    pub post: PostModel,
    /// Unix seconds. The view sorts and dates by this rather than by the post's
    /// own timestamps, which describe when it was written, not when it went.
    pub trashed_at: i64,
}

/// Delete a post from this machine for good: the row, the cached Markdown, the
/// staging and sync rows, and the revision history.
///
/// Only reachable from the trash, and only from a control that says what it
/// does. Everything else in this file can be walked back; this is the one thing
/// that cannot, which is why it is behind two deliberate steps rather than one.
///
/// **Local only, like trashing.** A post that is live on the blog stays live
/// after this — the local copy is what goes. Deleting the published article is
/// `unpublish_post` followed by the cloud's own delete, and doing it silently
/// from here would mean a control labelled "delete from this machine" quietly
/// editing the blog.
#[tauri::command]
pub async fn delete_post_permanently(
    app: tauri::AppHandle,
    conn: State<'_, DatabaseConnection>,
    id: i32,
) -> AppResult<()> {
    let post = db::get::<PostModel>(conn.inner(), id)
        .await?
        .ok_or(AppError::PostNotFound(id))?;
    purge(&app, conn.inner(), &post).await
}

/// Empty the trash, returning how many posts went.
///
/// One failure does not stop the rest: a body file that cannot be removed is
/// worth logging, not worth leaving the other twelve posts in a trash the person
/// has just asked to empty.
#[tauri::command]
pub async fn empty_trash(
    app: tauri::AppHandle,
    conn: State<'_, DatabaseConnection>,
) -> AppResult<usize> {
    let trashed = db::list_trashed_posts(conn.inner()).await?;
    let mut removed = 0usize;
    for (post, _) in trashed {
        // Re-read per post rather than trusted from the listing. Emptying the
        // trash walks it one post at a time while the trash view stays live and
        // interactive, so somebody can pull a post back out partway through —
        // and this loop, working from the snapshot it started with, would then
        // permanently delete the post they had just rescued.
        if db::trash_get(conn.inner(), post.id).await?.is_none() {
            log::info!("Post {} was restored while the trash was emptying; keeping it", post.id);
            continue;
        }
        match purge(&app, conn.inner(), &post).await {
            Ok(()) => removed += 1,
            Err(e) => log::error!("Could not permanently delete post {}: {e}", post.id),
        }
    }
    Ok(removed)
}

/// Remove every local trace of a post. Shared by the single and bulk deletes.
///
/// The row goes last. Everything before it is a side table keyed by the post's
/// id, and leaving one behind would attach a stranger's staging, sync record or
/// draft history to whichever post is assigned that id next.
async fn purge(
    app: &tauri::AppHandle,
    conn: &DatabaseConnection,
    post: &PostModel,
) -> AppResult<()> {
    // The body is moved aside *before* the transaction and removed after it —
    // the same staged-write idea `StagedBody` uses for saving, run backwards.
    //
    // Deleting the file first would mean a database error afterwards reporting
    // the deletion as unsuccessful with the text already gone: for a local-only
    // draft, content destroyed by an operation that said it had failed. But
    // deleting it *after* the commit is no better, because the commit frees the
    // slug: another window can create and save a post under the same name in
    // that gap, and the cleanup would take its body instead.
    //
    // A rename settles both. Nothing is destroyed before the commit — a failure
    // puts the file straight back — and the slug's file is already out of the
    // way before the slug becomes available, so no replacement post's Markdown
    // can be standing there when the removal happens.
    let staged = ArchivedBody::take(app, &post.slug).await?;

    let removed = async {
        let txn = conn.begin().await?;

        // The precondition, checked inside the transaction that acts on it.
        // Restore and Delete forever are two buttons on the same row, and the
        // list reloads asynchronously between them: without this, confirming a
        // deletion just after a restore would permanently delete the post that
        // was rescued. A check outside the transaction would only narrow that
        // window rather than close it.
        if db::trash_get(&txn, post.id).await?.is_none() {
            return Err(AppError::PostNotInTrash(post.slug.clone()));
        }

        db::stage_clear(&txn, post.id).await?;
        db::sync_clear(&txn, post.id).await?;
        db::revisions_clear(&txn, post.id).await?;
        db::trash_clear(&txn, post.id).await?;
        db::delete::<PostModel>(&txn, post.id).await?;
        // The one thing this deletion leaves behind, and the reason it can be
        // called permanent: the cloud's copy is untouched by design, so without
        // a record that this slug was deleted here the next refresh would pull
        // the post straight back in. See `post_tombstone`.
        db::tombstone_set(&txn, &post.slug, now_ts()).await?;
        txn.commit().await?;
        Ok::<(), AppError>(())
    }
    .await;

    match removed {
        Ok(()) => {
            staged.discard().await;
            Ok(())
        }
        // Nothing was deleted, so the post keeps its text.
        Err(e) => {
            staged.restore().await;
            Err(e)
        }
    }
}

/// A deleted post's Markdown, moved out of the way while its row is removed.
///
/// See [`purge`] for why neither ordering works without this: before the commit
/// the content must still be recoverable, and after it the slug is free for
/// somebody else's post to occupy.
struct ArchivedBody {
    /// Where the file went, and where it came from — `None` when the post had
    /// no cached body at all, which is an ordinary state for a post pulled from
    /// the cloud and never opened.
    moved: Option<(std::path::PathBuf, std::path::PathBuf)>,
}

impl ArchivedBody {
    /// Rename `posts/<slug>.md` aside. A missing file is not a failure.
    async fn take(app: &tauri::AppHandle, slug: &str) -> AppResult<Self> {
        if !media_keys::is_safe_slug(slug) {
            return Ok(Self { moved: None });
        }
        let dir = posts_dir(app).await?;
        let from = dir.join(format!("{slug}.md"));
        let to = dir.join(format!(".purge-{}.md.tmp", uuid::Uuid::new_v4().simple()));
        match tokio::fs::rename(&from, &to).await {
            Ok(()) => Ok(Self { moved: Some((from, to)) }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self { moved: None }),
            Err(e) => Err(AppError::io("Failed to set the post's body aside", e)),
        }
    }

    /// Put it back, because the deletion did not happen after all.
    ///
    /// Only onto an empty space. A deletion that failed because the post was
    /// restored leaves it live and editable, so an autosave or an MCP edit can
    /// have written a newer body by the time this runs — and a plain rename
    /// replaces files, which would lose that newer text under an older one that
    /// was only ever set aside. Where something is already there, it is by
    /// definition more recent than the copy this archived, and it wins.
    ///
    /// The check and the rename are not atomic, and cannot be: there is no
    /// rename-if-absent. It narrows the window to the width of one syscall
    /// pair, which is as far as the filesystem allows.
    async fn restore(self) {
        let Some((from, to)) = self.moved else { return };
        if tokio::fs::try_exists(&from).await.unwrap_or(false) {
            log::warn!(
                "A newer body was written for {} while it was being deleted; keeping it",
                from.display()
            );
            if let Err(e) = tokio::fs::remove_file(&to).await {
                log::warn!("Could not remove {}: {e}", to.display());
            }
            return;
        }
        if let Err(e) = tokio::fs::rename(&to, &from).await {
            log::error!("Could not put {} back after a failed deletion: {e}", from.display());
        }
    }

    /// Throw it away, the deletion having gone through.
    ///
    /// Best effort: the post is already gone and cannot come back, so a failure
    /// here is untidiness rather than something to report as the deletion having
    /// failed.
    async fn discard(self) {
        if let Some((_, to)) = self.moved {
            if let Err(e) = tokio::fs::remove_file(&to).await {
                log::warn!("Could not remove {}: {e}", to.display());
            }
        }
    }
}

/// Every post's sync state, keyed by post id — what the list needs to show
/// which published posts are carrying edits readers have not seen.
///
/// Returned for the whole library in one call rather than per post: the list
/// renders all of them at once, and two queries beat one per row.
#[tauri::command]
pub async fn list_sync_states(
    conn: State<'_, DatabaseConnection>,
) -> AppResult<Vec<PostSyncState>> {
    let stages: std::collections::HashMap<i32, post_stage::Model> = db::stages_all(conn.inner())
        .await?
        .into_iter()
        .map(|s| (s.post_id, s))
        .collect();
    let syncs: std::collections::HashMap<i32, crate::entities::post_sync::Model> =
        db::sync_all(conn.inner())
            .await?
            .into_iter()
            .map(|s| (s.post_id, s))
            .collect();

    Ok(db::list_active_posts(conn.inner())
        .await?
        .into_iter()
        .map(|post| PostSyncState {
            post_id: post.id,
            state: crate::sync_state::derive(stages.get(&post.id), syncs.get(&post.id)),
        })
        .collect())
}

/// One post's sync state, as the frontend reads it.
#[derive(serde::Serialize)]
pub struct PostSyncState {
    pub post_id: i32,
    pub state: crate::sync_state::SyncState,
}

// ── Revisions: local SQLite ─────────────────────────────────────────────────────

/// One entry in a post's history, as the editor's panel lists them.
///
/// Bodies are left out: a list of fifty versions of the same post would ship the
/// whole post fifty times to render a column of timestamps. `get_revision`
/// fetches the one the person actually opened.
#[derive(serde::Serialize)]
pub struct RevisionSummary {
    pub id: i32,
    pub post_id: i32,
    pub title: String,
    /// Why the snapshot was taken — one of the constants in
    /// [`crate::entities::post_revision`].
    pub origin: String,
    pub created_at: i64,
    /// Whether the post was live at the time. Shown because a version's text
    /// alone does not say whether readers were seeing it.
    pub published: bool,
    /// Characters of Markdown in the snapshot, or `None` when it carries no body
    /// at all — which the panel renders as "metadata only", since restoring one
    /// deliberately leaves the text alone.
    pub body_chars: Option<usize>,
}

impl From<crate::entities::post_revision::Model> for RevisionSummary {
    fn from(r: crate::entities::post_revision::Model) -> Self {
        Self {
            id: r.id,
            post_id: r.post_id,
            title: r.title,
            origin: r.origin,
            created_at: r.created_at,
            published: r.published,
            body_chars: r.body.as_deref().map(str::len),
        }
    }
}

/// A post's saved versions, newest first.
#[tauri::command]
pub async fn list_revisions(
    conn: State<'_, DatabaseConnection>,
    post_id: i32,
) -> AppResult<Vec<RevisionSummary>> {
    Ok(db::revisions_for_post(conn.inner(), post_id)
        .await?
        .into_iter()
        .map(RevisionSummary::from)
        .collect())
}

/// One saved version in full, body included — what the panel previews before
/// anyone commits to restoring it.
#[tauri::command]
pub async fn get_revision(
    conn: State<'_, DatabaseConnection>,
    revision_id: i32,
) -> AppResult<crate::entities::post_revision::Model> {
    db::revision_get(conn.inner(), revision_id)
        .await?
        .ok_or(AppError::RevisionNotFound(revision_id))
}

/// Put a post back to the version this snapshot holds.
///
/// Local only, and deliberately so: a restore writes the row and the cached
/// Markdown and stops there, leaving the post exactly as `modified` as any other
/// unpublished edit. Pushing it would publish a rollback nobody asked to
/// publish, from a button labelled "restore" — and would do it without the
/// approval gate for a post an MCP client had been editing.
///
/// **Restoring is itself an edit, so it takes its own snapshot first.** That is
/// what makes the operation reversible: restoring the wrong version leaves the
/// version you were on sitting at the top of the history, one click away. It is
/// also why nothing here deletes revisions — history only ever grows, until the
/// cap prunes its oldest end.
///
/// Unlike the edit paths, this snapshot is not best effort. A restore that could
/// not record where it came from would overwrite the current text with no way
/// back, which is precisely the failure this feature exists to prevent.
#[tauri::command]
pub async fn restore_revision(
    app: tauri::AppHandle,
    conn: State<'_, DatabaseConnection>,
    revision_id: i32,
) -> AppResult<PostModel> {
    let revision = db::revision_get(conn.inner(), revision_id)
        .await?
        .ok_or(AppError::RevisionNotFound(revision_id))?;
    let current = db::get::<PostModel>(conn.inner(), revision.post_id)
        .await?
        .ok_or(AppError::PostNotFound(revision.post_id))?;
    // A post in the trash keeps its history precisely so that restoring it
    // brings everything back — but the restoring happens after it comes out of
    // the trash, not into it.
    refuse_if_trashed(conn.inner(), &current).await?;

    // Where we are now, before it is replaced.
    revisions::snapshot(&app, conn.inner(), &current, post_revision::RESTORE).await?;

    // A snapshot with no body records metadata that was captured while the text
    // was not — see `post_revision::Model::body`. Restoring it must therefore
    // leave the text alone rather than blank the post, so the body that ends up
    // on disk is either the snapshot's or the one already there.
    let dir = posts_dir(&app).await?;
    let staged = match revision.body.as_deref() {
        Some(body) => Some((StagedBody::write(&dir, body).await?, body.to_string())),
        None => None,
    };

    // Re-checked inside the transaction that writes. The guard above ran before
    // the snapshot and the staged body, and another window can throw the post
    // away in that time — leaving Restore-from-trash handing back a version
    // changed after the deletion rather than the one discarded.
    let txn = conn.inner().begin().await?;
    if db::trash_get(&txn, current.id).await?.is_some() {
        if let Some((staged, _)) = staged {
            staged.discard().await;
        }
        return Err(AppError::PostInTrash(current.slug));
    }
    let restored = db::update::<PostModel>(
        &txn,
        PostModel { updated_at: now_ts(), ..revisions::apply(current.clone(), &revision) },
    )
    .await?;
    txn.commit().await?;

    let body = match staged {
        Some((staged, body)) => {
            if let Err(e) = staged.commit(&dir.join(format!("{}.md", restored.slug))).await {
                // The row is already back at the old version and its body is
                // not, so the row has to go forward again rather than sit there
                // describing text that was never written. Best effort: the
                // failure above is the one worth reporting.
                if let Err(undo) = db::update::<PostModel>(conn.inner(), current).await {
                    log::error!("Could not undo a restore whose body did not land: {undo}");
                }
                return Err(e);
            }
            body
        }
        // Nothing replaced the file, so what is on disk is what the fingerprint
        // below has to be taken over.
        None => revisions::cached_body(&app, &restored.slug).await.unwrap_or_default(),
    };

    // A restore changes what this machine holds and nothing else, which is
    // exactly what an unsynced fingerprint records. The stage is left alone on
    // purpose: a live post is still live, serving the version it was serving
    // before, and this rollback is one more edit waiting to be published.
    db::sync_set_local(
        conn.inner(),
        restored.id,
        crate::sync_state::content_hash(&restored, &body),
    )
    .await?;

    Ok(restored)
}

// ── Series: local SQLite ────────────────────────────────────────────────────────

#[tauri::command]
pub async fn create_series(
    conn: State<'_, DatabaseConnection>,
    series: SeriesModel,
) -> AppResult<SeriesModel> {
    let mut series = series;
    series.created_at = now_ts();
    db::create::<SeriesModel>(conn.inner(), series).await
}

#[tauri::command]
pub async fn list_series(conn: State<'_, DatabaseConnection>) -> AppResult<Vec<SeriesModel>> {
    db::list::<SeriesModel>(conn.inner()).await
}

#[tauri::command]
pub async fn get_series(
    conn: State<'_, DatabaseConnection>,
    id: i32,
) -> AppResult<Option<SeriesModel>> {
    db::get::<SeriesModel>(conn.inner(), id).await
}

#[tauri::command]
pub async fn update_series(
    conn: State<'_, DatabaseConnection>,
    series: SeriesModel,
) -> AppResult<SeriesModel> {
    db::update::<SeriesModel>(conn.inner(), series).await
}

#[tauri::command]
pub async fn delete_series(conn: State<'_, DatabaseConnection>, id: i32) -> AppResult<()> {
    db::delete::<SeriesModel>(conn.inner(), id).await
}

/// Set (or clear) a post's local staging stage without publishing.
#[tauri::command]
pub async fn set_post_stage(
    conn: State<'_, DatabaseConnection>,
    post_id: i32,
    stage: String,
) -> AppResult<post_stage::Model> {
    validate_stage(&stage)?;
    db::stage_set(
        conn.inner(),
        post_stage::Model { post_id, stage, staged_at: now_ts() },
    )
    .await
}

#[tauri::command]
pub async fn get_post_stage(
    conn: State<'_, DatabaseConnection>,
    post_id: i32,
) -> AppResult<Option<post_stage::Model>> {
    db::stage_get(conn.inner(), post_id).await
}

#[tauri::command]
pub async fn list_posts_by_stage(
    conn: State<'_, DatabaseConnection>,
    stage: String,
) -> AppResult<Vec<PostModel>> {
    validate_stage(&stage)?;
    db::posts_in_stage(conn.inner(), stage).await
}
