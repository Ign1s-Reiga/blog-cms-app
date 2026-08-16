//! Commands that write to Cloudflare D1.
//!
//! Several also touch the local cache first — a command lives with the
//! furthest-out store it writes, so a local-then-D1 operation belongs here
//! rather than in `local_db`.

use sea_orm::DatabaseConnection;
use sea_orm::DatabaseConnection as Db;
use tauri::State;
use crate::cloudflare::{self, cf};
use crate::db;
use crate::entities::post::Model as PostModel;
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
    let remote_series = cloudflare::d1_list::<SeriesModel>(&client, &config).await?;
    let series = db::SeriesMap::build(conn.inner(), &remote_series).await?;

    let mut synced = 0usize;
    let mut failed = 0usize;
    for mut post in posts {
        let post_id = post.id;
        let published = post.published;
        series.apply_outbound(&mut post);
        let stage = match cloudflare::d1_post_upsert(&client, &config, post).await {
            Ok(()) => {
                synced += 1;
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
    let (upserted, _deleted) = db::mirror_posts(conn.inner(), remote, &remote_series).await?;
    Ok(upserted)
}
