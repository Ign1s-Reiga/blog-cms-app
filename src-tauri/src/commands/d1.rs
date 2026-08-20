//! Commands that write to Cloudflare D1.
//!
//! Several also touch the local cache first — a command lives with the
//! furthest-out store it writes, so a local-then-D1 operation belongs here
//! rather than in `local_db`.

use sea_orm::DatabaseConnection;
use sea_orm::TransactionTrait;
use sea_orm::DatabaseConnection as Db;
use tauri::State;
use crate::cloudflare::{self, cf};
use crate::db;
use crate::entities::post::Model as PostModel;
use crate::entities::post_schedule::Model as ScheduleModel;
use crate::entities::post_stage;
use crate::entities::series::Model as SeriesModel;
use crate::error::{AppError, AppResult};
use super::*;

// ── Posts: Cloudflare D1 ────────────────────────────────────────────────────────

#[tauri::command]
pub async fn d1_create_post(
    conn: State<'_, DatabaseConnection>,
    post: PostModel,
) -> AppResult<i64> {
    let (client, config) = cf()?;
    let mut post = post;
    let now = now_ts();
    post.created_at = now;
    post.updated_at = now;
    let post = post_for_cloud(conn.inner(), &client, &config, post).await?;
    cloudflare::d1_insert::<PostModel>(&client, &config, post).await
}

#[tauri::command]
pub async fn d1_list_posts() -> AppResult<Vec<PostModel>> {
    let (client, config) = cf()?;
    cloudflare::d1_list::<PostModel>(&client, &config).await
}

#[tauri::command]
pub async fn d1_get_post(id: i32) -> AppResult<Option<PostModel>> {
    let (client, config) = cf()?;
    cloudflare::d1_get::<PostModel>(&client, &config, id).await
}

#[tauri::command]
pub async fn d1_update_post(
    conn: State<'_, DatabaseConnection>,
    post: PostModel,
) -> AppResult<()> {
    let (client, config) = cf()?;
    let mut post = post;
    post.updated_at = now_ts();
    let post = post_for_cloud(conn.inner(), &client, &config, post).await?;
    cloudflare::d1_post_update(&client, &config, post).await
}

#[tauri::command]
pub async fn d1_delete_post(id: i32) -> AppResult<()> {
    let (client, config) = cf()?;
    cloudflare::d1_delete::<PostModel>(&client, &config, id).await
}

// ── Series: Cloudflare D1 ───────────────────────────────────────────────────────

#[tauri::command]
pub async fn d1_create_series(series: SeriesModel) -> AppResult<i64> {
    let (client, config) = cf()?;
    let mut series = series;
    series.created_at = now_ts();
    cloudflare::d1_insert::<SeriesModel>(&client, &config, series).await
}

#[tauri::command]
pub async fn d1_list_series() -> AppResult<Vec<SeriesModel>> {
    let (client, config) = cf()?;
    cloudflare::d1_list::<SeriesModel>(&client, &config).await
}

#[tauri::command]
pub async fn d1_get_series(id: i32) -> AppResult<Option<SeriesModel>> {
    let (client, config) = cf()?;
    cloudflare::d1_get::<SeriesModel>(&client, &config, id).await
}

#[tauri::command]
pub async fn d1_update_series(series: SeriesModel) -> AppResult<()> {
    let (client, config) = cf()?;
    cloudflare::d1_series_update(&client, &config, series).await
}

#[tauri::command]
pub async fn d1_delete_series(id: i32) -> AppResult<()> {
    let (client, config) = cf()?;
    cloudflare::d1_delete::<SeriesModel>(&client, &config, id).await
}

/// Promote a post to Published: stage it locally, flip `published`/`published_at`
/// in the local cache, and push that to Cloudflare D1.
#[tauri::command]
pub async fn publish_post(
    conn: State<'_, DatabaseConnection>,
    post_id: i32,
) -> AppResult<PostModel> {
    set_stage_and_sync(conn.inner(), post_id, true).await
}

/// Revert a post to Draft: the mirror of `publish_post`.
#[tauri::command]
pub async fn unpublish_post(
    conn: State<'_, DatabaseConnection>,
    post_id: i32,
) -> AppResult<PostModel> {
    set_stage_and_sync(conn.inner(), post_id, false).await
}

async fn set_stage_and_sync(conn: &Db, post_id: i32, publish: bool) -> AppResult<PostModel> {
    let now = now_ts();

    // 1. Flip the post's published state in the local cache.
    let mut post = db::get::<PostModel>(conn, post_id)
        .await?
        .ok_or(AppError::PostNotFound(post_id))?;
    // Publishing a post out of the trash would put a deleted article on the
    // blog; unpublishing one from there is a cloud write the trash deliberately
    // does not make on anybody's behalf.
    refuse_if_trashed(conn, &post).await?;
    post.published = publish;
    post.published_at = if publish { Some(now) } else { None };
    post.updated_at = now;
    let post = db::update::<PostModel>(conn, post).await?;

    // 2. Push the change to Cloudflare D1, with the post's series reference
    //    translated into the cloud's ids.
    let synced = match cf() {
        Ok((client, config)) => {
            match post_for_cloud(conn, &client, &config, post.clone()).await {
                Ok(outbound) => cloudflare::d1_post_update(&client, &config, outbound).await,
                Err(e) => Err(e),
            }
        }
        Err(e) => Err(e),
    };

    // 3. Record the resulting stage: the intended draft/published on success, or
    //    the sync-failed marker when the cloud push didn't complete.
    let stage = if synced.is_ok() {
        if publish { post_stage::PUBLISHED } else { post_stage::DRAFT }
    } else {
        post_stage::SYNC_FAILED
    };
    db::stage_set(
        conn,
        post_stage::Model { post_id, stage: stage.to_string(), staged_at: now },
    )
    .await?;

    // Same reasoning as the push in `sync_posts`: the cloud's row now carries
    // this post's `updated_at`, and a baseline left behind it turns our own
    // write into a remote change the next refresh has to ask about.
    if synced.is_ok() {
        if let Err(e) =
            db::sync_accept_remote_baseline(conn, post_id, Some(post.updated_at)).await
        {
            log::warn!("Could not record the pushed version for post {post_id}: {e}");
        }
    }

    match synced {
        Ok(()) => Ok(post),
        Err(e) => Err(AppError::CloudSyncFailed(Box::new(e))),
    }
}

// ─── Sync ───────────────────────────────────────────────────────────────────

/// Push every local post up to Cloudflare D1, upserting by `slug` (local wins).
/// A post that fails to push is marked `sync_failed`; a successful push clears
/// that back to its draft/published stage. Returns the number of posts synced;
/// errors with a summary if any failed.
#[tauri::command]
pub async fn sync_posts(conn: State<'_, DatabaseConnection>) -> AppResult<usize> {
    // Trash excluded: pushing a post the person has thrown away would put it
    // back on the blog, which is the opposite of what the button they pressed
    // last says they wanted.
    let posts = db::list_active_posts(conn.inner()).await?;
    let (client, config) = cf()?;
    let now = now_ts();

    // A local `series_id` is a local primary key and means nothing in D1, so it
    // is translated through the slug both databases agree on. Sending it raw
    // would file the post under whichever unrelated remote series happened to
    // land on that number.
    // Series go up first. A post's `series_id` can only be translated into a
    // remote id that exists, so a series made on this machine and never sent
    // would take every post in it across unfiled — the association would be
    // made in the app and lost in the crossing, with only a log line to say so.
    //
    // Upsert only. A series the cloud has and this machine does not is left
    // alone: "absent locally" means *not pulled yet* as often as it means
    // deleted, and a push is not the place to decide which. Deleting a series
    // here therefore unfiles its posts everywhere on the next push, and leaves
    // the empty series row in D1 for `d1_delete_series` to take.
    let local_series = db::list::<SeriesModel>(conn.inner()).await?;
    for series in local_series {
        let slug = series.slug.clone();
        if let Err(e) = cloudflare::d1_series_upsert(&client, &config, series).await {
            // Not fatal to the whole push: the posts are still worth sending,
            // and `apply_outbound` sends the ones in this series unfiled rather
            // than pointing at a row that is not there.
            log::warn!("Could not push series `{slug}`, its posts will go up unfiled: {e}");
        }
    }

    let remote_series = cloudflare::d1_list::<SeriesModel>(&client, &config).await?;
    let series = db::SeriesMap::build(conn.inner(), &remote_series).await?;

    let mut synced = 0usize;
    let mut failed = 0usize;
    for mut post in posts {
        let post_id = post.id;
        // Re-read per post rather than trusted from the listing above. A push
        // walks the whole library over the network, and a post can be thrown
        // away while it does — the listing is a snapshot from before that.
        if db::trash_get(conn.inner(), post_id).await?.is_some() {
            continue;
        }
        let published = post.published;
        // The version this push is about to give the cloud. `d1_post_upsert`
        // writes the model's own `updated_at`, so once it lands this is what the
        // remote row says — read before the model is moved into the call.
        let pushed_updated_at = post.updated_at;
        series.apply_outbound(&mut post);
        let stage = match cloudflare::d1_post_upsert(&client, &config, post).await {
            Ok(()) => {
                synced += 1;
                // Record the version we just wrote as the one this machine has
                // seen. Without it the baseline stays at whatever the last
                // refresh observed, and the *next* refresh finds the remote row
                // newer than that — our own push — and reads it as somebody
                // else's change. A post only this machine has ever touched then
                // reports a conflict against itself, and the app refuses to act
                // on it until a side is picked.
                //
                // Not `sync_mark_synced`: this pushes metadata alone. The body in
                // R2 is still the last published one, so the two sides are not
                // holding the same content and must not be recorded as if they
                // were — a post with unpublished text stays `modified`, which is
                // the truth about what readers are being served.
                if let Err(e) =
                    db::sync_accept_remote_baseline(conn.inner(), post_id, Some(pushed_updated_at))
                        .await
                {
                    log::warn!("Could not record the pushed version for post {post_id}: {e}");
                }
                if published { post_stage::PUBLISHED } else { post_stage::DRAFT }
            }
            Err(_) => {
                failed += 1;
                post_stage::SYNC_FAILED
            }
        };
        // Best-effort stage update; don't abort the whole sync on a staging error.
        let _ = db::stage_set(
            conn.inner(),
            post_stage::Model { post_id, stage: stage.to_string(), staged_at: now },
        )
        .await;
    }

    if failed > 0 {
        return Err(AppError::PartialSync { synced, failed });
    }
    Ok(synced)
}

/// Mirror the local cache to Cloudflare D1: upsert every remote post (cloud
/// wins) and delete local posts that no longer exist remotely, leaving the local
/// posts table an exact copy of D1. This is the "refresh" path — the UI reads
/// local data, and this brings it in sync on app launch and when the refresh
/// button is pressed. Returns the number of remote posts mirrored.
///
/// The cloud's series table comes down alongside the posts, because a post's
/// remote `series_id` cannot be read without it.
#[tauri::command]
pub async fn sync_posts_from_cloud(conn: State<'_, DatabaseConnection>) -> AppResult<usize> {
    let (client, config) = cf()?;
    let remote = cloudflare::d1_list::<PostModel>(&client, &config).await?;
    let remote_series = cloudflare::d1_list::<SeriesModel>(&client, &config).await?;

    // Bring down the series this machine does not have, before the posts, so a
    // series made on another machine exists here to be filed under.
    //
    // Adding only, in both directions of not-doing-things: a local series the
    // cloud lacks is not deleted, and a local series the cloud also has is not
    // overwritten — see `db::adopt_series_from_remote` for why a Refresh must
    // not discard a rename that has not been pushed yet. Which series a *post*
    // belongs to is decided by `db::resolve_series`, and this does not change
    // that.
    for series in &remote_series {
        if let Err(e) = db::adopt_series_from_remote(conn.inner(), series.clone()).await {
            log::warn!("Could not bring series `{}` down: {e}", series.slug);
        }
    }

    let mirrored = db::mirror_posts(conn.inner(), remote, &remote_series).await?;

    // A post whose cloud copy moved on has a cached body from before that move,
    // and a refresh does not fetch bodies — so the file on disk is an older
    // version of a post the metadata now describes as current.
    //
    // No file is deleted here. `mirror_posts` writes a `post_body_stale` row in
    // the same transaction as the metadata, and that row *is* the invalidation:
    // every reader of a cached body consults it. Deleting the file as well would
    // buy nothing and could take a body a save is writing at that moment — the
    // one file a refresh has no business touching. The cached copy is replaced
    // the next time the post is read, which fetches the cloud's current body and
    // settles the mark.

    // The schedules the Worker acts on live in D1 too, and the local copy is a
    // mirror of them: this is where the app learns that a publication it asked
    // for has happened, or failed. Best effort — the table may not exist until
    // the Worker's migration has been applied, and a refresh of the posts is
    // still worth having without it.
    match cloudflare::d1_list::<ScheduleModel>(&client, &config).await {
        Ok(remote_schedules) => {
            // In one transaction: the mirror is emptied before it is refilled,
            // and a post whose schedule has momentarily vanished is a post the
            // trash would agree to delete. See `db::mirror_schedules`.
            let txn = conn.inner().begin().await?;
            db::mirror_schedules(&txn, remote_schedules).await?;
            txn.commit().await?;
        }
        Err(e) => log::warn!("Could not refresh the publication schedules: {e}"),
    }

    log::info!(
        "Refreshed {} post(s) from the cloud, removing {} that are no longer there",
        mirrored.upserted,
        mirrored.deleted
    );
    Ok(mirrored.upserted)
}
