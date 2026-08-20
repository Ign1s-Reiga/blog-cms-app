//! Commands that read or write objects in R2.
//!
//! `save_post` touches all three stores; by the same rule as `d1` it lives
//! here, with the body and image handling that is its distinctive work.

use std::ffi::OsStr;
use std::path::{Component, Path};

use serde::Serialize;
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;
use sea_orm::{DatabaseConnection, TransactionTrait};
use crate::cloudflare::{self, cf};
use crate::db;
use crate::entities::post::Model as PostModel;
use crate::entities::{post_revision, post_schedule, post_stage};
use crate::error::{AppError, AppResult};
use crate::imaging::{self, StagedImage};
use crate::media_keys;
use crate::media_usage;
use crate::revisions;
use crate::sync_state;
use super::*;

/// A safe single path segment for the local media cache: one ordinary file
/// name, and nothing else.
///
/// Spelling the rejections out by hand (`/`, `\`, `..`) misses what a platform
/// does with the rest. On Windows a drive-relative name like `C:x` carries a
/// path prefix, so joining it onto a directory *replaces* that directory rather
/// than descending into it. Asking the path parser instead makes the answer
/// whatever the platform itself would do: exactly one `Normal` component,
/// spelled the way it came in.
fn is_safe_file_name(name: &str) -> bool {
    // A backslash is an ordinary character in a Unix file name, so the parser
    // there would accept `a\b` as one component. Refuse it everywhere: these
    // names travel between machines through R2 keys and Markdown bodies.
    if name.contains('/') || name.contains('\\') {
        return false;
    }
    let mut components = Path::new(name).components();
    matches!(components.next(), Some(Component::Normal(c)) if c == OsStr::new(name))
        && components.next().is_none()
}

/// MIME type to send when uploading a file with this (lowercase) extension.
fn content_type_for(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        _ => "application/octet-stream",
    }
}

/// Unique `assets/<file>` references in a Markdown body — the local image paths
/// the editor inserts on drag-and-drop.
fn extract_asset_refs(body: &str) -> Vec<String> {
    let mut refs: Vec<String> = Vec::new();
    let mut rest = body;
    while let Some(pos) = rest.find("assets/") {
        let after = &rest[pos..];
        let end = after
            .find(|c: char| c == ')' || c == ']' || c == '"' || c == '\'' || c.is_whitespace())
            .unwrap_or(after.len());
        let r = &after[..end];
        if r.len() > "assets/".len() && !refs.iter().any(|x| x == r) {
            refs.push(r.to_string());
        }
        rest = &after[end..];
    }
    refs
}

/// The file name and bytes an `assets/<file>` reference points at, or `None`
/// when there is no such file — or when the reference is not one this post may
/// publish.
///
/// A body is no longer necessarily something a human typed: MCP clients write
/// them too, and publishing uploads whatever these references resolve to into a
/// public bucket. `assets_dir.join(name)` on an unchecked name is therefore a
/// file-read primitive pointed at the whole disk — `assets/../../.env` would
/// upload credentials to the blog. Two checks stand in the way:
///
/// 1. the reference must be a plain file name, which is all the editor and
///    `stage_media_from_library` ever produce; and
/// 2. the resolved path must still sit inside the assets directory, which is
///    what a name check alone cannot tell you about a symlink.
///
/// An unusable reference is skipped rather than failed, the same way a missing
/// asset already was: it publishes as a dead link, which is a great deal better
/// than a post that cannot be published at all because of one stale image.
async fn read_staged_asset<'a>(
    assets_dir: &Path,
    reference: &'a str,
) -> Option<(&'a str, Vec<u8>)> {
    let Some(file_name) = reference
        .strip_prefix("assets/")
        .filter(|name| is_safe_file_name(name))
    else {
        log::warn!("Ignoring asset reference that is not a plain file name: {reference}");
        return None;
    };

    // Both sides are canonicalized so the comparison is between two real,
    // fully-resolved paths — on Windows that also puts both in the same
    // verbatim (`\\?\`) form.
    let resolved = tokio::fs::canonicalize(assets_dir.join(file_name)).await.ok()?;
    let root = tokio::fs::canonicalize(assets_dir).await.ok()?;
    if !resolved.starts_with(&root) {
        log::warn!("Ignoring asset reference resolving outside the assets dir: {reference}");
        return None;
    }

    Some((file_name, tokio::fs::read(&resolved).await.ok()?))
}

// ─── Local save ───────────────────────────────────────────────────────────────

/// Write the post's row — and, for a draft, its staging row — in one
/// transaction, so a save cannot record the post without recording what stage
/// it is in.
///
/// A publish's stage is deliberately left out: it is not known yet. It depends
/// on whether the upload that follows succeeds, and is written once that is
/// settled.
async fn commit_metadata(
    conn: &DatabaseConnection,
    model: PostModel,
    existing: bool,
    draft_staged_at: Option<i64>,
) -> AppResult<PostModel> {
    let txn = conn.begin().await?;

    // The trash check that matters, because it is the one inside the
    // transaction that writes. The guard at the top of `save` runs several
    // awaits earlier — a body staged, a snapshot taken — and a post can be
    // thrown away in between.
    let saved = if existing {
        if db::trash_get(&txn, model.id).await?.is_some() {
            return Err(AppError::PostInTrash(model.slug));
        }
        db::update::<PostModel>(&txn, model).await?
    } else {
        db::create::<PostModel>(&txn, model).await?
    };

    if let Some(staged_at) = draft_staged_at {
        db::stage_set(
            &txn,
            post_stage::Model {
                post_id: saved.id,
                stage: post_stage::DRAFT.to_string(),
                staged_at,
            },
        )
        .await?;
    }

    txn.commit().await?;
    Ok(saved)
}

/// A post as it stood before the save — everything needed to put it back.
///
/// The stage travels with the row because the two can move together: saving a
/// published post as a draft rewrites both, so restoring only the row would
/// leave a `published` post staged `draft`.
#[derive(Clone)]
struct PreviousState {
    post: PostModel,
    stage: Option<post_stage::Model>,
}

impl PreviousState {
    /// Read a post and its stage as they currently stand.
    async fn read(conn: &DatabaseConnection, id: i32) -> AppResult<Self> {
        Ok(Self {
            post: db::get::<PostModel>(conn, id)
                .await?
                .ok_or(AppError::PostNotFound(id))?,
            stage: db::stage_get(conn, id).await?,
        })
    }
}

/// Undo a committed metadata write whose body then could not be moved into
/// place, so the editor is not left showing a saved title over a stale body.
///
/// The two cases differ, and neither is optional. A **new** post has no earlier
/// version to fall back to, so its row and stage go entirely — otherwise the
/// list gains a post whose body was never written. An **existing** post is put
/// back exactly as it was read, row and stage together, which is why
/// [`PreviousState`] is kept alive across the save.
///
/// Only *this* save is undone. The editor and an MCP client can both be saving
/// the same post — `mcp_approve_publish` calls this command with a post id the
/// editor may be writing at that moment — so between our metadata commit and
/// this compensation another save can commit its own metadata *and* land its
/// body. Restoring our snapshot then would revert metadata that is not ours to
/// revert, and leave it describing someone else's body. Their save is newer and
/// complete; ours is the one that failed, so it yields.
///
/// Best effort by necessity: if the compensating write also fails there is
/// nothing further to try, and the original error is the one the user needs.
async fn restore_metadata(
    conn: &DatabaseConnection,
    previous: Option<PreviousState>,
    saved: &PostModel,
) {
    let saved_id = saved.id;
    let undone = async {
        let txn = conn.begin().await?;

        // Re-read inside the transaction: if the row is no longer the one we
        // wrote, someone has saved since and this rollback is not ours to make.
        // A concurrent commit landing between this read and the writes below is
        // still possible in principle — narrowing the window is what is on offer
        // here, not closing it.
        if db::get::<PostModel>(&txn, saved_id).await?.as_ref() != Some(saved) {
            log::warn!("Not rolling back post {saved_id}: it has been saved again since");
            return Ok(());
        }

        match previous {
            Some(before) => {
                db::update::<PostModel>(&txn, before.post).await?;
                match before.stage {
                    Some(stage) => db::stage_set(&txn, stage).await.map(|_| ())?,
                    None => db::stage_clear(&txn, saved_id).await?,
                }
            }
            None => {
                db::stage_clear(&txn, saved_id).await?;
                db::delete::<PostModel>(&txn, saved_id).await?;
            }
        }
        txn.commit().await?;
        Ok::<(), AppError>(())
    }
    .await;

    if let Err(e) = undone {
        log::error!("Could not roll back post {saved_id} after a failed body write: {e}");
    }
}

// ─── Post content ───────────────────────────────────────────────────────────

/// Read a post's Markdown body (by slug) for the editor.
///
/// Prefers the local cache (`<app_data>/posts/<slug>.md`). If it isn't cached
/// locally but exists on R2, it's downloaded and cached so the editor can open
/// it offline next time. Returns an empty string when the post has no content
/// yet (nothing local and nothing on R2), or when the cloud is unreachable.
///
/// Keyed by slug (not id) so it works for posts sourced from D1, whose ids don't
/// match the local cache.
/// Does the cached body for `slug` hold work this machine has not pushed?
///
/// The question every replacement of a cached body has to ask first, because a
/// "no" is permission to write the cloud's copy over what is on disk.
///
/// Errors are returned rather than absorbed. This lookup is the only thing
/// standing between a draft and the older published version of it, so a database
/// that cannot answer must stop the read — treating "I do not know" as "nothing
/// local" is the one reading that loses the author's work. A post the local
/// database has never heard of is a genuine no: there is no draft to protect.
/// What a cached body looked like at a moment in time — modification time and
/// length — for comparing against itself later.
///
/// `None` for a file that is not there, which is a state worth telling apart:
/// one appearing where there was nothing is exactly as much of a write as one
/// being replaced.
///
/// Deliberately a property of the file rather than of the database. Every other
/// signal that a body has been written — the sync fingerprint, the staleness
/// mark — is bookkeeping recorded *after* the file moves, and each of those
/// writes can fail on its own, leaving the text on disk newer than anything that
/// describes it. The file is the thing being protected, so it is the thing to
/// ask.
async fn body_stamp(path: &std::path::Path) -> Option<(std::time::SystemTime, u64)> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

async fn has_local_edits(conn: &DatabaseConnection, slug: &str) -> AppResult<bool> {
    let Some(post) = db::post_by_slug(conn, slug).await? else {
        return Ok(false);
    };
    Ok(db::sync_get(conn, post.id)
        .await?
        .is_some_and(|row| sync_state::local_changed(&row)))
}

#[tauri::command]
pub async fn read_post_markdown(
    app: tauri::AppHandle,
    conn: State<'_, DatabaseConnection>,
    slug: String,
) -> AppResult<String> {
    // The slug builds a local file path and an R2 key, so reject anything that
    // isn't a strict slug (guards against path traversal / injection).
    if !media_keys::is_safe_slug(&slug) {
        return Err(AppError::InvalidSlug(slug));
    }

    let dir = app
        .path()
        .app_data_dir()
        .map_err(AppError::AppDataDir)?
        .join("posts");
    let local_path = dir.join(format!("{slug}.md"));

    // 1. Local cache hit — unless a refresh has marked it out of date. That
    //    mark means the cloud's copy moved on while this file did not, so
    //    serving it would hand back an older version of a post the app already
    //    describes as current. See `post_body_stale`.
    //
    //    Except where this machine has unpushed edits: then the cached body is
    //    the author's own newer text, and going to R2 for the published version
    //    would put an older copy over it. Derived rather than cleared on every
    //    write, so no failed bookkeeping can turn the mark into a way of losing
    //    a draft.
    //
    //    The answer and the baseline for step 2 are taken together under the
    //    lock. Apart, a save landing between them leaves `before` already
    //    describing that save's own draft — and the check after the download,
    //    finding the stamp exactly as it left it, would conclude nothing had
    //    happened and write over the very text it was meant to protect.
    let before = {
        let _guard = lock_body_commits().await;
        let stale = !has_local_edits(conn.inner(), &slug).await?
            && db::body_is_stale(conn.inner(), &slug).await?;
        if !stale {
            if let Ok(content) = tokio::fs::read_to_string(&local_path).await {
                return Ok(content);
            }
        }
        body_stamp(&local_path).await
    };

    // 2. Not cached locally — download from R2.
    //
    // Without credentials there is nowhere to read it from, and that is not the
    // same fact as "this post has no body". Reporting it as an empty document,
    // which this used to do, is a lie the rest of the app then acts on: the
    // editor shows an empty post, a save writes that emptiness into the local
    // cache, later reads prefer the cache, and publishing puts it over the
    // Markdown still sitting in R2. Saying "I cannot tell you" costs one error
    // message and keeps the post intact.
    let (client, config) = cf().map_err(|_| AppError::BodyUnavailable(slug.clone()))?;
    let key = media_keys::body_key(&slug);
    let downloaded = cloudflare::download_from_r2(&client, &config, &key).await?;

    // Whoever wrote this body while the download was in flight wins, and their
    // text is the answer.
    //
    // Two questions, because either alone leaves a gap. The sync row catches a
    // save that finished cleanly; the file's own stamp catches one whose
    // bookkeeping did not — a fingerprint or a mark-clearing write that failed
    // leaves text on disk newer than everything describing it, and only the file
    // still says so.
    //
    // Asked for an empty result as much as a fetched one. R2 having no object is
    // not evidence that this machine has none: a body created while the request
    // was out would be answered with an empty string, the editor would take that
    // as the post's loaded contents, and the next save would write nothing over
    // a draft that was there all along.
    let _guard = lock_body_commits().await;
    if has_local_edits(conn.inner(), &slug).await? || body_stamp(&local_path).await != before {
        return Ok(match tokio::fs::read_to_string(&local_path).await {
            Ok(current) => current,
            // Unreadable after all that: hand back what the cloud gave rather
            // than nothing, and leave the file alone regardless.
            Err(_) => downloaded.unwrap_or_default(),
        });
    }

    match downloaded {
        Some(content) => {
            // Cache locally for next time (best effort). This *is* the cloud's
            // current copy, so whatever was stale about the old one is settled.
            //
            // Staged like every other replacement, so a reader mid-swap sees one
            // whole body or the other rather than a partial file.
            let _ = tokio::fs::create_dir_all(&dir).await;
            if let Ok(staged) = StagedBody::write(&dir, &content).await {
                if staged.commit(&local_path).await.is_ok() {
                    let _ = db::body_stale_clear(conn.inner(), &slug).await;
                }
            }
            Ok(content)
        }
        None => Ok(String::new()),
    }
}

/// Save a post from the editor: persist its metadata + Markdown locally, and —
/// when `published` — upload the body to R2 and upsert the metadata to D1.
///
/// A new post (`id` is `None`) is created and its generated row is returned; an
/// existing post is updated in place, preserving its slug, created date, excerpt
/// and series membership. `tags` is a comma-separated string. A publish that
/// can't reach the cloud leaves the post saved locally and staged `sync_failed`,
/// returning an error.
///
/// `published` asks for this save to *publish*; it is not a statement about
/// whether the post is live. A live post saved with `published: false` stays
/// live — readers keep the version already on the blog — and its local edits are
/// recorded as unpublished. Taking a post off the blog is `unpublish_post`.
#[tauri::command]
pub async fn save_post(
    app: tauri::AppHandle,
    conn: State<'_, DatabaseConnection>,
    id: Option<i32>,
    title: String,
    tags: String,
    body: String,
    published: bool,
) -> AppResult<PostModel> {
    let origin = if published { post_revision::PUBLISH } else { post_revision::SAVE };
    save(app, conn.inner(), id, title, tags, body, published, origin).await
}

/// Persist the editor's in-progress work to this machine, and to nowhere else.
///
/// The same local half of [`save_post`], with two deliberate differences.
///
/// It **cannot publish**: there is no flag to pass, so a background timer can
/// never upload a half-written paragraph to the blog, and the editor's promise
/// that autosave is local is a property of the command surface rather than of
/// the caller remembering to send `false`.
///
/// And it is recorded in the history as an autosave, which is what lets those
/// snapshots coalesce — see [`crate::revisions::AUTOSAVE_COALESCE_SECS`]. A
/// flush every couple of seconds recorded as an ordinary save would push the
/// version somebody actually wants out of a fifty-row history within the hour.
#[tauri::command]
pub async fn autosave_post(
    app: tauri::AppHandle,
    conn: State<'_, DatabaseConnection>,
    id: Option<i32>,
    title: String,
    tags: String,
    body: String,
) -> AppResult<PostModel> {
    save(app, conn.inner(), id, title, tags, body, false, post_revision::AUTOSAVE).await
}

/// The save itself, shared by the editor's Save/Publish buttons and its
/// autosave. `origin` is the history entry this save's snapshot is filed under.
#[allow(clippy::too_many_arguments)]
async fn save(
    app: tauri::AppHandle,
    conn: &DatabaseConnection,
    id: Option<i32>,
    title: String,
    tags: String,
    body: String,
    published: bool,
    origin: &'static str,
) -> AppResult<PostModel> {
    let now = now_ts();

    // Start from the existing row (preserving slug/created_at/series/excerpt) or
    // build a fresh one for a new post. The untouched row is kept: it is what
    // `restore_metadata` writes back if the body cannot be moved into place.
    let previous = match id {
        Some(id) => Some(PreviousState::read(conn, id).await?),
        None => None,
    };

    // An editor left open on a post that has since been thrown away must not
    // write into the copy being kept for recovery — nor, on Publish, put a
    // deleted post on the blog.
    if let Some(existing) = previous.as_ref() {
        refuse_if_trashed(conn, &existing.post).await?;
    }

    let mut model = match previous.clone() {
        Some(existing) => existing.post,
        None => {
            let slug = slugify(&title);
            let slug = if slug.is_empty() { format!("post-{now}") } else { slug };
            PostModel {
                id: 0,
                slug,
                title: String::new(),
                excerpt: None,
                tags: None,
                published: false,
                published_at: None,
                series_id: None,
                series_order: None,
                created_at: now,
                updated_at: now,
            }
        }
    };

    // Apply the editor's fields.
    model.title = title;
    model.tags = Some(tags_to_json(&tags));
    // Saving locally never takes a live post off the blog. `published` describes
    // the *cloud's* copy, which a local save does not touch — the old version
    // goes on being served either way — so clearing the flag here would report a
    // post as unpublished while readers were still reading it, and would take
    // the editor's Save Draft button, whose job is "keep this for later", and
    // quietly make it an unpublish button.
    //
    // That is also what makes the difference this state model exists for
    // reachable from the editor at all: a live post saved locally stays live and
    // becomes `modified`, exactly as it does when an MCP client edits it.
    // Unpublishing is `unpublish_post`, deliberately and on its own.
    model.published = published || model.published;
    model.published_at = if model.published { model.published_at.or(Some(now)) } else { None };
    model.updated_at = now;

    // Whether the post is live once saved — which is not the same question as
    // whether *this* save publishes it.
    let live = model.published;

    let dir = posts_dir(&app).await?;

    // 1. Stage the body. Nothing else has changed yet, so a disk that cannot
    //    take the write leaves the post exactly as it was.
    let staged = StagedBody::write(&dir, &body).await?;

    // From here to the end of step 4 this is the only writer of this body. A
    // read that found the cached copy stale is at this moment holding an older
    // version fetched from R2 and looking for somewhere to put it; without the
    // lock it can land between the metadata below and the rename that belongs
    // with it, and the post ends up with this save's database row and the
    // cloud's text. See `lock_body_commits`.
    //
    // Taken after the staging write above, which touches nothing anybody reads.
    let body_guard = lock_body_commits().await;

    // 2. Commit the metadata, with the staging row when the stage is already
    //    known. A post that stays live keeps the stage it has: saving edits
    //    locally does not demote it to a draft, and its publish stage moves only
    //    when a push succeeds or fails.
    let saved = match commit_metadata(conn, model, id.is_some(), (!live).then_some(now))
        .await
    {
        Ok(saved) => saved,
        Err(e) => {
            staged.discard().await;
            return Err(e);
        }
    };

    // 3. Keep what is about to be replaced, for a post that has a previous
    //    version to lose. Deliberately here and not earlier: the body on disk is
    //    still the old one until the rename below, and a save that failed at
    //    step 1 or 2 changed nothing and so has nothing to snapshot.
    //
    //    Best effort by design — see `revisions::snapshot_or_log`.
    if let Some(before) = previous.as_ref() {
        revisions::snapshot_or_log(&app, conn, &before.post, origin).await;
    }

    // The cached body is about to become this machine's own writing, so whatever
    // a refresh said about it being behind the cloud stops being true — see
    // `post_body_stale`.
    //
    // Cleared *before* the rename, and allowed to fail the save. Ordered the
    // other way, a database that goes down between the rename and here leaves
    // new text on disk with the mark still standing and the fingerprint below
    // never written: a later read then finds a post that reads as clean and
    // stale, and fetches the older published copy over the draft. Nothing later
    // can notice, because the stamp a read takes describes the draft it is about
    // to destroy.
    //
    // This way round, the same outage stops the save before the file moves and
    // the text is still in the editor. The cost is the opposite failure — a
    // cleared mark with the rename then failing, which serves a stale body until
    // the next write — and that one loses nothing.
    let was_stale = db::body_is_stale(conn, &saved.slug).await?;
    db::body_stale_clear(conn, &saved.slug).await?;

    // 4. Swap the new body in. Only a rename is left, so the window in which the
    //    database and the file disagree is as small as it can be — and if even
    //    that fails, the metadata goes back rather than outliving its body.
    if let Err(e) = staged.commit(&dir.join(format!("{}.md", saved.slug))).await {
        restore_metadata(conn, previous, &saved).await;
        // The body it described never arrived, so the cloud's copy is the newer
        // one again — but only for a post that was already behind it. Setting the
        // mark on one that was not would send its next read to R2 for a body it
        // already has. Best effort: the rename failure is what is worth
        // reporting, and a mark that does not come back costs a stale read
        // rather than any text.
        if was_stale {
            if let Err(mark) = db::body_stale_set(conn, &saved.slug, now).await {
                log::warn!("Could not restore the staleness mark for `{}`: {mark}", saved.slug);
            }
        }
        return Err(e);
    }

    // 5. Fingerprint what is now on this machine, so the difference between
    //    "published" and "published, and then edited" is recorded rather than
    //    inferred.
    let hash = sync_state::content_hash(&saved, &body);

    // Recorded before the lock goes, and for a publish as well as a draft.
    //
    // This row is what a waiting read consults to decide whether the body on
    // disk may be replaced, so the moment between renaming the file and saying
    // it is this machine's own is a moment when the post reads as untouched
    // while holding text nobody else has. A read that was already queued on the
    // lock would take that answer and rename its older R2 copy straight over
    // this save. The mark cleared above does not cover it: the reader made its
    // staleness decision before the download and only re-asks about local edits.
    //
    // For a publish it is momentarily true in its own right — the body is on
    // disk and not yet in R2 — and step 6 corrects it to `synced` once the push
    // lands. A push that never lands leaves it accurate.
    db::sync_set_local(conn, saved.id, hash.clone()).await?;

    // The file, the row and the fingerprint now agree. Released before the
    // upload below, which is slow and has no business holding every other
    // post's saves up.
    drop(body_guard);

    // 6. Draft → local only. Publish → push the body to R2 and metadata to D1.
    if !published {
        return Ok(saved);
    }

    let synced = push_to_cloud(&app, conn, &saved, &body).await;

    let stage = if synced.is_ok() { post_stage::PUBLISHED } else { post_stage::SYNC_FAILED };
    db::stage_set(
        conn,
        post_stage::Model { post_id: saved.id, stage: stage.to_string(), staged_at: now },
    )
    .await?;

    // A push that landed means the cloud now holds exactly this content. A push
    // that did not means it holds the previous version, and the post has to keep
    // saying so rather than reading as freshly published.
    if synced.is_ok() {
        // `d1_post_upsert` writes this post's own `updated_at` into D1, so that
        // is the cloud's version now — record it as the baseline, or the next
        // refresh reads our own push as somebody else's change.
        db::sync_mark_synced(conn, saved.id, hash, Some(saved.updated_at), now).await?;
    } else {
        // Already true — step 5 recorded it before the lock was released — and
        // restated rather than left implied, so the failure path says what it
        // leaves behind instead of depending on a caller further up.
        db::sync_set_local(conn, saved.id, hash).await?;
    }

    match synced {
        Ok(()) => Ok(saved),
        Err(e) => Err(AppError::PublishSyncFailed(Box::new(e))),
    }
}

/// Put a post's content where readers get it: images and body into R2, metadata
/// into D1.
///
/// Shared by publishing and by scheduling, which need exactly the same upload
/// and differ only in the `published` flag on the row that goes with it. A
/// scheduled post's body is uploaded when it is *scheduled*, not when it goes
/// live — the blog serves published rows only, so a body sitting in R2 under an
/// unpublished post is invisible, and it means the Worker that runs the
/// publication needs nothing but one D1 statement.
///
/// The referenced local images go up first, so the body never lands pointing at
/// an object that is not there yet. Each is rewritten to its public URL, which
/// makes the published Markdown self-contained: the blog renders it as-is, with
/// no rewriting step of its own.
async fn push_to_cloud(
    app: &tauri::AppHandle,
    conn: &DatabaseConnection,
    post: &PostModel,
    body: &str,
) -> AppResult<()> {
    // Checked here, at the last moment before anything reaches the cloud. Both
    // callers arrive after several awaits — a publish has staged a body and
    // taken a snapshot, a schedule has read the Markdown — and a post can be
    // thrown away in that window. Publishing a deleted post is the one thing
    // that cannot be taken back locally.
    refuse_if_trashed(conn, post).await?;

    let assets_dir = app
        .path()
        .app_data_dir()
        .map_err(AppError::AppDataDir)?
        .join("assets");

    let (client, config) = cf()?;

    let public_base = config.r2_public_url.trim_end_matches('/');
    if public_base.is_empty() {
        return Err(AppError::NoPublicUrl);
    }

    let mut published = body.to_string();
    for r in extract_asset_refs(body) {
        let Some((file_name, bytes)) = read_staged_asset(&assets_dir, &r).await else {
            continue;
        };
        let ext = file_name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        let key = media_keys::media_key(&config.media_key_pattern, &post.slug, &bytes, &ext);
        cloudflare::upload_bytes_to_r2(&client, &config, &key, bytes, content_type_for(&ext))
            .await?;
        published = published.replace(&r, &media_keys::public_url(public_base, &key));
    }

    cloudflare::upload_to_r2(&client, &config, &media_keys::body_key(&post.slug), &published)
        .await?;
    // Metadata → D1, with the series reference translated into the cloud's ids —
    // a local `series_id` would file the post under an unrelated remote series.
    let outbound = post_for_cloud(conn, &client, &config, post.clone()).await?;
    cloudflare::d1_post_upsert(&client, &config, outbound).await
}

// ─── Scheduled publishing ─────────────────────────────────────────────────────

/// Ask for a post to go live at a given time.
///
/// The publication itself is carried out by a Cloudflare Worker on a cron
/// trigger, because the acceptance criterion is that it happens whether or not
/// this app is running — and a timer inside a desktop app cannot promise that.
/// See `worker/README.md`.
///
/// What happens here is everything *except* the flip:
///
/// 1. the body and its images go to R2, and the metadata to D1 with
///    `published` still false. The blog serves published rows only, so this is
///    invisible to readers; it also means the Worker needs no R2 access and no
///    knowledge of how a post is assembled.
/// 2. a `post_schedule` row goes to D1, which is what the Worker reads.
/// 3. the same row is mirrored locally, so the app can show the schedule
///    offline.
///
/// Uploading now rather than later is the whole design. A Worker that had to
/// assemble the post at publication time would need the R2 credentials, the
/// image rewriting rules and the local asset cache — none of which exist in
/// Cloudflare — and would fail at the one moment nobody is watching.
#[tauri::command]
pub async fn schedule_post(
    app: tauri::AppHandle,
    conn: State<'_, DatabaseConnection>,
    post_id: i32,
    publish_at: i64,
) -> AppResult<crate::entities::post_schedule::Model> {
    let post = db::get::<PostModel>(conn.inner(), post_id)
        .await?
        .ok_or(AppError::PostNotFound(post_id))?;

    // A live post has nothing to schedule: it is already what a schedule would
    // make it. Publishing an edit to it is the Publish button.
    if post.published {
        return Err(AppError::AlreadyPublished(post.slug));
    }
    // And a post in the trash has been deleted as far as this app is concerned.
    // Scheduling it would put it on the blog from inside the bin — the same
    // mistake `trash_post` refuses from the other direction.
    refuse_if_trashed(conn.inner(), &post).await?;

    let now = now_ts();
    if publish_at <= now {
        return Err(AppError::ScheduleInThePast(publish_at));
    }

    // The guard above reads this machine's mirror, and on this particular field
    // the mirror is the one thing that cannot be trusted: the Worker publishes by
    // writing `published` straight into D1, and `upsert_post_from_remote` skips
    // any post carrying unpushed local edits — so a post the Worker has already
    // put live goes on reading as a draft here for as long as it has edits
    // waiting. Scheduling it again from that stale reading would send
    // `published = false` up in the upsert below (`d1_post_upsert` lists that
    // column), taking a live article off the blog while reporting success.
    //
    // So the cloud is asked. This narrows the window to the round trip rather
    // than closing it — the Worker could still publish between this answer and
    // the upsert — but that is a moment instead of a condition that persists
    // until someone happens to save.
    let (check_client, check_config) = cf()?;
    let remote_published = cloudflare::d1_list::<PostModel>(&check_client, &check_config)
        .await?
        .into_iter()
        .find(|p| p.slug == post.slug)
        .is_some_and(|p| p.published);
    if remote_published {
        return Err(AppError::AlreadyPublished(post.slug));
    }

    let body = read_post_markdown(app.clone(), conn.clone(), post.slug.clone()).await?;
    push_to_cloud(&app, conn.inner(), &post, &body).await?;

    let row = crate::entities::post_schedule::Model {
        slug: post.slug.clone(),
        publish_at,
        state: post_schedule::PENDING.to_string(),
        error: None,
        updated_at: now,
    };
    let (client, config) = cf()?;

    // Local first, then the cloud. `trash_post` reads the local mirror to refuse
    // deleting a post Cloudflare is about to publish, so a schedule that exists
    // in D1 and not here is one the Worker will carry out with nothing standing
    // in the way of the post being thrown away first — and offline, where a
    // refresh cannot correct the mirror, that is not a brief window.
    //
    // This order fails the other way: a local row with no cloud schedule, which
    // refuses a deletion that would in fact have been safe, and disappears at the
    // next refresh. Cleared here as well, so it usually does not outlive the
    // error — best effort, because the row it might leave behind is the harmless
    // one.
    db::schedule_set(conn.inner(), row.clone()).await?;
    if let Err(e) = cloudflare::d1_schedule_upsert(&client, &config, row.clone()).await {
        if let Err(cleanup) = db::schedule_clear(conn.inner(), &post.slug).await {
            log::warn!("Could not undo the local schedule for `{}`: {cleanup}", post.slug);
        }
        return Err(e);
    }

    // The body is in R2 and the metadata is in D1; what is here and what is
    // there agree, and the only thing still to come is the flip.
    //
    // Unless the text moved while it was going up. Everything above this takes a
    // network round trip, and a save landing inside one leaves this machine
    // holding something the cloud has not seen — recording it as synced would
    // hide that, and would mark a draft as safe to replace with the copy that
    // was uploaded instead of it. The schedule still stands; what is scheduled
    // is simply the version that was sent, and the newer text reads as the
    // unpushed edit it is.
    let uploaded = {
        let _guard = lock_body_commits().await;
        if revisions::cached_body(&app, &post.slug).await.as_deref() == Some(body.as_str()) {
            db::sync_mark_synced(
                conn.inner(),
                post.id,
                sync_state::content_hash(&post, &body),
                Some(post.updated_at),
                now,
            )
            .await?;
            true
        } else {
            false
        }
    };
    if !uploaded {
        log::info!(
            "`{}` was edited while it was being scheduled; the version that went up is the one              scheduled, and the newer text stays an unpushed edit",
            post.slug
        );
    }

    Ok(row)
}

/// Call off a pending publication.
///
/// The row is kept and marked `cancelled` rather than deleted, so the app can
/// say what became of a schedule somebody set — and so a Worker mid-run finds a
/// row that says "do not publish this" rather than no row at all, which it
/// could not tell from a schedule that had never existed.
///
/// The cancellation is conditional on the schedule still being pending **in D1**,
/// not on what the local mirror last saw. Those differ exactly when it matters:
/// a Worker run may have claimed the schedule seconds ago, and a cancellation
/// that overwrote that claim would leave a row reading `cancelled` for a post
/// that went live anyway. When that happens the local mirror is brought up to
/// date and the attempt is reported rather than pretended.
#[tauri::command]
pub async fn cancel_schedule(
    conn: State<'_, DatabaseConnection>,
    post_id: i32,
) -> AppResult<crate::entities::post_schedule::Model> {
    let post = db::get::<PostModel>(conn.inner(), post_id)
        .await?
        .ok_or(AppError::PostNotFound(post_id))?;
    let existing = db::schedule_get(conn.inner(), &post.slug)
        .await?
        .ok_or_else(|| AppError::NotScheduled(post.slug.clone()))?;

    let now = now_ts();
    let (client, config) = cf()?;
    if !cloudflare::d1_schedule_cancel(&client, &config, &post.slug, now).await? {
        // Whatever the cloud now says is the truth; take a copy of it so the
        // screen showing "scheduled" stops saying so.
        if let Ok(Some(remote)) = cloudflare::d1_get::<crate::entities::post_schedule::Model>(
            &client,
            &config,
            post.slug.clone(),
        )
        .await
        {
            db::schedule_set(conn.inner(), remote).await?;
        }
        return Err(AppError::ScheduleNotPending(post.slug));
    }

    let row = crate::entities::post_schedule::Model {
        state: post_schedule::CANCELLED.to_string(),
        error: None,
        updated_at: now,
        ..existing
    };
    db::schedule_set(conn.inner(), row.clone()).await?;
    Ok(row)
}

/// One schedule as the desktop reads it: the stored row, plus the state it
/// actually displays as — see [`post_schedule::Model::display_state`].
#[derive(serde::Serialize)]
pub struct ScheduleView {
    pub slug: String,
    pub publish_at: i64,
    /// `scheduled` | `overdue` | `published` | `failed` | `cancelled` |
    /// `unknown`.
    pub state: &'static str,
    pub error: Option<String>,
    pub updated_at: i64,
}

/// Every schedule this machine knows about, soonest first.
///
/// Read from the local mirror, so it works offline; `sync_posts_from_cloud`
/// brings it up to date with what the Worker has actually done.
#[tauri::command]
pub async fn list_schedules(conn: State<'_, DatabaseConnection>) -> AppResult<Vec<ScheduleView>> {
    let now = now_ts();
    Ok(db::schedules_all(conn.inner())
        .await?
        .into_iter()
        .map(|row| ScheduleView {
            state: row.display_state(now),
            slug: row.slug,
            publish_at: row.publish_at,
            error: row.error,
            updated_at: row.updated_at,
        })
        .collect())
}

// ─── Conflict resolution ──────────────────────────────────────────────────────

/// Which copy of a conflicted post to keep.
#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    /// Keep what is on this machine. The cloud's version stamp is accepted as
    /// the new baseline, so the post stops being a conflict and becomes an
    /// ordinary pending edit, ready to publish.
    KeepLocal,
    /// Take the cloud's copy, metadata and body, over the local one.
    KeepRemote,
}

/// Settle a post whose local and cloud copies both changed.
///
/// This is the only place either copy is overwritten. A refresh deliberately
/// refuses to choose — see `db::mirror_posts` — because both answers destroy
/// work and only a person knows which loss is acceptable.
///
/// Keeping **local** writes nothing to the cloud. It records that the cloud's
/// current version has been seen and accounted for, which drops the post from
/// `conflict` to `modified`: the edits are still pending, and publishing them is
/// a separate, deliberate act. Doing it here would push over the remote change
/// the person just chose to discard — a decision they have made, but not one
/// this command was asked to carry out, and for MCP-originated posts it would
/// walk straight through the approval gate.
///
/// Keeping **remote** replaces the local metadata and body with the cloud's, and
/// the two agree from there.
#[tauri::command]
pub async fn resolve_conflict(
    app: tauri::AppHandle,
    conn: State<'_, DatabaseConnection>,
    post_id: i32,
    keep: Resolution,
) -> AppResult<PostModel> {
    let now = now_ts();
    let post = db::get::<PostModel>(conn.inner(), post_id)
        .await?
        .ok_or(AppError::PostNotFound(post_id))?;
    // A post can be trashed while carrying an unsettled conflict. Settling it
    // then writes to a deleted post, and "keep cloud" would download over the
    // very copy the trash is holding on to.
    refuse_if_trashed(conn.inner(), &post).await?;

    // Refuse anything that is not actually a conflict. Resolving a post that is
    // merely modified would silently discard the pending edit under a button
    // labelled for a situation it is not in.
    let sync = db::sync_get(conn.inner(), post_id).await?;
    let stage = db::stage_get(conn.inner(), post_id).await?;
    if sync_state::derive(stage.as_ref(), sync.as_ref()) != sync_state::SyncState::Conflict {
        return Err(AppError::NotConflicted(post_id));
    }
    // Guaranteed by the state above; a conflict cannot be derived without it.
    let observed = sync.and_then(|s| s.remote_seen_at);

    match keep {
        Resolution::KeepLocal => {
            // The cloud's version is now accounted for. Nothing local moves, so
            // the fingerprint still describes exactly what is on disk.
            db::sync_accept_remote_baseline(conn.inner(), post_id, observed).await?;
            Ok(post)
        }
        Resolution::KeepRemote => {
            let (client, config) = cf()?;

            let remote = cloudflare::d1_list::<PostModel>(&client, &config)
                .await?
                .into_iter()
                .find(|p| p.slug == post.slug)
                .ok_or_else(|| AppError::RemotePostGone(post.slug.clone()))?;

            let body = cloudflare::download_from_r2(
                &client,
                &config,
                &media_keys::body_key(&post.slug),
            )
            .await?
            .unwrap_or_default();

            // Body first, by the same reasoning as `save_post`: the write is
            // what fails, and until the rename nothing has been replaced.
            let dir = posts_dir(&app).await?;
            let staged = StagedBody::write(&dir, &body).await?;

            // The remote's series reference is a remote primary key; translate
            // it rather than storing a number that means something else here.
            let remote_series = cloudflare::d1_list::<SeriesModel>(&client, &config).await?;
            let series = db::SeriesMap::build(conn.inner(), &remote_series).await?;

            let mut model = remote;
            model.id = post.id;
            // The same rule a refresh uses, and for the same reason: the cloud
            // cannot distinguish "not in a series" from "in a series that only
            // exists here", so taking its answer literally would drop the
            // grouping of every post filed under a local-only series. Settling a
            // conflict about *content* has no business deleting that.
            let (series_id, series_order) = db::resolve_series(&model, Some(&post), &series);
            model.series_id = series_id;
            model.series_order = series_order;
            let remote_updated_at = model.updated_at;

            // "Keep cloud" is the one place the app throws away local work on
            // purpose, so it is also the place a snapshot matters most: taken
            // while the local body is still on disk, it turns an irreversible
            // choice into one the history can walk back.
            revisions::snapshot_or_log(
                &app,
                conn.inner(),
                &post,
                post_revision::CONFLICT_KEEP_REMOTE,
            )
            .await;

            // What this arm is about to overwrite, kept so a failed rename can
            // be undone below.
            let previous = PreviousState::read(conn.inner(), post.id).await?;

            // Ordered lock-then-database like every other body writer, and
            // held through the rename that matches the metadata.
            //
            // Without it an editor autosave can land its row, body and
            // fingerprint between the commit and the rename below — and the
            // `sync_agree` ending this arm would then record the draft it
            // overwrote as agreeing with the cloud, so the loss reads as clean.
            let body_guard = lock_body_commits().await;

            // The trash check that counts: inside the transaction that writes.
            // The one at the top of this command ran before a D1 listing and an
            // R2 download, which is plenty of time for another window to throw
            // the post away — and taking the cloud's copy over it would replace
            // the very version being kept for recovery.
            let txn = conn.inner().begin().await?;
            if db::trash_get(&txn, post.id).await?.is_some() {
                staged.discard().await;
                return Err(AppError::PostInTrash(post.slug));
            }
            let saved = db::update::<PostModel>(&txn, model).await?;
            txn.commit().await?;

            // Committed metadata, body not yet in place — the window `save`
            // spends `PreviousState` on. Failing without undoing it leaves the
            // cloud's title and flags describing the local body, with the stale
            // mark, stage and fingerprint below all skipped, so nothing records
            // that the halves came from different versions.
            if let Err(e) = staged.commit(&dir.join(format!("{}.md", saved.slug))).await {
                restore_metadata(conn.inner(), Some(previous), &saved).await;
                return Err(e);
            }

            // This body came from R2 a moment ago, so it is the cloud's current
            // copy by construction and any staleness is settled.
            let _ = db::body_stale_clear(conn.inner(), &saved.slug).await;

            db::stage_set(
                conn.inner(),
                post_stage::Model {
                    post_id: saved.id,
                    stage: if saved.published {
                        post_stage::PUBLISHED.to_string()
                    } else {
                        post_stage::DRAFT.to_string()
                    },
                    staged_at: now,
                },
            )
            .await?;
            db::sync_agree(
                conn.inner(),
                saved.id,
                sync_state::content_hash(&saved, &body),
                Some(remote_updated_at),
                now,
            )
            .await?;

            // The row, the file and the fingerprint all describe the cloud's
            // version from here on, so the next writer may have the lock.
            drop(body_guard);

            Ok(saved)
        }
    }
}

// ─── Media library (R2 + local cache) ─────────────────────────────────────────

/// A media object stored in R2, cached locally for display.
#[derive(Serialize)]
pub struct MediaItem {
    /// R2 key, also the local-relative cache path, e.g. `"media/<uuid>.png"`.
    pub key: String,
    /// The key's last segment.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
}

/// Media extensions accepted by the uploader.
const MEDIA_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "avif", "svg", "bmp", "ico", "mp4", "webm", "mov",
];

/// Pick a media file, upload it to R2 under `media/<uuid>.<ext>`, and cache it
/// locally. Returns the new item, or `Err("cancelled")` when the dialog closes.
#[tauri::command]
pub async fn upload_media(app: tauri::AppHandle) -> AppResult<MediaItem> {
    let app_clone = app.clone();
    let picked = tokio::task::spawn_blocking(move || {
        app_clone
            .dialog()
            .file()
            .add_filter("Media", MEDIA_EXTS)
            .blocking_pick_file()
    })
    .await
    .map_err(|e| AppError::join("Dialog thread panicked", e))?;

    let src = match picked {
        None => return Err(AppError::Cancelled),
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        Some(tauri_plugin_dialog::FilePath::Path(p)) => p,
        #[allow(unreachable_patterns)]
        Some(_) => return Err(AppError::UnsupportedPathFormat),
    };

    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|e| MEDIA_EXTS.contains(&e.as_str()))
        .ok_or(AppError::UnsupportedMedia)?;

    let bytes = tokio::fs::read(&src)
        .await
        .map_err(|e| AppError::io("Failed to read file", e))?;

    // JPG/PNG become AVIF; everything else is uploaded as picked. `size` is
    // measured after conversion so the library reports what R2 actually holds.
    let (ext, bytes) = if imaging::is_convertible(&ext) {
        ("avif".to_string(), imaging::convert_to_avif(bytes).await?)
    } else {
        (ext, bytes)
    };
    let size = bytes.len() as u64;

    let file_name = format!("{}.{ext}", uuid::Uuid::new_v4());
    let key = format!("media/{file_name}");

    // Upload to R2.
    let (client, config) = cf()?;
    cloudflare::upload_bytes_to_r2(&client, &config, &key, bytes.clone(), content_type_for(&ext))
        .await?;

    // Cache locally (best effort).
    let dir = app
        .path()
        .app_data_dir()
        .map_err(AppError::AppDataDir)?
        .join("media");
    let _ = tokio::fs::create_dir_all(&dir).await;
    let _ = tokio::fs::write(dir.join(&file_name), &bytes).await;

    Ok(MediaItem { key, name: file_name, size })
}

/// Copy a media-library object into the post's local assets directory so the
/// editor can insert it, backing the "Insert media → Select from Media library"
/// flow.
///
/// The library stays a reusable pool under `media/`; nothing there is read by
/// the blog. Staging locally means the object then travels the same publish
/// path as a dropped image — hashed and uploaded to `posts/<slug>/<sha256>.ext`
/// — so one image reused across posts lands under each post's own prefix and
/// the reader never has to know the library exists.
#[tauri::command]
pub async fn stage_media_from_library(
    app: tauri::AppHandle,
    key: String,
) -> AppResult<StagedImage> {
    let file_name = key
        .strip_prefix("media/")
        .filter(|n| is_safe_file_name(n))
        .ok_or_else(|| AppError::NotAMediaKey(key.clone()))?
        .to_string();

    let ext = file_name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if ext.is_empty() {
        return Err(AppError::MediaKeyHasNoExtension(key));
    }

    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(AppError::AppDataDir)?;

    // Prefer the local cache the media library already maintains; fall back to
    // R2 for an object listed but not yet cached.
    let cached = data_dir.join("media").join(&file_name);
    let bytes = match tokio::fs::read(&cached).await {
        Ok(bytes) => bytes,
        Err(_) => {
            let (client, config) = cf()?;
            cloudflare::download_bytes_from_r2(&client, &config, &key)
                .await?
                .ok_or_else(|| AppError::MediaNotFound(key.clone()))?
        }
    };

    let assets_dir = data_dir.join("assets");
    tokio::fs::create_dir_all(&assets_dir)
        .await
        .map_err(|e| AppError::io("Failed to create assets dir", e))?;

    let staged_name = format!("{}.{ext}", uuid::Uuid::new_v4());
    tokio::fs::write(assets_dir.join(&staged_name), &bytes)
        .await
        .map_err(|e| AppError::io("Failed to stage media", e))?;

    Ok(StagedImage { rel: format!("assets/{staged_name}"), name: file_name })
}

/// List media objects in R2 (prefix `media/`), caching any not already local.
#[tauri::command]
pub async fn list_media(app: tauri::AppHandle) -> AppResult<Vec<MediaItem>> {
    let (client, config) = cf()?;
    let objects = cloudflare::list_r2(&client, &config, "media/").await?;

    let dir = app
        .path()
        .app_data_dir()
        .map_err(AppError::AppDataDir)?
        .join("media");
    let _ = tokio::fs::create_dir_all(&dir).await;

    let mut items = Vec::new();
    for obj in objects {
        // The same guard `delete_media` and `stage_media_from_library` put on
        // this join, which this listing was the one place to skip. The name
        // comes from an R2 key, and R2 keys may contain `..` and `/` — the
        // bucket is written by the blog and by anything else holding the token,
        // so a key is not something this app decided the shape of. Joined
        // unchecked, `media/../../../evil.js` writes outside the cache
        // directory.
        //
        // A nested key like `media/2026/pic.png` is refused by the same test.
        // That one is harmless but not cacheable, and it used to fail silently
        // inside `let _`, so every listing downloaded it again to no effect.
        let file_name = match obj.key.strip_prefix("media/") {
            Some(name) if is_safe_file_name(name) => name.to_string(),
            // Also the folder marker (`media/` itself), which is empty and so
            // not a plain file name either.
            _ => {
                log::warn!("Skipping media object with an unusable key: {}", obj.key);
                continue;
            }
        };
        // Ensure a local cached copy exists for display.
        let local = dir.join(&file_name);
        if tokio::fs::metadata(&local).await.is_err() {
            if let Ok(Some(bytes)) =
                cloudflare::download_bytes_from_r2(&client, &config, &obj.key).await
            {
                let _ = tokio::fs::write(&local, &bytes).await;
            }
        }
        items.push(MediaItem { key: obj.key, name: file_name, size: obj.size });
    }
    Ok(items)
}

/// Which posts still reference each media object — see [`crate::media_usage`]
/// for why the answer is derived from the posts rather than kept in a table.
#[tauri::command]
pub async fn media_usage(
    app: tauri::AppHandle,
    conn: State<'_, DatabaseConnection>,
) -> AppResult<media_usage::UsageReport> {
    media_usage::survey(&app, conn.inner()).await
}

/// Delete a media object from R2 and its local cache.
///
/// Refused while a post still references it, unless `force` says the warning has
/// been read and answered. The check is here rather than only in the UI because
/// this is the point of no return: the object is gone from R2 straight away, and
/// every post pointing at it is left serving a hole to readers.
#[tauri::command]
pub async fn delete_media(
    app: tauri::AppHandle,
    conn: State<'_, DatabaseConnection>,
    key: String,
    force: bool,
) -> AppResult<()> {
    if !force {
        let check = media_usage::users_of(&app, conn.inner(), &key).await?;
        if !check.is_safe() {
            return Err(AppError::MediaInUse {
                key,
                posts: check.users.len(),
                unchecked_posts: check.unchecked_posts.len(),
            });
        }
    }

    let (client, config) = cf()?;
    cloudflare::delete_from_r2(&client, &config, &key).await?;

    if let Some(file_name) = key.strip_prefix("media/") {
        // Only touch the local cache for a safe single filename (no traversal).
        if is_safe_file_name(file_name) {
            let local = app
                .path()
                .app_data_dir()
                .map_err(AppError::AppDataDir)?
                .join("media")
                .join(file_name);
            let _ = tokio::fs::remove_file(local).await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The question every replacement of a cached body turns on: may the
    /// cloud's copy be written over what is on disk?
    ///
    /// A post nobody has touched says yes by saying there are no local edits; a
    /// post with unpushed text says no. The published state is irrelevant to it
    /// — what matters is whether this machine holds something the cloud has not
    /// seen.
    #[tokio::test]
    async fn unpushed_text_is_reported_before_a_body_is_replaced() {
        let db = crate::db::connect_in_memory().await.unwrap();
        let post = crate::db::create::<PostModel>(
            &db,
            PostModel {
                id: 0,
                slug: "a-post".into(),
                title: "A post".into(),
                excerpt: None,
                tags: None,
                published: true,
                published_at: None,
                series_id: None,
                series_order: None,
                created_at: 0,
                updated_at: 0,
            },
        )
        .await
        .unwrap();

        // Nothing recorded yet, so there is no draft to protect.
        assert!(!has_local_edits(&db, "a-post").await.unwrap());

        crate::db::sync_set_local(&db, post.id, "local".into()).await.unwrap();
        assert!(has_local_edits(&db, "a-post").await.unwrap());

        // Pushed: the cloud has this text now, and its copy is no longer older.
        crate::db::sync_mark_synced(&db, post.id, "local".into(), Some(0), 0).await.unwrap();
        assert!(!has_local_edits(&db, "a-post").await.unwrap());

        // A slug the local database has never heard of is a genuine no rather
        // than an error: there is nothing here that could be lost.
        assert!(!has_local_edits(&db, "never-existed").await.unwrap());
    }

    /// The check that does not depend on any bookkeeping having succeeded: a
    /// body that was written while a download was in flight has to be
    /// recognisable from the file alone.
    ///
    /// Both transitions count as a write. One appearing where there was nothing
    /// is a post that gained a body during the round trip, which is every bit as
    /// much somebody else's work as one that was replaced.
    #[tokio::test]
    async fn a_body_written_during_a_download_is_visible_in_its_stamp() {
        let dir = std::env::temp_dir()
            .join(format!("blog-cms-stamp-{}", uuid::Uuid::new_v4().simple()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("a-post.md");

        // Nothing there yet.
        let absent = body_stamp(&path).await;
        assert!(absent.is_none());

        tokio::fs::write(&path, "the draft").await.unwrap();
        let written = body_stamp(&path).await;
        assert!(written.is_some());
        assert_ne!(written, absent, "a body appearing is a write");

        // Replaced with text of a different length.
        tokio::fs::write(&path, "the draft, edited").await.unwrap();
        assert_ne!(body_stamp(&path).await, written, "a body replaced is a write");

        // Untouched between two looks.
        let settled = body_stamp(&path).await;
        assert_eq!(body_stamp(&path).await, settled);

        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }

    #[test]
    fn only_plain_file_names_are_safe() {
        assert!(is_safe_file_name("3f2b8c.avif"));
        assert!(is_safe_file_name("a picture.png"));
        assert!(is_safe_file_name(".hidden.png"));

        assert!(!is_safe_file_name(""));
        assert!(!is_safe_file_name("."));
        assert!(!is_safe_file_name(".."));
        assert!(!is_safe_file_name("../secret.env"));
        assert!(!is_safe_file_name("nested/name.png"));
        assert!(!is_safe_file_name("nested\\name.png"));
        assert!(!is_safe_file_name("/etc/passwd"));
    }

    /// A drive-relative name carries no separator at all, and yet joining it
    /// onto a directory discards that directory outright — which is why this
    /// check asks the path parser rather than scanning for `/`, `\` and `..`.
    #[cfg(windows)]
    #[test]
    fn drive_relative_names_are_rejected() {
        assert!(!is_safe_file_name("C:secret.env"));
        assert_eq!(
            Path::new(r"D:\data\assets").join("C:secret.env"),
            Path::new("C:secret.env")
        );
    }

    /// Extraction stays deliberately loose — it is the *read* that is guarded,
    /// not the scan — so a traversal attempt really does reach
    /// [`read_staged_asset`] and has to be stopped there.
    #[test]
    fn traversal_attempts_survive_extraction() {
        let body = "![ok](assets/ok.png) ![bad](assets/../../secret.env)";
        assert_eq!(
            extract_asset_refs(body),
            vec!["assets/ok.png", "assets/../../secret.env"]
        );
    }

    fn post(slug: &str, title: &str) -> PostModel {
        PostModel {
            id: 0,
            slug: slug.to_string(),
            title: title.to_string(),
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

    /// An existing post is put back the way it was read, so the editor is never
    /// left showing a saved title over the body it failed to replace.
    #[tokio::test]
    async fn rolling_back_an_existing_post_restores_the_previous_row() {
        let db = db::connect_in_memory().await.unwrap();
        let mut original = post("a-post", "Original");
        original.published = true;
        let before = db::create::<PostModel>(&db, original).await.unwrap();
        db::stage_set(
            &db,
            post_stage::Model {
                post_id: before.id,
                stage: post_stage::PUBLISHED.to_string(),
                staged_at: 0,
            },
        )
        .await
        .unwrap();
        let previous = PreviousState::read(&db, before.id).await.unwrap();

        // Save it as a retitled draft — which rewrites the row *and* the stage.
        let mut edited = before.clone();
        edited.title = "Edited".into();
        edited.published = false;
        let saved = commit_metadata(&db, edited, true, Some(99)).await.unwrap();

        restore_metadata(&db, Some(previous), &saved).await;

        let now = db::get::<PostModel>(&db, before.id).await.unwrap().unwrap();
        assert_eq!(now.title, "Original");
        assert!(now.published);
        // Restoring only the row would leave a published post staged `draft`.
        let stage = db::stage_get(&db, before.id).await.unwrap().unwrap();
        assert_eq!(stage.stage, post_stage::PUBLISHED);
    }

    /// A new post has no earlier version to fall back to, so the row and its
    /// stage go — otherwise the list gains a post whose body was never written.
    #[tokio::test]
    async fn rolling_back_a_new_post_removes_the_row_and_its_stage() {
        let db = db::connect_in_memory().await.unwrap();
        let created = db::create::<PostModel>(&db, post("new-post", "New")).await.unwrap();
        db::stage_set(
            &db,
            post_stage::Model {
                post_id: created.id,
                stage: post_stage::DRAFT.to_string(),
                staged_at: 0,
            },
        )
        .await
        .unwrap();

        restore_metadata(&db, None, &created).await;

        assert!(db::get::<PostModel>(&db, created.id).await.unwrap().is_none());
        assert!(db::stage_get(&db, created.id).await.unwrap().is_none());
    }

    /// A save that failed must not undo a *different* save that succeeded.
    ///
    /// The editor and an MCP client can both be saving one post, so another
    /// save can commit its metadata and land its body in the window between
    /// this one's commit and its rollback. Rolling back regardless would revert
    /// their metadata and leave it describing their body — the exact mismatch
    /// this whole change exists to prevent.
    #[tokio::test]
    async fn a_failed_save_leaves_a_newer_save_alone() {
        let db = db::connect_in_memory().await.unwrap();
        let before = db::create::<PostModel>(&db, post("shared", "Original")).await.unwrap();
        let previous = PreviousState::read(&db, before.id).await.unwrap();

        // Our save commits its metadata…
        let mut ours = before.clone();
        ours.title = "Ours".into();
        let ours = commit_metadata(&db, ours, true, Some(1)).await.unwrap();

        // …then another save commits and completes before our rename fails.
        let mut theirs = ours.clone();
        theirs.title = "Theirs".into();
        commit_metadata(&db, theirs, true, Some(2)).await.unwrap();

        restore_metadata(&db, Some(previous), &ours).await;

        let now = db::get::<PostModel>(&db, before.id).await.unwrap().unwrap();
        assert_eq!(now.title, "Theirs", "a failed save reverted a newer one");
    }

    /// The row and the stage land together, so a saved draft is never a post
    /// with no stage at all.
    #[tokio::test]
    async fn a_draft_commits_its_stage_with_its_row() {
        let db = db::connect_in_memory().await.unwrap();

        let saved = commit_metadata(&db, post("drafted", "Drafted"), false, Some(1_700_000_000))
            .await
            .unwrap();

        let stage = db::stage_get(&db, saved.id).await.unwrap().unwrap();
        assert_eq!(stage.stage, post_stage::DRAFT);
        assert_eq!(stage.staged_at, 1_700_000_000);
    }

    /// A live post saved locally keeps the stage it has. Demoting it to `draft`
    /// would say the post is not on the blog, when the previous version still
    /// is — and it is that pairing, live plus unpublished edits, that the whole
    /// state model exists to express.
    #[tokio::test]
    async fn saving_a_live_post_locally_does_not_demote_its_stage() {
        let db = db::connect_in_memory().await.unwrap();
        let mut live = post("live-post", "Live");
        live.published = true;
        let saved = db::create::<PostModel>(&db, live).await.unwrap();
        db::stage_set(
            &db,
            post_stage::Model {
                post_id: saved.id,
                stage: post_stage::PUBLISHED.to_string(),
                staged_at: 0,
            },
        )
        .await
        .unwrap();

        // The shape `save_post` uses for a local save of a post that stays live:
        // no draft stage is written, so the published one survives.
        commit_metadata(&db, saved.clone(), true, None).await.unwrap();

        let stage = db::stage_get(&db, saved.id).await.unwrap().unwrap();
        assert_eq!(stage.stage, post_stage::PUBLISHED);
    }

    /// A publish's stage is not known until the upload has been tried, so the
    /// transaction must not invent one.
    #[tokio::test]
    async fn a_publish_leaves_its_stage_for_the_upload_to_decide() {
        let db = db::connect_in_memory().await.unwrap();

        let saved = commit_metadata(&db, post("publishing", "Publishing"), false, None)
            .await
            .unwrap();

        assert!(db::stage_get(&db, saved.id).await.unwrap().is_none());
    }

    /// Publishing uploads whatever these references resolve to, so a body that
    /// points outside the assets directory must read nothing at all — however
    /// it spells the escape.
    #[tokio::test]
    async fn references_that_escape_the_assets_dir_read_nothing() {
        let root = std::env::temp_dir()
            .join(format!("blog-cms-assets-{}", uuid::Uuid::new_v4().simple()));
        let assets = root.join("assets");
        tokio::fs::create_dir_all(&assets).await.unwrap();
        tokio::fs::write(root.join("secret.env"), b"CF_API_TOKEN=hunter2")
            .await
            .unwrap();
        tokio::fs::write(assets.join("staged.avif"), b"image bytes")
            .await
            .unwrap();

        // A genuinely staged image still publishes.
        assert_eq!(
            read_staged_asset(&assets, "assets/staged.avif").await,
            Some(("staged.avif", b"image bytes".to_vec()))
        );
        // A reference to nothing is skipped, as it always was.
        assert_eq!(read_staged_asset(&assets, "assets/gone.avif").await, None);

        for escape in [
            "assets/../secret.env",
            "assets/../../secret.env",
            "assets/..\\secret.env",
            "assets/./../secret.env",
            "assets//../secret.env",
        ] {
            assert_eq!(
                read_staged_asset(&assets, escape).await,
                None,
                "`{escape}` read a file outside the assets dir"
            );
        }

        let _ = tokio::fs::remove_dir_all(&root).await;
    }
}
