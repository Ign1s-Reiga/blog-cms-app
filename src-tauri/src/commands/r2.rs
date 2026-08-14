//! Commands that read or write objects in R2.
//!
//! `save_post` touches all three stores; by the same rule as `d1` it lives
//! here, with the body and image handling that is its distinctive work.

use serde::Serialize;
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;
use sea_orm::DatabaseConnection;
use crate::cloudflare::{self, cf};
use crate::db;
use crate::entities::post::Model as PostModel;
use crate::entities::post_stage;
use crate::error::{AppError, AppResult};
use crate::imaging::{self, StagedImage};
use crate::media_keys;
use super::*;

/// A safe single path segment for the local media cache: no separators or `..`.
fn is_safe_file_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.contains('\\') && !name.contains("..")
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

    // 2. Not cached locally — download from R2 if we can reach it.
    let (client, config) = match cf() {
        Ok(cc) => cc,
        Err(_) => return Ok(String::new()), // offline / no credentials
    };
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
    let now = now_ts();

    // Start from the existing row (preserving slug/created_at/series/excerpt) or
    // build a fresh one for a new post.
    let mut model = match id {
        Some(id) => db::get::<PostModel>(conn.inner(), id)
            .await?
            .ok_or(AppError::PostNotFound(id))?,
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
    model.published = published;
    model.published_at = if published { model.published_at.or(Some(now)) } else { None };
    model.updated_at = now;

    // 1. Persist metadata locally.
    let saved = match id {
        Some(_) => db::update::<PostModel>(conn.inner(), model).await?,
        None => db::create::<PostModel>(conn.inner(), model).await?,
    };

    // 2. Write the Markdown body to the local cache.
    let dir = app
        .path()
        .app_data_dir()
        .map_err(AppError::AppDataDir)?
        .join("posts");
    let _ = tokio::fs::create_dir_all(&dir).await;
    tokio::fs::write(dir.join(format!("{}.md", saved.slug)), &body)
        .await
        .map_err(|e| AppError::io("Failed to write local markdown", e))?;

    // 3. Draft → local only. Publish → push the body to R2 and metadata to D1.
    if !published {
        db::stage_set(
            conn.inner(),
            post_stage::Model { post_id: saved.id, stage: post_stage::DRAFT.to_string(), staged_at: now },
        )
        .await?;
        return Ok(saved);
    }

    let assets_dir = app
        .path()
        .app_data_dir()
        .map_err(AppError::AppDataDir)?
        .join("assets");

    let synced = async {
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
            let file_name = r.strip_prefix("assets/").unwrap_or(&r);
            if let Ok(bytes) = tokio::fs::read(assets_dir.join(file_name)).await {
                let ext = file_name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
                let key =
                    media_keys::media_key(&config.media_key_pattern, &saved.slug, &bytes, &ext);
                cloudflare::upload_bytes_to_r2(&client, &config, &key, bytes, content_type_for(&ext))
                    .await?;
                published = published.replace(&r, &media_keys::public_url(public_base, &key));
            }
        }

        // Body → R2.
        cloudflare::upload_to_r2(&client, &config, &media_keys::body_key(&saved.slug), &published)
            .await?;
        // Metadata → D1.
        cloudflare::d1_post_upsert(&client, &config, saved.clone()).await?;
        Ok::<(), AppError>(())
    }
    .await;

    let stage = if synced.is_ok() { post_stage::PUBLISHED } else { post_stage::SYNC_FAILED };
    db::stage_set(
        conn.inner(),
        post_stage::Model { post_id: saved.id, stage: stage.to_string(), staged_at: now },
    )
    .await?;

    match synced {
        Ok(()) => Ok(saved),
        Err(e) => Err(AppError::PublishSyncFailed(Box::new(e))),
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
