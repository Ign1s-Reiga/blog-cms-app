//! Tauri commands, grouped by the store they act on.
//!
//! A command lives with the furthest-out store it writes: local-only in
//! `local_db`, anything reaching D1 in `d1`, anything reaching R2 in `r2`.
//! `save_post` touches all three and so sits in `r2`.
//!
//! The re-exports are globs by necessity: `#[tauri::command]` emits a
//! companion `__cmd__<name>` macro next to each function, and
//! `generate_handler!` needs both, so naming the functions individually
//! leaves the macros behind.

use sea_orm::DatabaseConnection;

use crate::cloudflare::{self, CloudflareConfig};
use crate::db;
use crate::entities::post::Model as PostModel;
use crate::entities::post_stage;
use crate::entities::series::Model as SeriesModel;
use crate::error::{AppError, AppResult};

mod d1;
mod local_db;
mod r2;

pub use d1::*;
pub use local_db::*;
pub use r2::*;

/// A post ready to send to D1, with its series reference translated out of
/// local ids.
///
/// **Every path that writes a post to the cloud goes through here** — the
/// editor's publish, the stage toggles, and the raw D1 commands. A local
/// `series_id` is a local primary key and means nothing in D1, so a path that
/// forgets this files the post under whichever unrelated remote series happens
/// to hold that number. Routing them all through one function is what keeps
/// that from depending on nobody forgetting.
///
/// It costs one extra D1 query per post pushed. `sync_posts` is the exception
/// and builds the map once for its whole batch; everywhere else pushes a single
/// post, where one query alongside the write is not worth optimising away.
async fn post_for_cloud(
    conn: &DatabaseConnection,
    client: &reqwest::Client,
    config: &CloudflareConfig,
    mut post: PostModel,
) -> AppResult<PostModel> {
    let remote_series = cloudflare::d1_list::<SeriesModel>(client, config).await?;
    db::SeriesMap::build(conn, &remote_series)
        .await?
        .apply_outbound(&mut post);
    Ok(post)
}

// ─── Shared helpers ───────────────────────────────────────────────────────────
//
// Only helpers with more than one caller across the modules above. Anything
// used by a single module lives in that module instead.

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

fn validate_stage(stage: &str) -> AppResult<()> {
    match stage {
        post_stage::DRAFT | post_stage::PUBLISHED | post_stage::SYNC_FAILED => Ok(()),
        other => Err(AppError::InvalidStage(other.to_string())),
    }
}
