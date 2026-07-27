use std::path::PathBuf;

use sea_orm::DatabaseConnection;
use serde::Serialize;
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::cloudflare::{self, CloudflareConfig};
use crate::db;
use crate::entities::post::Model as PostModel;
use crate::entities::post_stage;
use crate::entities::series::Model as SeriesModel;
use sea_orm::DatabaseConnection as Db;

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

// ─── Frontmatter parser ───────────────────────────────────────────────────────

struct Frontmatter {
    title: Option<String>,
    tags:  Option<String>,
}

/// Parse YAML-style front matter delimited by `---`.
/// Recognises `title:` and `tags:` fields; ignores everything else.
fn parse_frontmatter(content: &str) -> Frontmatter {
    // Front matter must begin at the very first character.
    let body = match content.strip_prefix("---") {
        Some(s) => s,
        None => return Frontmatter { title: None, tags: None },
    };

    // Find the closing delimiter (handles both LF and CRLF).
    let end = body.find("\n---").or_else(|| body.find("\r\n---"));
    let block = match end {
        Some(pos) => &body[..pos],
        None => return Frontmatter { title: None, tags: None },
    };

    let mut title = None;
    let mut tags  = None;

    for raw in block.lines() {
        let line = raw.trim();
        if let Some(val) = line.strip_prefix("title:") {
            title = Some(
                val.trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string(),
            );
        } else if let Some(val) = line.strip_prefix("tags:") {
            // Accept both `tags: rust, tauri` and `tags: "rust, tauri"`
            tags = Some(
                val.trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string(),
            );
        }
    }

    Frontmatter { title, tags }
}

// ─── Command ──────────────────────────────────────────────────────────────────

/// Open a native file picker, upload the selected Markdown file to R2,
/// and register its metadata in D1.
///
/// Returns the post title on success.
/// Returns `Err("cancelled")` when the user dismisses the dialog without
/// choosing a file — the frontend treats this differently from real errors.
#[tauri::command]
pub async fn upload_article(
    app: tauri::AppHandle,
    conn: State<'_, DatabaseConnection>,
) -> Result<String, String> {
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
    .map_err(|e| format!("Dialog thread panicked: {e}"))?;

    // Resolve to a PathBuf; return "cancelled" if the dialog was dismissed.
    let file_path = match picked {
        None => return Err("cancelled".to_string()),
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        Some(tauri_plugin_dialog::FilePath::Path(p)) => p,
        #[allow(unreachable_patterns)]
        Some(_) => return Err("Unsupported path format on this platform".to_string()),
    };

    // ── 2. Read file ──────────────────────────────────────────────────────────
    let content = tokio::fs::read_to_string(&file_path)
        .await
        .map_err(|e| format!("Failed to read file: {e}"))?;

    // ── 3. Extract metadata ───────────────────────────────────────────────────
    let fm = parse_frontmatter(&content);

    let stem = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled");

    let title = fm.title.unwrap_or_else(|| stem.to_string());
    let tags  = fm.tags.unwrap_or_default();

    // ── 4. Derive slug + R2 key ───────────────────────────────────────────────
    // The id is auto-assigned by the DB, so the R2 object key is keyed by slug.
    let now = now_ts();
    let slug = {
        let s = slugify(&title);
        let s = if s.is_empty() { slugify(stem) } else { s };
        // Fall back to a unique, non-empty slug (e.g. non-ASCII titles).
        if s.is_empty() { format!("post-{now}") } else { s }
    };
    let r2_key = format!("posts/{slug}.md");

    // ── 5. Load Cloudflare credentials ───────────────────────────────────────
    let config = CloudflareConfig::from_env()?;
    let client = reqwest::Client::new();

    // ── 6. Upload to R2 ───────────────────────────────────────────────────────
    cloudflare::upload_to_r2(&client, &config, &r2_key, &content).await?;

    // ── 7. Record metadata in D1 and the local cache ─────────────────────────
    // R2 succeeded; mirror the metadata to D1, then cache it locally. If D1
    // fails we surface the error — the caller should decide whether to retry or
    // clean up the orphaned R2 object.
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
    cloudflare::d1_post_insert(&client, &config, post.clone()).await?;
    let created = db::post_create(conn.inner(), post).await?;
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

    Ok(title)
}

// ─── Metadata CRUD ────────────────────────────────────────────────────────────
//
// Local SQLite is the offline working store (full Sea ORM); the `d1_*` commands
// operate on Cloudflare D1 for cloud sync. Ids are auto-assigned by the database,
// so create ignores any incoming id and D1 creates return the new row id.
// `created_at` / `updated_at` are stamped server-side here.

/// A reqwest client plus credentials, built per call from the environment.
fn cf() -> Result<(reqwest::Client, CloudflareConfig), String> {
    Ok((reqwest::Client::new(), CloudflareConfig::from_env()?))
}

// ── Posts: local SQLite ─────────────────────────────────────────────────────────

#[tauri::command]
pub async fn create_post(
    conn: State<'_, DatabaseConnection>,
    post: PostModel,
) -> Result<PostModel, String> {
    let mut post = post;
    let now = now_ts();
    post.created_at = now;
    post.updated_at = now;
    let created = db::post_create(conn.inner(), post).await?;
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

#[tauri::command]
pub async fn list_posts(conn: State<'_, DatabaseConnection>) -> Result<Vec<PostModel>, String> {
    db::post_list(conn.inner()).await
}

#[tauri::command]
pub async fn get_post(
    conn: State<'_, DatabaseConnection>,
    id: i32,
) -> Result<Option<PostModel>, String> {
    db::post_get(conn.inner(), id).await
}

#[tauri::command]
pub async fn update_post(
    conn: State<'_, DatabaseConnection>,
    post: PostModel,
) -> Result<PostModel, String> {
    let mut post = post;
    post.updated_at = now_ts();
    db::post_update(conn.inner(), post).await
}

#[tauri::command]
pub async fn delete_post(conn: State<'_, DatabaseConnection>, id: i32) -> Result<(), String> {
    db::post_delete(conn.inner(), id).await
}

// ── Posts: Cloudflare D1 ────────────────────────────────────────────────────────

#[tauri::command]
pub async fn d1_create_post(post: PostModel) -> Result<i64, String> {
    let (client, config) = cf()?;
    let mut post = post;
    let now = now_ts();
    post.created_at = now;
    post.updated_at = now;
    cloudflare::d1_post_insert(&client, &config, post).await
}

#[tauri::command]
pub async fn d1_list_posts() -> Result<Vec<PostModel>, String> {
    let (client, config) = cf()?;
    cloudflare::d1_post_list(&client, &config).await
}

#[tauri::command]
pub async fn d1_get_post(id: i32) -> Result<Option<PostModel>, String> {
    let (client, config) = cf()?;
    cloudflare::d1_post_get(&client, &config, id).await
}

#[tauri::command]
pub async fn d1_update_post(post: PostModel) -> Result<(), String> {
    let (client, config) = cf()?;
    let mut post = post;
    post.updated_at = now_ts();
    cloudflare::d1_post_update(&client, &config, post).await
}

#[tauri::command]
pub async fn d1_delete_post(id: i32) -> Result<(), String> {
    let (client, config) = cf()?;
    cloudflare::d1_post_delete(&client, &config, id).await
}

// ── Series: local SQLite ────────────────────────────────────────────────────────

#[tauri::command]
pub async fn create_series(
    conn: State<'_, DatabaseConnection>,
    series: SeriesModel,
) -> Result<SeriesModel, String> {
    let mut series = series;
    series.created_at = now_ts();
    db::series_create(conn.inner(), series).await
}

#[tauri::command]
pub async fn list_series(conn: State<'_, DatabaseConnection>) -> Result<Vec<SeriesModel>, String> {
    db::series_list(conn.inner()).await
}

#[tauri::command]
pub async fn get_series(
    conn: State<'_, DatabaseConnection>,
    id: i32,
) -> Result<Option<SeriesModel>, String> {
    db::series_get(conn.inner(), id).await
}

#[tauri::command]
pub async fn update_series(
    conn: State<'_, DatabaseConnection>,
    series: SeriesModel,
) -> Result<SeriesModel, String> {
    db::series_update(conn.inner(), series).await
}

#[tauri::command]
pub async fn delete_series(conn: State<'_, DatabaseConnection>, id: i32) -> Result<(), String> {
    db::series_delete(conn.inner(), id).await
}

// ── Series: Cloudflare D1 ───────────────────────────────────────────────────────

#[tauri::command]
pub async fn d1_create_series(series: SeriesModel) -> Result<i64, String> {
    let (client, config) = cf()?;
    let mut series = series;
    series.created_at = now_ts();
    cloudflare::d1_series_insert(&client, &config, series).await
}

#[tauri::command]
pub async fn d1_list_series() -> Result<Vec<SeriesModel>, String> {
    let (client, config) = cf()?;
    cloudflare::d1_series_list(&client, &config).await
}

#[tauri::command]
pub async fn d1_get_series(id: i32) -> Result<Option<SeriesModel>, String> {
    let (client, config) = cf()?;
    cloudflare::d1_series_get(&client, &config, id).await
}

#[tauri::command]
pub async fn d1_update_series(series: SeriesModel) -> Result<(), String> {
    let (client, config) = cf()?;
    cloudflare::d1_series_update(&client, &config, series).await
}

#[tauri::command]
pub async fn d1_delete_series(id: i32) -> Result<(), String> {
    let (client, config) = cf()?;
    cloudflare::d1_series_delete(&client, &config, id).await
}

// ─── Publish staging ────────────────────────────────────────────────────────
//
// A local-only staging table records each post's editorial stage
// (`draft`/`published`). `set_post_stage` only touches that table; `publish_post`
// / `unpublish_post` also flip the post's `published` field locally and push the
// change to Cloudflare D1.

fn validate_stage(stage: &str) -> Result<(), String> {
    match stage {
        post_stage::DRAFT | post_stage::PUBLISHED | post_stage::SYNC_FAILED => Ok(()),
        other => Err(format!(
            "Invalid stage `{other}` (expected `draft`, `published`, or `sync_failed`)"
        )),
    }
}

/// Set (or clear) a post's local staging stage without publishing.
#[tauri::command]
pub async fn set_post_stage(
    conn: State<'_, DatabaseConnection>,
    post_id: i32,
    stage: String,
) -> Result<post_stage::Model, String> {
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
) -> Result<Option<post_stage::Model>, String> {
    db::stage_get(conn.inner(), post_id).await
}

#[tauri::command]
pub async fn list_posts_by_stage(
    conn: State<'_, DatabaseConnection>,
    stage: String,
) -> Result<Vec<PostModel>, String> {
    validate_stage(&stage)?;
    db::posts_in_stage(conn.inner(), stage).await
}

/// Promote a post to Published: stage it locally, flip `published`/`published_at`
/// in the local cache, and push that to Cloudflare D1.
#[tauri::command]
pub async fn publish_post(
    conn: State<'_, DatabaseConnection>,
    post_id: i32,
) -> Result<PostModel, String> {
    set_stage_and_sync(conn.inner(), post_id, true).await
}

/// Revert a post to Draft: the mirror of `publish_post`.
#[tauri::command]
pub async fn unpublish_post(
    conn: State<'_, DatabaseConnection>,
    post_id: i32,
) -> Result<PostModel, String> {
    set_stage_and_sync(conn.inner(), post_id, false).await
}

async fn set_stage_and_sync(conn: &Db, post_id: i32, publish: bool) -> Result<PostModel, String> {
    let now = now_ts();

    // 1. Flip the post's published state in the local cache.
    let mut post = db::post_get(conn, post_id)
        .await?
        .ok_or_else(|| format!("post {post_id} not found"))?;
    post.published = publish;
    post.published_at = if publish { Some(now) } else { None };
    post.updated_at = now;
    let post = db::post_update(conn, post).await?;

    // 2. Push the change to Cloudflare D1.
    let synced = match cf() {
        Ok((client, config)) => cloudflare::d1_post_update(&client, &config, post.clone()).await,
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
        Err(e) => Err(format!("post updated locally but cloud sync failed: {e}")),
    }
}

// ─── Sync ───────────────────────────────────────────────────────────────────

/// Push every local post up to Cloudflare D1, upserting by `slug` (local wins).
/// A post that fails to push is marked `sync_failed`; a successful push clears
/// that back to its draft/published stage. Returns the number of posts synced;
/// errors with a summary if any failed.
#[tauri::command]
pub async fn sync_posts(conn: State<'_, DatabaseConnection>) -> Result<usize, String> {
    let posts = db::post_list(conn.inner()).await?;
    let (client, config) = cf()?;
    let now = now_ts();

    let mut synced = 0usize;
    let mut failed = 0usize;
    for post in posts {
        let post_id = post.id;
        let published = post.published;
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
        return Err(format!("synced {synced}, {failed} failed to sync"));
    }
    Ok(synced)
}

// ─── Post content ───────────────────────────────────────────────────────────

/// Read a post's Markdown body for the editor.
///
/// Prefers the local cache (`<app_data>/posts/<slug>.md`). If it isn't cached
/// locally but exists on R2, it's downloaded and cached so the editor can open
/// it offline next time. Returns an empty string when the post has no content
/// yet (nothing local and nothing on R2), or when the cloud is unreachable.
#[tauri::command]
pub async fn read_post_markdown(
    app: tauri::AppHandle,
    conn: State<'_, DatabaseConnection>,
    id: i32,
) -> Result<String, String> {
    let post = db::post_get(conn.inner(), id)
        .await?
        .ok_or_else(|| format!("post {id} not found"))?;

    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Cannot resolve app data dir: {e}"))?
        .join("posts");
    let local_path = dir.join(format!("{}.md", post.slug));

    // 1. Local cache hit.
    if let Ok(content) = tokio::fs::read_to_string(&local_path).await {
        return Ok(content);
    }

    // 2. Not cached locally — download from R2 if we can reach it.
    let (client, config) = match cf() {
        Ok(cc) => cc,
        Err(_) => return Ok(String::new()), // offline / no credentials
    };
    let key = format!("posts/{}.md", post.slug);
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
) -> Result<PostModel, String> {
    let now = now_ts();

    // Start from the existing row (preserving slug/created_at/series/excerpt) or
    // build a fresh one for a new post.
    let mut model = match id {
        Some(id) => db::post_get(conn.inner(), id)
            .await?
            .ok_or_else(|| format!("post {id} not found"))?,
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
        Some(_) => db::post_update(conn.inner(), model).await?,
        None => db::post_create(conn.inner(), model).await?,
    };

    // 2. Write the Markdown body to the local cache.
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Cannot resolve app data dir: {e}"))?
        .join("posts");
    let _ = tokio::fs::create_dir_all(&dir).await;
    tokio::fs::write(dir.join(format!("{}.md", saved.slug)), &body)
        .await
        .map_err(|e| format!("Failed to write local markdown: {e}"))?;

    // 3. Draft → local only. Publish → push the body to R2 and metadata to D1.
    if !published {
        db::stage_set(
            conn.inner(),
            post_stage::Model { post_id: saved.id, stage: post_stage::DRAFT.to_string(), staged_at: now },
        )
        .await?;
        return Ok(saved);
    }

    let synced = async {
        let (client, config) = cf()?;
        cloudflare::upload_to_r2(&client, &config, &format!("posts/{}.md", saved.slug), &body).await?;
        cloudflare::d1_post_upsert(&client, &config, saved.clone()).await?;
        Ok::<(), String>(())
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
        Err(e) => Err(format!("post saved locally but publish sync failed: {e}")),
    }
}

// ─── Image staging ──────────────────────────────────────────────────────────

/// A dropped image after it has been copied into the local assets directory.
#[derive(Serialize)]
pub struct StagedImage {
    /// Markdown-relative reference, e.g. `"assets/<uuid>.png"`.
    pub rel: String,
    /// Original file name — used as the inserted image's alt text.
    pub name: String,
}

/// Copy a dropped image into the app's local `assets` directory so it can be
/// referenced from a post and rendered in the preview via the asset protocol.
/// Cloud (R2) upload is deferred to the save/publish sync.
///
/// `src_path` is an absolute path from an OS drag-and-drop. The extension is
/// validated against a fixed allow-list; other files are rejected.
#[tauri::command]
pub async fn stage_image(app: tauri::AppHandle, src_path: String) -> Result<StagedImage, String> {
    let src = PathBuf::from(&src_path);

    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|e| {
            matches!(
                e.as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "svg" | "bmp" | "ico"
            )
        })
        .ok_or_else(|| format!("Unsupported image type: {src_path}"))?;

    let assets_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Cannot resolve app data dir: {e}"))?
        .join("assets");
    tokio::fs::create_dir_all(&assets_dir)
        .await
        .map_err(|e| format!("Failed to create assets dir: {e}"))?;

    let file_name = format!("{}.{ext}", uuid::Uuid::new_v4());
    let dest = assets_dir.join(&file_name);
    tokio::fs::copy(&src, &dest)
        .await
        .map_err(|e| format!("Failed to copy image: {e}"))?;

    let name = src
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("image")
        .to_string();

    Ok(StagedImage { rel: format!("assets/{file_name}"), name })
}
