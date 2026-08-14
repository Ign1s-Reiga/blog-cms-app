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

use crate::entities::post_stage;
use crate::error::{AppError, AppResult};

mod d1;
mod local_db;
mod r2;

pub use d1::*;
pub use local_db::*;
pub use r2::*;

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
