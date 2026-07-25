use std::path::PathBuf;

use serde::Serialize;
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

use crate::cloudflare::{insert_d1_record, upload_to_r2, CloudflareConfig};

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
pub async fn upload_article(app: tauri::AppHandle) -> Result<String, String> {
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

    // ── 4. Generate stable identifiers ───────────────────────────────────────
    let id       = uuid::Uuid::new_v4().to_string();
    let now      = chrono::Utc::now().to_rfc3339();
    let r2_key   = format!("posts/{id}.md");

    // ── 5. Load Cloudflare credentials ───────────────────────────────────────
    let config = CloudflareConfig::from_env()?;
    let client = reqwest::Client::new();

    // ── 6. Upload to R2 ───────────────────────────────────────────────────────
    upload_to_r2(&client, &config, &r2_key, &content).await?;

    // ── 7. Insert D1 record ───────────────────────────────────────────────────
    // R2 succeeded; attempt D1. If D1 fails we surface the error — the caller
    // should decide whether to retry or clean up the orphaned R2 object.
    insert_d1_record(&client, &config, &id, &title, &now, &now, &tags).await?;

    Ok(title)
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
