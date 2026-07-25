//! Local SQLite cache, accessed through the full Sea ORM entity API.
//!
//! The database file lives in the app data dir and holds the offline editing
//! state that later syncs to Cloudflare D1 (see `cloudflare::d1_*`). The tables
//! mirror the Drizzle schema (`series` and `blog-db`).

use sea_orm::{
    ActiveModelTrait, ConnectionTrait, Database, DatabaseConnection, EntityTrait, QueryOrder,
    Schema,
};
use tauri::Manager;

use crate::entities::{post, series};

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
