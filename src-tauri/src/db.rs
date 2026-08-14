//! Local SQLite cache, accessed through the full Sea ORM entity API.
//!
//! The database file lives in the app data dir and holds the offline editing
//! state that later syncs to Cloudflare D1 (see `cloudflare::d1_*`). The tables
//! mirror the Drizzle schema (`series` and `blog-db`).

use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ColumnTrait, ConnectionTrait, Database,
    DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, Schema, Set,
};
use tauri::Manager;

use crate::entities::record::{Id, Record};
use crate::entities::{post, post_stage, series};
use crate::error::{AppError, AppResult};

/// Open (creating if needed) the local SQLite database and ensure its schema
/// exists. Returns a connection to store in Tauri's managed state.
pub async fn connect(app: &tauri::AppHandle) -> AppResult<DatabaseConnection> {
    let dir = app.path().app_data_dir().map_err(AppError::AppDataDir)?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::io("Failed to create data dir", e))?;

    let db_path = dir.join("blog-cms.db");
    // `mode=rwc` opens read/write and creates the file if it doesn't exist.
    // Use forward slashes so the URL parses on Windows too.
    let url = format!(
        "sqlite:{}?mode=rwc",
        db_path.to_string_lossy().replace('\\', "/")
    );

    let db = Database::connect(&url)
        .await
        .map_err(|e| AppError::db_init("Failed to open local database", e))?;
    ensure_schema(&db).await?;
    Ok(db)
}

/// An empty database with the schema applied, held in memory for the length of
/// the connection. For tests that need real SQL — a unique-constraint violation
/// is not something a fake can be trusted to reproduce.
///
/// The pool is pinned to one connection because each connection to
/// `sqlite::memory:` opens its *own* database: a second one would find no
/// tables, and which one a query landed on would be down to pool scheduling.
#[cfg(test)]
pub async fn connect_in_memory() -> AppResult<DatabaseConnection> {
    let mut options = sea_orm::ConnectOptions::new("sqlite::memory:");
    options.max_connections(1).min_connections(1);

    let db = Database::connect(options)
        .await
        .map_err(|e| AppError::db_init("Failed to open in-memory database", e))?;
    ensure_schema(&db).await?;
    Ok(db)
}

/// Create the tables from the entity definitions if they aren't there yet.
/// `series` is created first because `blog-db` references it.
async fn ensure_schema(db: &DatabaseConnection) -> AppResult<()> {
    let schema = Schema::new(db.get_database_backend());

    let mut series_tbl = schema.create_table_from_entity(series::Entity);
    series_tbl.if_not_exists();
    db.execute(&series_tbl)
        .await
        .map_err(|e| AppError::db_init("Failed to create `series` table", e))?;

    let mut post_tbl = schema.create_table_from_entity(post::Entity);
    post_tbl.if_not_exists();
    db.execute(&post_tbl)
        .await
        .map_err(|e| AppError::db_init("Failed to create `blog-db` table", e))?;

    // Local-only staging table (no D1 counterpart).
    let mut stage_tbl = schema.create_table_from_entity(post_stage::Entity);
    stage_tbl.if_not_exists();
    db.execute(&stage_tbl)
        .await
        .map_err(|e| AppError::db_init("Failed to create `post_stage` table", e))?;

    Ok(())
}

// ─── CRUD ─────────────────────────────────────────────────────────────────────
//
// One implementation per operation, shared by every entity that implements
// `Record`. `post` and `series` previously had five identical functions each,
// differing only in the type they named.
//
// Each takes `&impl ConnectionTrait` rather than `&DatabaseConnection` so a
// caller can hand it either the pool or an open transaction. `save_post` needs
// the latter: a post's row and its staging row have to land together or not at
// all, or a failed save leaves the two disagreeing about what happened.
//
// `impl Trait` rather than a named parameter deliberately — it keeps the
// connection out of the turbofish, so every `db::get::<PostModel>(..)` call
// site reads exactly as it did.

/// Insert a model, returning it as stored (with its assigned primary key).
pub async fn create<M>(db: &impl ConnectionTrait, model: M) -> AppResult<M>
where
    M: Record + IntoActiveModel<<M::Entity as EntityTrait>::ActiveModel>,
    <M::Entity as EntityTrait>::ActiveModel:
        ActiveModelTrait<Entity = M::Entity> + ActiveModelBehavior + Send,
{
    Ok(model.into_insert().insert(db).await?)
}

/// Every row, newest first by the record's own ordering column.
pub async fn list<M: Record>(db: &impl ConnectionTrait) -> AppResult<Vec<M>> {
    Ok(M::Entity::find()
        .order_by_desc(M::order_column())
        .all(db)
        .await?)
}

/// One row by primary key, or `None` when it does not exist.
pub async fn get<M: Record>(db: &impl ConnectionTrait, id: Id<M>) -> AppResult<Option<M>> {
    Ok(M::Entity::find_by_id(id).one(db).await?)
}

/// Overwrite the row this model's primary key points at.
pub async fn update<M>(db: &impl ConnectionTrait, model: M) -> AppResult<M>
where
    M: Record + IntoActiveModel<<M::Entity as EntityTrait>::ActiveModel>,
    <M::Entity as EntityTrait>::ActiveModel:
        ActiveModelTrait<Entity = M::Entity> + ActiveModelBehavior + Send,
{
    Ok(model.into_update().update(db).await?)
}

/// Delete by primary key. Deleting an absent row is not an error.
pub async fn delete<M: Record>(db: &impl ConnectionTrait, id: Id<M>) -> AppResult<()> {
    M::Entity::delete_by_id(id).exec(db).await?;
    Ok(())
}

/// The post with this slug, if there is one. The column is unique, so at most
/// one row can match.
pub async fn post_by_slug(db: &impl ConnectionTrait, slug: &str) -> AppResult<Option<post::Model>> {
    Ok(post::Entity::find()
        .filter(post::Column::Slug.eq(slug))
        .one(db)
        .await?)
}

/// Mirror the local posts table onto the cloud's set of posts, keyed by `slug`.
///
/// The cloud is authoritative: every remote post is upserted into the local
/// cache (overwriting the local copy), and local posts whose slug isn't in the
/// remote set are deleted — so `local == remote` afterwards. Unsynced local-only
/// drafts are therefore removed. Returns `(upserted, deleted)`.
pub async fn mirror_posts(
    db: &DatabaseConnection,
    remote: Vec<post::Model>,
) -> AppResult<(usize, usize)> {
    let remote_slugs: std::collections::HashSet<String> =
        remote.iter().map(|p| p.slug.clone()).collect();
    let upserted = remote.len();

    for post in remote {
        upsert_post_from_remote(db, post).await?;
    }

    // Drop anything local that no longer exists remotely (+ its staging row).
    let locals = post::Entity::find().all(db).await?;
    let mut deleted = 0usize;
    for local in locals {
        if !remote_slugs.contains(&local.slug) {
            let _ = post_stage::Entity::delete_by_id(local.id).exec(db).await;
            post::Entity::delete_by_id(local.id).exec(db).await?;
            deleted += 1;
        }
    }

    Ok((upserted, deleted))
}

/// Upsert one remote post into the local cache, keyed by `slug` (cloud wins).
///
/// An existing local row (matched by slug) is overwritten in place, keeping its
/// local primary key; a new slug is inserted. Staging is reset to the post's
/// published/draft state so a stale `sync_failed` doesn't linger. Series linkage
/// is dropped — series aren't synced and remote ids don't map to local rows.
async fn upsert_post_from_remote(db: &DatabaseConnection, remote: post::Model) -> AppResult<()> {
    let existing = post_by_slug(db, &remote.slug).await?;

    let mut model = remote;
    model.series_id = None;
    model.series_order = None;

    let saved = match existing {
        Some(local) => {
            model.id = local.id;
            model.into_update().update(db).await?
        }
        None => model.into_insert().insert(db).await?,
    };

    let stage = if saved.published { post_stage::PUBLISHED } else { post_stage::DRAFT };
    stage_set(
        db,
        post_stage::Model { post_id: saved.id, stage: stage.to_string(), staged_at: saved.updated_at },
    )
    .await?;
    Ok(())
}

// ─── Publish staging (local only) ───────────────────────────────────────────────

pub async fn stage_get(
    db: &impl ConnectionTrait,
    post_id: i32,
) -> AppResult<Option<post_stage::Model>> {
    Ok(post_stage::Entity::find_by_id(post_id).one(db).await?)
}

/// Remove a post's staging row. Clearing an absent row is not an error.
pub async fn stage_clear(db: &impl ConnectionTrait, post_id: i32) -> AppResult<()> {
    post_stage::Entity::delete_by_id(post_id).exec(db).await?;
    Ok(())
}

/// Upsert a post's staging row (there is one row per post).
pub async fn stage_set(
    db: &impl ConnectionTrait,
    model: post_stage::Model,
) -> AppResult<post_stage::Model> {
    let exists = post_stage::Entity::find_by_id(model.post_id)
        .one(db)
        .await?
        .is_some();

    if exists {
        let active = post_stage::ActiveModel {
            post_id: sea_orm::ActiveValue::Unchanged(model.post_id),
            stage: Set(model.stage),
            staged_at: Set(model.staged_at),
        };
        Ok(active.update(db).await?)
    } else {
        let active = post_stage::ActiveModel {
            post_id: Set(model.post_id),
            stage: Set(model.stage),
            staged_at: Set(model.staged_at),
        };
        Ok(active.insert(db).await?)
    }
}

/// Posts whose staging row is in `stage` (`"draft"` | `"published"`).
pub async fn posts_in_stage(
    db: &DatabaseConnection,
    stage: String,
) -> AppResult<Vec<post::Model>> {
    let ids: Vec<i32> = post_stage::Entity::find()
        .filter(post_stage::Column::Stage.eq(stage))
        .all(db)
        .await?
        .into_iter()
        .map(|s| s.post_id)
        .collect();

    if ids.is_empty() {
        return Ok(Vec::new());
    }

    Ok(post::Entity::find()
        .filter(post::Column::Id.is_in(ids))
        .order_by_desc(post::Column::CreatedAt)
        .all(db)
        .await?)
}

// ─── Sample data ────────────────────────────────────────────────────────────────

/// Populate an empty local database with a handful of sample posts (each also
/// staged draft/published). No-ops once the table has any rows.
pub async fn seed_sample_posts(db: &DatabaseConnection) -> AppResult<()> {
    if post::Entity::find().one(db).await?.is_some() {
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
        .await?;

        let stage = if published { post_stage::PUBLISHED } else { post_stage::DRAFT };
        stage_set(
            db,
            post_stage::Model { post_id: created.id, stage: stage.to_string(), staged_at: ts },
        )
        .await?;
    }

    Ok(())
}
