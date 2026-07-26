//! Local SQLite cache, accessed through the full Sea ORM entity API.
//!
//! The database file lives in the app data dir and holds the offline editing
//! state that later syncs to Cloudflare D1 (see `cloudflare::d1_*`). The tables
//! mirror the Drizzle schema (`series` and `blog-db`).

use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, Database, DatabaseConnection, EntityTrait,
    QueryFilter, QueryOrder, Schema, Set,
};
use tauri::Manager;

use crate::entities::{post, post_stage, series};

/// Open (creating if needed) the local SQLite database and ensure its schema
/// exists. Returns a connection to store in Tauri's managed state.
pub async fn connect(app: &tauri::AppHandle) -> Result<DatabaseConnection, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Cannot resolve app data dir: {e}"))?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("Failed to create data dir: {e}"))?;

    let db_path = dir.join("blog-cms.db");
    // `mode=rwc` opens read/write and creates the file if it doesn't exist.
    // Use forward slashes so the URL parses on Windows too.
    let url = format!(
        "sqlite:{}?mode=rwc",
        db_path.to_string_lossy().replace('\\', "/")
    );

    let db = Database::connect(&url)
        .await
        .map_err(|e| format!("Failed to open local database: {e}"))?;
    ensure_schema(&db).await?;
    Ok(db)
}

/// Create the tables from the entity definitions if they aren't there yet.
/// `series` is created first because `blog-db` references it.
async fn ensure_schema(db: &DatabaseConnection) -> Result<(), String> {
    let schema = Schema::new(db.get_database_backend());

    let mut series_tbl = schema.create_table_from_entity(series::Entity);
    series_tbl.if_not_exists();
    db.execute(&series_tbl)
        .await
        .map_err(|e| format!("Failed to create `series` table: {e}"))?;

    let mut post_tbl = schema.create_table_from_entity(post::Entity);
    post_tbl.if_not_exists();
    db.execute(&post_tbl)
        .await
        .map_err(|e| format!("Failed to create `blog-db` table: {e}"))?;

    // Local-only staging table (no D1 counterpart).
    let mut stage_tbl = schema.create_table_from_entity(post_stage::Entity);
    stage_tbl.if_not_exists();
    db.execute(&stage_tbl)
        .await
        .map_err(|e| format!("Failed to create `post_stage` table: {e}"))?;

    Ok(())
}

// ─── Posts ────────────────────────────────────────────────────────────────────

pub async fn post_create(db: &DatabaseConnection, model: post::Model) -> Result<post::Model, String> {
    model
        .into_insert()
        .insert(db)
        .await
        .map_err(|e| e.to_string())
}

pub async fn post_list(db: &DatabaseConnection) -> Result<Vec<post::Model>, String> {
    post::Entity::find()
        .order_by_desc(post::Column::CreatedAt)
        .all(db)
        .await
        .map_err(|e| e.to_string())
}

pub async fn post_get(db: &DatabaseConnection, id: i32) -> Result<Option<post::Model>, String> {
    post::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|e| e.to_string())
}

pub async fn post_update(db: &DatabaseConnection, model: post::Model) -> Result<post::Model, String> {
    model
        .into_update()
        .update(db)
        .await
        .map_err(|e| e.to_string())
}

pub async fn post_delete(db: &DatabaseConnection, id: i32) -> Result<(), String> {
    post::Entity::delete_by_id(id)
        .exec(db)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ─── Series ───────────────────────────────────────────────────────────────────

pub async fn series_create(
    db: &DatabaseConnection,
    model: series::Model,
) -> Result<series::Model, String> {
    model
        .into_insert()
        .insert(db)
        .await
        .map_err(|e| e.to_string())
}

pub async fn series_list(db: &DatabaseConnection) -> Result<Vec<series::Model>, String> {
    series::Entity::find()
        .order_by_desc(series::Column::CreatedAt)
        .all(db)
        .await
        .map_err(|e| e.to_string())
}

pub async fn series_get(db: &DatabaseConnection, id: i32) -> Result<Option<series::Model>, String> {
    series::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|e| e.to_string())
}

pub async fn series_update(
    db: &DatabaseConnection,
    model: series::Model,
) -> Result<series::Model, String> {
    model
        .into_update()
        .update(db)
        .await
        .map_err(|e| e.to_string())
}

pub async fn series_delete(db: &DatabaseConnection, id: i32) -> Result<(), String> {
    series::Entity::delete_by_id(id)
        .exec(db)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ─── Publish staging (local only) ───────────────────────────────────────────────

pub async fn stage_get(
    db: &DatabaseConnection,
    post_id: i32,
) -> Result<Option<post_stage::Model>, String> {
    post_stage::Entity::find_by_id(post_id)
        .one(db)
        .await
        .map_err(|e| e.to_string())
}

/// Upsert a post's staging row (there is one row per post).
pub async fn stage_set(
    db: &DatabaseConnection,
    model: post_stage::Model,
) -> Result<post_stage::Model, String> {
    let exists = post_stage::Entity::find_by_id(model.post_id)
        .one(db)
        .await
        .map_err(|e| e.to_string())?
        .is_some();

    if exists {
        let active = post_stage::ActiveModel {
            post_id: sea_orm::ActiveValue::Unchanged(model.post_id),
            stage: Set(model.stage),
            staged_at: Set(model.staged_at),
        };
        active.update(db).await.map_err(|e| e.to_string())
    } else {
        let active = post_stage::ActiveModel {
            post_id: Set(model.post_id),
            stage: Set(model.stage),
            staged_at: Set(model.staged_at),
        };
        active.insert(db).await.map_err(|e| e.to_string())
    }
}

/// Posts whose staging row is in `stage` (`"draft"` | `"published"`).
pub async fn posts_in_stage(
    db: &DatabaseConnection,
    stage: String,
) -> Result<Vec<post::Model>, String> {
    let ids: Vec<i32> = post_stage::Entity::find()
        .filter(post_stage::Column::Stage.eq(stage))
        .all(db)
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|s| s.post_id)
        .collect();

    if ids.is_empty() {
        return Ok(Vec::new());
    }

    post::Entity::find()
        .filter(post::Column::Id.is_in(ids))
        .order_by_desc(post::Column::CreatedAt)
        .all(db)
        .await
        .map_err(|e| e.to_string())
}

// ─── Sample data ────────────────────────────────────────────────────────────────

/// Populate an empty local database with a handful of sample posts (each also
/// staged draft/published). No-ops once the table has any rows.
pub async fn seed_sample_posts(db: &DatabaseConnection) -> Result<(), String> {
    if post::Entity::find()
        .one(db)
        .await
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Ok(());
    }

    let now = chrono::Utc::now().timestamp();
    // (slug, title, tags JSON, published)
    let samples: [(&str, &str, &str, bool); 5] = [
        ("getting-started-with-tauri-and-nextjs", "Getting Started with Tauri and Next.js", r#"["tauri","nextjs"]"#, true),
        ("cloudflare-r2-as-a-blog-storage-backend", "Cloudflare R2 as a Blog Storage Backend", r#"["cloudflare","storage"]"#, true),
        ("markdown-parsing-deep-dive", "Markdown Parsing Deep Dive", r#"["markdown"]"#, false),
        ("building-a-cms-with-rust", "Building a CMS with Rust", r#"["rust","cms"]"#, false),
        ("deploying-tauri-apps-to-windows", "Deploying Tauri Apps to Windows", r#"["tauri","deployment"]"#, true),
    ];

    for (i, (slug, title, tags, published)) in samples.into_iter().enumerate() {
        // Stagger dates a day apart so the list has a natural order.
        let ts = now - (i as i64) * 86_400;
        let created = post::Model {
            id: 0,
            slug: slug.to_string(),
            title: title.to_string(),
            excerpt: None,
            tags: Some(tags.to_string()),
            published,
            published_at: published.then_some(ts),
            series_id: None,
            series_order: None,
            created_at: ts,
            updated_at: ts,
        }
        .into_insert()
        .insert(db)
        .await
        .map_err(|e| e.to_string())?;

        let stage = if published { post_stage::PUBLISHED } else { post_stage::DRAFT };
        stage_set(
            db,
            post_stage::Model { post_id: created.id, stage: stage.to_string(), staged_at: ts },
        )
        .await?;
    }

    Ok(())
}
