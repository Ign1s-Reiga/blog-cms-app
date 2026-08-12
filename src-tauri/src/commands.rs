use sea_orm::DatabaseConnection;
use serde::Serialize;
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::cloudflare::{self, cf};
use crate::db;
use crate::entities::post::Model as PostModel;
use crate::entities::post_stage;
use crate::entities::series::Model as SeriesModel;
use crate::imaging::{self, StagedImage};
use crate::media_keys;
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
    let slug = {
        let s = slugify(&title);
        let s = if s.is_empty() { slugify(stem) } else { s };
        // Fall back to a unique, non-empty slug (e.g. non-ASCII titles).
        if s.is_empty() { format!("post-{now}") } else { s }
    };
    // ── 5. Cache the body locally ────────────────────────────────────────────
    // An import lands as a draft, and a draft is local-only — the same rule
    // `save_post` follows. Nothing reaches R2 or D1 until the post is published
    // from the editor or pushed with "Push to cloud", so importing needs no
    // credentials and never puts an unpublished body in the bucket.
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Cannot resolve app data dir: {e}"))?
        .join("posts");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("Failed to create posts dir: {e}"))?;
    tokio::fs::write(dir.join(format!("{slug}.md")), body)
        .await
        .map_err(|e| format!("Failed to write local markdown: {e}"))?;

    // ── 6. Record the metadata locally ───────────────────────────────────────
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

#[tauri::command]
pub async fn list_posts(conn: State<'_, DatabaseConnection>) -> Result<Vec<PostModel>, String> {
    db::list::<PostModel>(conn.inner()).await
}

#[tauri::command]
pub async fn get_post(
    conn: State<'_, DatabaseConnection>,
    id: i32,
) -> Result<Option<PostModel>, String> {
    db::get::<PostModel>(conn.inner(), id).await
}

#[tauri::command]
pub async fn update_post(
    conn: State<'_, DatabaseConnection>,
    post: PostModel,
) -> Result<PostModel, String> {
    let mut post = post;
    post.updated_at = now_ts();
    db::update::<PostModel>(conn.inner(), post).await
}

#[tauri::command]
pub async fn delete_post(conn: State<'_, DatabaseConnection>, id: i32) -> Result<(), String> {
    db::delete::<PostModel>(conn.inner(), id).await
}

// ── Posts: Cloudflare D1 ────────────────────────────────────────────────────────

#[tauri::command]
pub async fn d1_create_post(post: PostModel) -> Result<i64, String> {
    let (client, config) = cf()?;
    let mut post = post;
    let now = now_ts();
    post.created_at = now;
    post.updated_at = now;
    cloudflare::d1_insert::<PostModel>(&client, &config, post).await
}

#[tauri::command]
pub async fn d1_list_posts() -> Result<Vec<PostModel>, String> {
    let (client, config) = cf()?;
    cloudflare::d1_list::<PostModel>(&client, &config).await
}

#[tauri::command]
pub async fn d1_get_post(id: i32) -> Result<Option<PostModel>, String> {
    let (client, config) = cf()?;
    cloudflare::d1_get::<PostModel>(&client, &config, id).await
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
    cloudflare::d1_delete::<PostModel>(&client, &config, id).await
}

// ── Series: local SQLite ────────────────────────────────────────────────────────

#[tauri::command]
pub async fn create_series(
    conn: State<'_, DatabaseConnection>,
    series: SeriesModel,
) -> Result<SeriesModel, String> {
    let mut series = series;
    series.created_at = now_ts();
    db::create::<SeriesModel>(conn.inner(), series).await
}

#[tauri::command]
pub async fn list_series(conn: State<'_, DatabaseConnection>) -> Result<Vec<SeriesModel>, String> {
    db::list::<SeriesModel>(conn.inner()).await
}

#[tauri::command]
pub async fn get_series(
    conn: State<'_, DatabaseConnection>,
    id: i32,
) -> Result<Option<SeriesModel>, String> {
    db::get::<SeriesModel>(conn.inner(), id).await
}

#[tauri::command]
pub async fn update_series(
    conn: State<'_, DatabaseConnection>,
    series: SeriesModel,
) -> Result<SeriesModel, String> {
    db::update::<SeriesModel>(conn.inner(), series).await
}

#[tauri::command]
pub async fn delete_series(conn: State<'_, DatabaseConnection>, id: i32) -> Result<(), String> {
    db::delete::<SeriesModel>(conn.inner(), id).await
}

// ── Series: Cloudflare D1 ───────────────────────────────────────────────────────

#[tauri::command]
pub async fn d1_create_series(series: SeriesModel) -> Result<i64, String> {
    let (client, config) = cf()?;
    let mut series = series;
    series.created_at = now_ts();
    cloudflare::d1_insert::<SeriesModel>(&client, &config, series).await
}

#[tauri::command]
pub async fn d1_list_series() -> Result<Vec<SeriesModel>, String> {
    let (client, config) = cf()?;
    cloudflare::d1_list::<SeriesModel>(&client, &config).await
}

#[tauri::command]
pub async fn d1_get_series(id: i32) -> Result<Option<SeriesModel>, String> {
    let (client, config) = cf()?;
    cloudflare::d1_get::<SeriesModel>(&client, &config, id).await
}

#[tauri::command]
pub async fn d1_update_series(series: SeriesModel) -> Result<(), String> {
    let (client, config) = cf()?;
    cloudflare::d1_series_update(&client, &config, series).await
}

#[tauri::command]
pub async fn d1_delete_series(id: i32) -> Result<(), String> {
    let (client, config) = cf()?;
    cloudflare::d1_delete::<SeriesModel>(&client, &config, id).await
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
    let mut post = db::get::<PostModel>(conn, post_id)
        .await?
        .ok_or_else(|| format!("post {post_id} not found"))?;
    post.published = publish;
    post.published_at = if publish { Some(now) } else { None };
    post.updated_at = now;
    let post = db::update::<PostModel>(conn, post).await?;

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
    let posts = db::list::<PostModel>(conn.inner()).await?;
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

/// Mirror the local cache to Cloudflare D1: upsert every remote post (cloud
/// wins) and delete local posts that no longer exist remotely, leaving the local
/// posts table an exact copy of D1. This is the "refresh" path — the UI reads
/// local data, and this brings it in sync on app launch and when the refresh
/// button is pressed. Returns the number of remote posts mirrored.
#[tauri::command]
pub async fn sync_posts_from_cloud(conn: State<'_, DatabaseConnection>) -> Result<usize, String> {
    let (client, config) = cf()?;
    let remote = cloudflare::d1_list::<PostModel>(&client, &config).await?;
    let (upserted, _deleted) = db::mirror_posts(conn.inner(), remote).await?;
    Ok(upserted)
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
pub async fn read_post_markdown(app: tauri::AppHandle, slug: String) -> Result<String, String> {
    // The slug builds a local file path and an R2 key, so reject anything that
    // isn't a strict slug (guards against path traversal / injection).
    if !media_keys::is_safe_slug(&slug) {
        return Err(format!("Invalid post slug: {slug}"));
    }

    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Cannot resolve app data dir: {e}"))?
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
) -> Result<PostModel, String> {
    let now = now_ts();

    // Start from the existing row (preserving slug/created_at/series/excerpt) or
    // build a fresh one for a new post.
    let mut model = match id {
        Some(id) => db::get::<PostModel>(conn.inner(), id)
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
        Some(_) => db::update::<PostModel>(conn.inner(), model).await?,
        None => db::create::<PostModel>(conn.inner(), model).await?,
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

    let assets_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Cannot resolve app data dir: {e}"))?
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
            return Err(
                "No R2 public URL is configured, so image links cannot be written. \
                 Sign out and sign in again to set it."
                    .to_string(),
            );
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
pub async fn upload_media(app: tauri::AppHandle) -> Result<MediaItem, String> {
    let app_clone = app.clone();
    let picked = tokio::task::spawn_blocking(move || {
        app_clone
            .dialog()
            .file()
            .add_filter("Media", MEDIA_EXTS)
            .blocking_pick_file()
    })
    .await
    .map_err(|e| format!("Dialog thread panicked: {e}"))?;

    let src = match picked {
        None => return Err("cancelled".to_string()),
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        Some(tauri_plugin_dialog::FilePath::Path(p)) => p,
        #[allow(unreachable_patterns)]
        Some(_) => return Err("Unsupported path format on this platform".to_string()),
    };

    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|e| MEDIA_EXTS.contains(&e.as_str()))
        .ok_or_else(|| "Unsupported media type".to_string())?;

    let bytes = tokio::fs::read(&src)
        .await
        .map_err(|e| format!("Failed to read file: {e}"))?;

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
        .map_err(|e| format!("Cannot resolve app data dir: {e}"))?
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
) -> Result<StagedImage, String> {
    let file_name = key
        .strip_prefix("media/")
        .filter(|n| is_safe_file_name(n))
        .ok_or_else(|| format!("Not a media library key: {key}"))?
        .to_string();

    let ext = file_name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if ext.is_empty() {
        return Err(format!("Media object has no extension: {key}"));
    }

    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Cannot resolve app data dir: {e}"))?;

    // Prefer the local cache the media library already maintains; fall back to
    // R2 for an object listed but not yet cached.
    let cached = data_dir.join("media").join(&file_name);
    let bytes = match tokio::fs::read(&cached).await {
        Ok(bytes) => bytes,
        Err(_) => {
            let (client, config) = cf()?;
            cloudflare::download_bytes_from_r2(&client, &config, &key)
                .await?
                .ok_or_else(|| format!("Media object not found: {key}"))?
        }
    };

    let assets_dir = data_dir.join("assets");
    tokio::fs::create_dir_all(&assets_dir)
        .await
        .map_err(|e| format!("Failed to create assets dir: {e}"))?;

    let staged_name = format!("{}.{ext}", uuid::Uuid::new_v4());
    tokio::fs::write(assets_dir.join(&staged_name), &bytes)
        .await
        .map_err(|e| format!("Failed to stage media: {e}"))?;

    Ok(StagedImage { rel: format!("assets/{staged_name}"), name: file_name })
}

/// List media objects in R2 (prefix `media/`), caching any not already local.
#[tauri::command]
pub async fn list_media(app: tauri::AppHandle) -> Result<Vec<MediaItem>, String> {
    let (client, config) = cf()?;
    let objects = cloudflare::list_r2(&client, &config, "media/").await?;

    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Cannot resolve app data dir: {e}"))?
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
pub async fn delete_media(app: tauri::AppHandle, key: String) -> Result<(), String> {
    let (client, config) = cf()?;
    cloudflare::delete_from_r2(&client, &config, &key).await?;

    if let Some(file_name) = key.strip_prefix("media/") {
        // Only touch the local cache for a safe single filename (no traversal).
        if is_safe_file_name(file_name) {
            let local = app
                .path()
                .app_data_dir()
                .map_err(|e| format!("Cannot resolve app data dir: {e}"))?
                .join("media")
                .join(file_name);
            let _ = tokio::fs::remove_file(local).await;
        }
    }
    Ok(())
}
