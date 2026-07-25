//! Local SQLite cache, accessed through the full Sea ORM entity API.
//!
//! The database file lives in the app data dir and holds the offline editing
//! state that later syncs to Cloudflare D1 (see `cloudflare::d1_*`).

use sea_orm::{
    ActiveModelTrait, ConnectionTrait, Database, DatabaseConnection, EntityTrait, QueryOrder,
    Schema,
};
use tauri::Manager;

use crate::entities::post;

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

/// Create the `posts` table from the entity definition if it isn't there yet.
async fn ensure_schema(db: &DatabaseConnection) -> Result<(), String> {
    let backend = db.get_database_backend();
    let mut stmt = Schema::new(backend).create_table_from_entity(post::Entity);
    stmt.if_not_exists();
    db.execute(&stmt)
        .await
        .map_err(|e| format!("Failed to create schema: {e}"))?;
    Ok(())
}

// ─── CRUD ─────────────────────────────────────────────────────────────────────

pub async fn create(db: &DatabaseConnection, model: post::Model) -> Result<post::Model, String> {
    model
        .into_active_set()
        .insert(db)
        .await
        .map_err(|e| e.to_string())
}

pub async fn list(db: &DatabaseConnection) -> Result<Vec<post::Model>, String> {
    post::Entity::find()
        .order_by_desc(post::Column::LastUpdatedDate)
        .all(db)
        .await
        .map_err(|e| e.to_string())
}

pub async fn get(db: &DatabaseConnection, id: String) -> Result<Option<post::Model>, String> {
    post::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|e| e.to_string())
}

pub async fn update(db: &DatabaseConnection, model: post::Model) -> Result<post::Model, String> {
    let id = model.id.clone();
    let mut active = model.into_active_set();
    // Locate the row by its (unchanged) primary key; write the rest.
    active.id = sea_orm::ActiveValue::Unchanged(id);
    active.update(db).await.map_err(|e| e.to_string())
}

pub async fn delete(db: &DatabaseConnection, id: String) -> Result<(), String> {
    post::Entity::delete_by_id(id)
        .exec(db)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}
