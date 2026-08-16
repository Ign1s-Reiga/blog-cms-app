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
use crate::entities::{post_revision, post_stage};
use crate::error::{AppError, AppResult};
use crate::imaging::{self, StagedImage};
use crate::media_keys;
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
#[tauri::command]
pub async fn read_post_markdown(app: tauri::AppHandle, slug: String) -> AppResult<String> {
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

    // 1. Local cache hit.
    if let Ok(content) = tokio::fs::read_to_string(&local_path).await {
        return Ok(content);
    }

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
    match cloudflare::download_from_r2(&client, &config, &key).await? {
        Some(content) => {
            // Cache locally for next time (best effort).
            let _ = tokio::fs::create_dir_all(&dir).await;
            let _ = tokio::fs::write(&local_path, &content).await;
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

    // 4. Swap the new body in. Only a rename is left, so the window in which the
    //    database and the file disagree is as small as it can be — and if even
    //    that fails, the metadata goes back rather than outliving its body.
    if let Err(e) = staged.commit(&dir.join(format!("{}.md", saved.slug))).await {
        restore_metadata(conn, previous, &saved).await;
        return Err(e);
    }

    // 5. Fingerprint what is now on this machine, so the difference between
    //    "published" and "published, and then edited" is recorded rather than
    //    inferred.
    let hash = sync_state::content_hash(&saved, &body);

    // 6. Draft → local only. Publish → push the body to R2 and metadata to D1.
    if !published {
        db::sync_set_local(conn, saved.id, hash).await?;
        return Ok(saved);
    }

    let assets_dir = app
        .path()
        .app_data_dir()
        .map_err(AppError::AppDataDir)?
        .join("assets");

    let synced = async {
        // Re-checked here, immediately before the first cloud write. The guard
        // at the top of this function ran several awaits ago — long enough to
        // stage a body, commit metadata and take a snapshot — and the post can
        // be thrown away in that window. What must not happen is publishing a
        // deleted post; the local half above is recoverable, this is not.
        if let Some(existing) = previous.as_ref() {
            refuse_if_trashed(conn, &existing.post).await?;
        }

        let (client, config) = cf()?;

        // Referenced local images → R2 under `posts/<slug>/<sha256>.<ext>`, and
        // the body's local `assets/<uuid>.<ext>` reference is rewritten to that
        // object's public URL. The published Markdown is then self-contained:
        // the blog renders it as-is with no rewriting step.
        //
        // Images go up before the body, so the body never lands referencing an
        // object that isn't there yet.
        let public_base = config.r2_public_url.trim_end_matches('/');
        if public_base.is_empty() {
            return Err(AppError::NoPublicUrl);
        }

        let mut published = body.clone();
        for r in extract_asset_refs(&body) {
            let Some((file_name, bytes)) = read_staged_asset(&assets_dir, &r).await else {
                continue;
            };
            let ext = file_name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
            let key = media_keys::media_key(&config.media_key_pattern, &saved.slug, &bytes, &ext);
            cloudflare::upload_bytes_to_r2(&client, &config, &key, bytes, content_type_for(&ext))
                .await?;
            published = published.replace(&r, &media_keys::public_url(public_base, &key));
        }

        // Body → R2.
        cloudflare::upload_to_r2(&client, &config, &media_keys::body_key(&saved.slug), &published)
            .await?;
        // Metadata → D1, with the series reference translated into the cloud's
        // ids — a local `series_id` would file the post under an unrelated
        // remote series.
        let outbound = post_for_cloud(conn, &client, &config, saved.clone()).await?;
        cloudflare::d1_post_upsert(&client, &config, outbound).await?;
        Ok::<(), AppError>(())
    }
    .await;

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
        db::sync_set_local(conn, saved.id, hash).await?;
    }

    match synced {
        Ok(()) => Ok(saved),
        Err(e) => Err(AppError::PublishSyncFailed(Box::new(e))),
    }
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

            staged
                .commit(&dir.join(format!("{}.md", saved.slug)))
                .await?;

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
        let file_name = match obj.key.strip_prefix("media/") {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => continue, // skip a folder marker, if any
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

/// Delete a media object from R2 and its local cache.
#[tauri::command]
pub async fn delete_media(app: tauri::AppHandle, key: String) -> AppResult<()> {
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
