//! Commands that touch only the local SQLite cache (and the app's own files).
//!
//! Nothing here reaches the network, so these work offline and without
//! Cloudflare credentials. Anything that also writes to the cloud lives in
//! `d1` or `r2` instead.

use sea_orm::DatabaseConnection;
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;
use crate::db;
use crate::entities::post::Model as PostModel;
use crate::entities::post_stage;
use crate::entities::series::Model as SeriesModel;
use crate::error::{AppError, AppResult};
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

#[tauri::command]
pub async fn list_posts(conn: State<'_, DatabaseConnection>) -> AppResult<Vec<PostModel>> {
    db::list::<PostModel>(conn.inner()).await
}

#[tauri::command]
pub async fn get_post(
    conn: State<'_, DatabaseConnection>,
    id: i32,
) -> AppResult<Option<PostModel>> {
    db::get::<PostModel>(conn.inner(), id).await
}

#[tauri::command]
pub async fn update_post(
    conn: State<'_, DatabaseConnection>,
    post: PostModel,
) -> AppResult<PostModel> {
    let mut post = post;
    post.updated_at = now_ts();
    db::update::<PostModel>(conn.inner(), post).await
}

#[tauri::command]
pub async fn delete_post(conn: State<'_, DatabaseConnection>, id: i32) -> AppResult<()> {
    db::delete::<PostModel>(conn.inner(), id).await
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
