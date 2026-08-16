//! Local SQLite cache, accessed through the full Sea ORM entity API.
//!
//! The database file lives in the app data dir and holds the offline editing
//! state that later syncs to Cloudflare D1 (see `cloudflare::d1_*`). The tables
//! mirror the Drizzle schema (`series` and `blog-db`).

use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ColumnTrait, ConnectionTrait, Database,
    DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, QuerySelect, Schema,
    Set,
};
use tauri::Manager;

use crate::entities::record::{Id, Record};
use crate::entities::{post, post_revision, post_stage, post_sync, post_trash, series};
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

    // Also local-only: how each post's content compares with the cloud's.
    // Existing libraries simply have no rows here, which reads as "nothing has
    // been touched since it arrived" — see `sync_state::derive`.
    let mut sync_tbl = schema.create_table_from_entity(post_sync::Entity);
    sync_tbl.if_not_exists();
    db.execute(&sync_tbl)
        .await
        .map_err(|e| AppError::db_init("Failed to create `post_sync` table", e))?;

    // Local-only as well: which posts are in the trash.
    let mut trash_tbl = schema.create_table_from_entity(post_trash::Entity);
    trash_tbl.if_not_exists();
    db.execute(&trash_tbl)
        .await
        .map_err(|e| AppError::db_init("Failed to create `post_trash` table", e))?;

    // Local-only too: what each post looked like before each edit.
    let mut revision_tbl = schema.create_table_from_entity(post_revision::Entity);
    revision_tbl.if_not_exists();
    db.execute(&revision_tbl)
        .await
        .map_err(|e| AppError::db_init("Failed to create `post_revision` table", e))?;

    // Every read of the table is "this post's history, newest first", and a
    // library with a few hundred revisions would otherwise scan all of them to
    // answer it.
    db.execute_raw(sea_orm::Statement::from_string(
        db.get_database_backend(),
        "CREATE INDEX IF NOT EXISTS `idx_post_revision_post` \
         ON `post_revision` (`post_id`, `created_at` DESC, `id` DESC)"
            .to_string(),
    ))
    .await
    .map_err(|e| AppError::db_init("Failed to index `post_revision`", e))?;

    // `post_sync` grew two columns after it first shipped. See `ensure_columns`
    // for why creating the table is not enough.
    ensure_columns(
        db,
        "post_sync",
        &[("remote_updated_at", "integer"), ("remote_seen_at", "integer")],
    )
    .await?;

    Ok(())
}

/// Add any columns an existing table is missing.
///
/// `create_table_from_entity` is `IF NOT EXISTS`, so a database that already has
/// the table keeps whatever shape it had. That is silent and harmless until an
/// entity grows a field — and then every query naming the new column fails, on
/// exactly the machines that have been running the app longest, while a fresh
/// install works perfectly. It is the kind of break that never shows up in
/// development, because development databases get deleted.
///
/// Kept deliberately small: this is a column-adding backstop, not a migration
/// framework. Anything beyond adding nullable columns — renames, type changes,
/// backfills — needs a real migration with a version number, and should not be
/// smuggled in here.
async fn ensure_columns(
    db: &DatabaseConnection,
    table: &str,
    columns: &[(&str, &str)],
) -> AppResult<()> {
    let backend = db.get_database_backend();
    let existing: std::collections::HashSet<String> = db
        .query_all_raw(sea_orm::Statement::from_string(
            backend,
            format!("PRAGMA table_info(`{table}`)"),
        ))
        .await
        .map_err(|e| AppError::db_init("Failed to inspect an existing table", e))?
        .into_iter()
        .filter_map(|row| row.try_get::<String>("", "name").ok())
        .collect();

    for (name, ty) in columns {
        if existing.contains(*name) {
            continue;
        }
        log::info!("Adding missing column `{name}` to `{table}`");
        db.execute_raw(sea_orm::Statement::from_string(
            backend,
            format!("ALTER TABLE `{table}` ADD COLUMN `{name}` {ty}"),
        ))
        .await
        .map_err(|e| AppError::db_init("Failed to add a missing column", e))?;
    }
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

// ─── Series identity across the two databases ─────────────────────────────────

/// Translates series references between the local database and D1.
///
/// The two number their rows independently — a local `series.id` and a remote
/// one are unrelated integers that happen to share a type — so a post's
/// `series_id` means nothing until it is read in the right database. `slug` is
/// the identity both sides agree on (it is unique in each), so every crossing
/// goes id → slug → id.
///
/// Built once per sync rather than queried per post: a refresh walks every post,
/// and there are only ever a handful of series.
#[derive(Default)]
pub struct SeriesMap {
    /// Remote id → the local id wearing the same slug.
    inbound: std::collections::HashMap<i32, i32>,
    /// Local id → the remote id wearing the same slug.
    outbound: std::collections::HashMap<i32, i32>,
}

impl SeriesMap {
    /// Pair up the local series table with the cloud's by slug.
    pub async fn build(
        db: &impl ConnectionTrait,
        remote: &[series::Model],
    ) -> AppResult<Self> {
        let locals = series::Entity::find().all(db).await?;
        let by_slug: std::collections::HashMap<&str, i32> =
            locals.iter().map(|s| (s.slug.as_str(), s.id)).collect();

        let mut map = Self::default();
        for remote in remote {
            if let Some(&local_id) = by_slug.get(remote.slug.as_str()) {
                map.inbound.insert(remote.id, local_id);
                map.outbound.insert(local_id, remote.id);
            }
        }
        Ok(map)
    }

    /// The local series id for a remote one, or `None` when no local series
    /// carries that slug.
    pub fn to_local(&self, remote_id: i32) -> Option<i32> {
        self.inbound.get(&remote_id).copied()
    }

    /// The remote series id for a local one, or `None` when the cloud has no
    /// series with that slug — series themselves are not synced, so a local-only
    /// series simply has no counterpart yet.
    pub fn to_remote(&self, local_id: i32) -> Option<i32> {
        self.outbound.get(&local_id).copied()
    }

    /// Rewrite a post's series reference from local ids into the cloud's, in
    /// place, ready to send.
    ///
    /// A series that exists only on this machine has no remote counterpart to
    /// point at, so the post goes up unfiled rather than pointing at a number
    /// that means something else over there. Nothing is lost by that: the pull
    /// side keeps the local grouping instead of reading the gap as a removal.
    pub fn apply_outbound(&self, post: &mut post::Model) {
        let Some(local_id) = post.series_id else {
            return;
        };
        post.series_id = self.to_remote(local_id);
        if post.series_id.is_none() {
            post.series_order = None;
            log::info!(
                "Post `{}` is in a series the cloud does not have; pushing it unfiled",
                post.slug
            );
        }
    }
}

/// Mirror the local posts table onto the cloud's set of posts, keyed by `slug`.
///
/// The cloud is authoritative: every remote post is upserted into the local
/// cache (overwriting the local copy), and local posts whose slug isn't in the
/// remote set are deleted — so `local == remote` afterwards. Unsynced local-only
/// drafts are therefore removed. Returns `(upserted, deleted)`.
///
/// `remote_series` is the cloud's series table, needed to translate each post's
/// `series_id` into the local row it means.
///
/// **A trashed post takes no part in any of this.** It is neither overwritten by
/// the cloud's copy nor deleted for being absent from it: the first would edit
/// something the person has thrown away, and the second would empty their trash
/// on their behalf — destroying the only recoverable copy — as a side effect of
/// pressing Refresh. It rejoins the library, and the sync, when it is restored.
pub async fn mirror_posts(
    db: &DatabaseConnection,
    remote: Vec<post::Model>,
    remote_series: &[series::Model],
) -> AppResult<(usize, usize)> {
    let remote_slugs: std::collections::HashSet<String> =
        remote.iter().map(|p| p.slug.clone()).collect();
    let upserted = remote.len();
    let series = SeriesMap::build(db, remote_series).await?;
    let trashed = trashed_ids(db).await?;

    for post in remote {
        upsert_post_from_remote(db, post, &series, &trashed).await?;
    }

    // Drop anything local that no longer exists remotely (+ its staging and
    // sync rows, which describe a post that is about to stop existing).
    //
    // Except a post the cloud has never accepted. "Absent from D1" means two
    // entirely different things — *deleted there*, or *never sent* — and the
    // sync row tells them apart: a post with a fingerprint and no synced
    // counterpart has never been pushed, so its absence upstream is not news
    // about a deletion, it is the ordinary state of local work. Deleting on
    // that basis destroyed drafts for the crime of not having been published
    // yet, on a path reached by pressing Refresh.
    let locals = post::Entity::find().all(db).await?;
    let mut deleted = 0usize;
    for local in locals {
        if remote_slugs.contains(&local.slug) {
            continue;
        }
        // Re-read rather than taken from the set captured before the refresh
        // began. Mirroring a library is a long walk through the database, and a
        // post thrown away partway through it would otherwise be deleted
        // outright here — turning a recoverable trash action into permanent
        // loss, and leaving its trash row pointing at nothing.
        if trashed.contains(&local.id) || trash_get(db, local.id).await?.is_some() {
            // Already thrown away here, and its absence upstream says nothing
            // about whether the person still wants it back.
            continue;
        }
        let never_pushed = sync_get(db, local.id)
            .await?
            .is_some_and(|sync| sync.synced_hash.is_none());
        if never_pushed {
            log::info!(
                "Post `{}` is absent from the cloud because it has never been pushed; keeping it",
                local.slug
            );
            continue;
        }

        let _ = post_stage::Entity::delete_by_id(local.id).exec(db).await;
        let _ = sync_clear(db, local.id).await;
        let _ = revisions_clear(db, local.id).await;
        post::Entity::delete_by_id(local.id).exec(db).await?;
        deleted += 1;
    }

    Ok((upserted, deleted))
}

/// What series a refreshed post should belong to locally, as `(id, order)`.
///
/// Every other column on a pull is "cloud wins", and series membership
/// deliberately is not. The cloud is not authoritative about it: series rows
/// themselves are never synced, so a post filed under a local-only series has
/// nothing on the remote row to say so. Taking the cloud's answer there — as
/// clearing the columns outright did — throws away editorial grouping that
/// nothing else records.
///
/// So:
///
/// * **The remote names a series we know** → use the local row wearing that
///   slug. The remote integer itself is never stored; it means nothing here.
/// * **The remote names a series we do not know** → keep whatever the post is
///   already filed under, and say so in the log. The series may be one this
///   machine has not pulled, and guessing wrong loses the local grouping.
/// * **The remote names no series** → keep the local membership. "No series"
///   over there is indistinguishable from "this was never pushed", and only one
///   of those two readings is recoverable if it turns out to be wrong.
///
/// The cost is that removing a post from a series in the cloud does not
/// propagate down. That is the right way round while series do not sync at all:
/// a stale grouping is visible and fixable in the app, whereas a deleted one is
/// gone. It is worth revisiting once series themselves sync, since only then is
/// there a remote signal that genuinely means "removed".
pub fn resolve_series(
    remote: &post::Model,
    existing: Option<&post::Model>,
    series: &SeriesMap,
) -> (Option<i32>, Option<i32>) {
    let local = existing.and_then(|p| p.series_id.map(|id| (id, p.series_order)));

    match remote.series_id {
        Some(remote_id) => match series.to_local(remote_id) {
            Some(local_id) => (Some(local_id), remote.series_order),
            None => {
                log::warn!(
                    "Post `{}` references remote series {remote_id}, which has no local \
                     counterpart; keeping its existing series membership",
                    remote.slug
                );
                unpack(local)
            }
        },
        None => unpack(local),
    }
}

fn unpack(local: Option<(i32, Option<i32>)>) -> (Option<i32>, Option<i32>) {
    match local {
        Some((id, order)) => (Some(id), order),
        None => (None, None),
    }
}

/// Upsert one remote post into the local cache, keyed by `slug`.
///
/// The cloud wins *unless this machine has unpushed changes of its own*. A post
/// edited here and not there is left alone — overwriting it would silently
/// delete the edit — and a post edited on **both** sides is left alone and
/// marked, because there is no answer that does not throw away someone's work.
/// Only the person can pick; [`crate::commands::resolve_conflict`] is where they
/// do.
///
/// An existing local row (matched by slug) is overwritten in place, keeping its
/// local primary key; a new slug is inserted. Staging is reset to the post's
/// published/draft state so a stale `sync_failed` doesn't linger.
///
/// Series membership is the one field the cloud does not win outright even when
/// it does otherwise — see [`resolve_series`].
async fn upsert_post_from_remote(
    db: &DatabaseConnection,
    remote: post::Model,
    series: &SeriesMap,
    trashed: &std::collections::HashSet<i32>,
) -> AppResult<()> {
    let existing = post_by_slug(db, &remote.slug).await?;

    // A post in the trash is not part of the library, so the cloud has nothing
    // to say about it. Applying the refresh anyway would rewrite the copy the
    // person is holding on to in case they want it back — and restoring it
    // would then hand back the cloud's version rather than the one they threw
    // away.
    //
    // The captured set is consulted first because it answers for free; the row
    // itself is read when it does not, since a refresh walks the whole library
    // and a post can be trashed while it does.
    let trashed_now = match existing.as_ref() {
        Some(local) => trashed.contains(&local.id) || trash_get(db, local.id).await?.is_some(),
        None => false,
    };
    if trashed_now {
        log::info!("Post `{}` is in the trash; leaving it out of the refresh", remote.slug);
        return Ok(());
    }

    // A post with unpushed local edits is not overwritten — decided here, before
    // anything is written, because once the row is overwritten the evidence of
    // what was local is gone.
    //
    // Applying the cloud's metadata over it and keeping the fingerprint is not a
    // middle course, it is the worst of both: `mirror_posts` never replaces the
    // cached `<slug>.md`, so the post would end up carrying the cloud's title
    // over the local body, described by a fingerprint matching neither.
    //
    // What *is* taken from the refresh is the cloud's version stamp. It costs
    // nothing, changes no content, and is the whole basis for telling the two
    // cases apart afterwards: if the remote has moved too this is a conflict,
    // and if it has not, the local edit is simply ahead and waiting to be
    // published.
    if let Some(local) = existing.as_ref() {
        if sync_get(db, local.id)
            .await?
            .is_some_and(|sync| crate::sync_state::local_changed(&sync))
        {
            sync_observe_remote(db, local.id, remote.updated_at).await?;
            log::info!(
                "Post `{}` has unpushed local edits; leaving the refresh to the person",
                remote.slug
            );
            return Ok(());
        }
    }

    let mut model = remote;
    // Captured before the model is consumed: this is the version of the cloud's
    // copy that the agreement below is about.
    let remote_updated_at = model.updated_at;
    let (series_id, series_order) = resolve_series(&model, existing.as_ref(), series);
    model.series_id = series_id;
    model.series_order = series_order;

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

    // Only posts with nothing pending reach this point, so both sides now hold
    // the same thing — and *which* version of the cloud's copy that is, is the
    // one fact worth keeping.
    //
    // Discarding it instead would quietly break the case this exists for: pull a
    // clean post, edit it here, let another machine publish, refresh. With no
    // baseline, that refresh's observation becomes the baseline, the other
    // machine's change reads as nothing at all, and the next publish overwrites
    // it without ever offering a choice.
    //
    // The fingerprint covers the metadata alone — a refresh never sees the
    // remote body — which is sound because both hashes are set to the same value
    // here, so the post reads clean until something local genuinely changes it.
    sync_agree(
        db,
        saved.id,
        crate::sync_state::content_hash(&saved, ""),
        Some(remote_updated_at),
        chrono::Utc::now().timestamp(),
    )
    .await?;
    Ok(())
}

// ─── Publish staging (local only) ───────────────────────────────────────────────

/// Every post's staging row, for building a whole-library view in one query.
pub async fn stages_all(db: &impl ConnectionTrait) -> AppResult<Vec<post_stage::Model>> {
    Ok(post_stage::Entity::find().all(db).await?)
}

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

// ─── Sync state (local only) ────────────────────────────────────────────────────

pub async fn sync_get(
    db: &impl ConnectionTrait,
    post_id: i32,
) -> AppResult<Option<post_sync::Model>> {
    Ok(post_sync::Entity::find_by_id(post_id).one(db).await?)
}

/// Every post's sync row, for building a whole-library view in one query.
pub async fn sync_all(db: &impl ConnectionTrait) -> AppResult<Vec<post_sync::Model>> {
    Ok(post_sync::Entity::find().all(db).await?)
}

/// Record what the post's content hashes to right now, leaving the synced
/// fingerprint alone — the cloud has not been told anything by a local edit.
pub async fn sync_set_local(
    db: &impl ConnectionTrait,
    post_id: i32,
    local_hash: String,
) -> AppResult<post_sync::Model> {
    let existing = post_sync::Entity::find_by_id(post_id).one(db).await?;
    Ok(match existing {
        Some(row) => post_sync::ActiveModel {
            post_id: sea_orm::ActiveValue::Unchanged(post_id),
            local_hash: Set(local_hash),
            synced_hash: Set(row.synced_hash),
            synced_at: Set(row.synced_at),
            // A local edit says nothing about the cloud; what was observed there
            // stays observed.
            remote_updated_at: Set(row.remote_updated_at),
            remote_seen_at: Set(row.remote_seen_at),
        }
        .update(db)
        .await?,
        None => post_sync::ActiveModel {
            post_id: Set(post_id),
            local_hash: Set(local_hash),
            synced_hash: Set(None),
            synced_at: Set(None),
            remote_updated_at: Set(None),
            remote_seen_at: Set(None),
        }
        .insert(db)
        .await?,
    })
}

/// Record what the last refresh saw of the cloud's copy, without touching the
/// local side. This is the observation `sync_state::remote_changed` compares
/// against the baseline.
///
/// **The first observation becomes the baseline.** A post edited here has a sync
/// row created by that edit, which knows nothing about the cloud — so without
/// this, the first refresh would find an observation and no baseline to compare
/// it against, and reporting "different from nothing" as a change would call
/// every locally-edited post a conflict the moment anyone pressed Refresh.
///
/// The cost is that a remote change made *before* this machine first looked goes
/// unnoticed. There is no way around that: a version stamp cannot be compared
/// with one that was never recorded, and inventing a conflict is worse than
/// admitting the window exists. Every later change is caught, because from here
/// there is something to compare against.
pub async fn sync_observe_remote(
    db: &impl ConnectionTrait,
    post_id: i32,
    remote_updated_at: i64,
) -> AppResult<Option<post_sync::Model>> {
    let Some(row) = post_sync::Entity::find_by_id(post_id).one(db).await? else {
        // Nothing local has been touched since this post arrived, so there is no
        // baseline to be ahead of and nothing to record.
        return Ok(None);
    };
    let baseline = row.remote_updated_at.or(Some(remote_updated_at));
    Ok(Some(
        post_sync::ActiveModel {
            post_id: sea_orm::ActiveValue::Unchanged(post_id),
            local_hash: Set(row.local_hash),
            synced_hash: Set(row.synced_hash),
            synced_at: Set(row.synced_at),
            remote_updated_at: Set(baseline),
            remote_seen_at: Set(Some(remote_updated_at)),
        }
        .update(db)
        .await?,
    ))
}

/// Accept the cloud's currently-observed version as the baseline, leaving the
/// local content untouched.
///
/// This is "keep local": the remote change has been seen and accounted for, so
/// the post stops being a conflict, while its own edits stay pending. Nothing
/// about the local fingerprint moves, because nothing local did.
pub async fn sync_accept_remote_baseline(
    db: &impl ConnectionTrait,
    post_id: i32,
    baseline: Option<i64>,
) -> AppResult<Option<post_sync::Model>> {
    let Some(row) = post_sync::Entity::find_by_id(post_id).one(db).await? else {
        return Ok(None);
    };
    Ok(Some(
        post_sync::ActiveModel {
            post_id: sea_orm::ActiveValue::Unchanged(post_id),
            local_hash: Set(row.local_hash),
            synced_hash: Set(row.synced_hash),
            synced_at: Set(row.synced_at),
            remote_updated_at: Set(baseline),
            remote_seen_at: Set(baseline),
        }
        .update(db)
        .await?,
    ))
}

/// Declare the two sides in agreement: this content, and this version of the
/// cloud's copy, are the baseline from here.
///
/// Used when a pull is taken wholesale and when a conflict is resolved. The
/// fingerprint is over the metadata alone — a refresh never sees the remote body
/// — which is sound because both hashes are set to the same value here, so the
/// post reads clean until something local genuinely changes it.
pub async fn sync_agree(
    db: &impl ConnectionTrait,
    post_id: i32,
    hash: String,
    remote_updated_at: Option<i64>,
    at: i64,
) -> AppResult<post_sync::Model> {
    let exists = post_sync::Entity::find_by_id(post_id).one(db).await?.is_some();
    let model = post_sync::ActiveModel {
        post_id: if exists {
            sea_orm::ActiveValue::Unchanged(post_id)
        } else {
            Set(post_id)
        },
        local_hash: Set(hash.clone()),
        synced_hash: Set(Some(hash)),
        synced_at: Set(Some(at)),
        remote_updated_at: Set(remote_updated_at),
        remote_seen_at: Set(remote_updated_at),
    };
    Ok(if exists { model.update(db).await? } else { model.insert(db).await? })
}

/// Record that the cloud has accepted exactly this content: the two
/// fingerprints agree from here until the next local edit.
pub async fn sync_mark_synced(
    db: &impl ConnectionTrait,
    post_id: i32,
    hash: String,
    remote_updated_at: Option<i64>,
    at: i64,
) -> AppResult<post_sync::Model> {
    let existing = post_sync::Entity::find_by_id(post_id).one(db).await?;
    // The baseline becomes the version *we just wrote*, not the one last seen
    // before writing it. A push sets the remote row's `updated_at` to this
    // post's, so recording anything older leaves the next refresh finding a
    // remote change — our own — and reporting a conflict against ourselves.
    let baseline = remote_updated_at.or_else(|| existing.as_ref().and_then(|row| row.remote_seen_at));
    let model = post_sync::ActiveModel {
        post_id: if existing.is_some() {
            sea_orm::ActiveValue::Unchanged(post_id)
        } else {
            Set(post_id)
        },
        local_hash: Set(hash.clone()),
        synced_hash: Set(Some(hash)),
        synced_at: Set(Some(at)),
        remote_updated_at: Set(baseline),
        remote_seen_at: Set(baseline),
    };
    Ok(if existing.is_some() { model.update(db).await? } else { model.insert(db).await? })
}

/// Forget a post's sync record. Clearing an absent row is not an error.
pub async fn sync_clear(db: &impl ConnectionTrait, post_id: i32) -> AppResult<()> {
    post_sync::Entity::delete_by_id(post_id).exec(db).await?;
    Ok(())
}

// ─── Trash (local only) ───────────────────────────────────────────────────────

/// Every trashed post's id, for the many places that have to leave them out.
///
/// One query rather than a join per caller: a post being in the trash is a fact
/// about a handful of rows, and every listing needs the whole set anyway.
pub async fn trashed_ids(
    db: &impl ConnectionTrait,
) -> AppResult<std::collections::HashSet<i32>> {
    Ok(post_trash::Entity::find()
        .all(db)
        .await?
        .into_iter()
        .map(|t| t.post_id)
        .collect())
}

pub async fn trash_get(
    db: &impl ConnectionTrait,
    post_id: i32,
) -> AppResult<Option<post_trash::Model>> {
    Ok(post_trash::Entity::find_by_id(post_id).one(db).await?)
}

/// Move a post to the trash. Trashing an already-trashed post keeps the original
/// time, so restoring and re-trashing does not quietly reorder the view.
pub async fn trash_set(
    db: &impl ConnectionTrait,
    post_id: i32,
    trashed_at: i64,
) -> AppResult<post_trash::Model> {
    if let Some(existing) = trash_get(db, post_id).await? {
        return Ok(existing);
    }
    Ok(post_trash::ActiveModel {
        post_id: Set(post_id),
        trashed_at: Set(trashed_at),
    }
    .insert(db)
    .await?)
}

/// Take a post back out of the trash. Clearing an absent row is not an error.
pub async fn trash_clear(db: &impl ConnectionTrait, post_id: i32) -> AppResult<()> {
    post_trash::Entity::delete_by_id(post_id).exec(db).await?;
    Ok(())
}

/// The library as everything except the trash view sees it: newest first, with
/// trashed posts left out.
pub async fn list_active_posts(db: &impl ConnectionTrait) -> AppResult<Vec<post::Model>> {
    let trashed = trashed_ids(db).await?;
    Ok(list::<post::Model>(db)
        .await?
        .into_iter()
        .filter(|p| !trashed.contains(&p.id))
        .collect())
}

/// The trash itself, most recently thrown away first, each post paired with when
/// it went.
pub async fn list_trashed_posts(
    db: &impl ConnectionTrait,
) -> AppResult<Vec<(post::Model, post_trash::Model)>> {
    let trash: std::collections::HashMap<i32, post_trash::Model> = post_trash::Entity::find()
        .all(db)
        .await?
        .into_iter()
        .map(|t| (t.post_id, t))
        .collect();

    let mut rows: Vec<(post::Model, post_trash::Model)> = list::<post::Model>(db)
        .await?
        .into_iter()
        .filter_map(|post| trash.get(&post.id).map(|t| (post, t.clone())))
        .collect();
    rows.sort_by_key(|(_, t)| std::cmp::Reverse(t.trashed_at));
    Ok(rows)
}

// ─── Revisions (local only) ───────────────────────────────────────────────────

/// How many snapshots a post keeps before the oldest are dropped.
///
/// Whole bodies are stored, so the table grows with every edit and nothing else
/// would ever shrink it — a post edited daily for a year would carry its entire
/// first draft's worth of prose forever. Fifty is far more than the "undo that
/// bad edit" this exists for needs, and still only a few hundred kilobytes of
/// prose per post.
pub const REVISIONS_PER_POST: usize = 50;

/// Record a snapshot and drop anything past the cap.
///
/// Pruning happens here rather than on a schedule so the bound holds
/// continuously: there is no window in which the table is over its limit, and no
/// second place that has to remember this table exists.
pub async fn revision_add(
    db: &impl ConnectionTrait,
    model: post_revision::Model,
) -> AppResult<post_revision::Model> {
    let post_id = model.post_id;
    let created = model.into_insert().insert(db).await?;
    prune_revisions(db, post_id).await?;
    Ok(created)
}

/// Delete everything past the newest [`REVISIONS_PER_POST`] for one post.
async fn prune_revisions(db: &impl ConnectionTrait, post_id: i32) -> AppResult<()> {
    // Ordered exactly as the history is read, so the rows dropped are the ones
    // the UI would have shown last. `id` breaks ties because several snapshots
    // can share a second — a save and the publish that follows it, say — and a
    // tie broken arbitrarily would prune whichever the query planner felt like.
    //
    // Ids only, and the offset is taken here rather than in SQL: SQLite rejects
    // an `OFFSET` with no `LIMIT` beside it, and the alternative — a sentinel
    // limit — reads as a magic number for the sake of a list that is fifty rows
    // long by construction.
    let ids: Vec<i32> = post_revision::Entity::find()
        .select_only()
        .column(post_revision::Column::Id)
        .filter(post_revision::Column::PostId.eq(post_id))
        .order_by_desc(post_revision::Column::CreatedAt)
        .order_by_desc(post_revision::Column::Id)
        .into_tuple()
        .all(db)
        .await?;
    let surplus: Vec<i32> = ids.into_iter().skip(REVISIONS_PER_POST).collect();

    if surplus.is_empty() {
        return Ok(());
    }
    post_revision::Entity::delete_many()
        .filter(post_revision::Column::Id.is_in(surplus))
        .exec(db)
        .await?;
    Ok(())
}

/// One post's snapshots, newest first.
pub async fn revisions_for_post(
    db: &impl ConnectionTrait,
    post_id: i32,
) -> AppResult<Vec<post_revision::Model>> {
    Ok(post_revision::Entity::find()
        .filter(post_revision::Column::PostId.eq(post_id))
        .order_by_desc(post_revision::Column::CreatedAt)
        .order_by_desc(post_revision::Column::Id)
        .all(db)
        .await?)
}

/// The newest snapshot of a post, if it has any.
pub async fn revision_head(
    db: &impl ConnectionTrait,
    post_id: i32,
) -> AppResult<Option<post_revision::Model>> {
    Ok(post_revision::Entity::find()
        .filter(post_revision::Column::PostId.eq(post_id))
        .order_by_desc(post_revision::Column::CreatedAt)
        .order_by_desc(post_revision::Column::Id)
        .one(db)
        .await?)
}

/// One snapshot by its own id.
pub async fn revision_get(
    db: &impl ConnectionTrait,
    id: i32,
) -> AppResult<Option<post_revision::Model>> {
    Ok(post_revision::Entity::find_by_id(id).one(db).await?)
}

/// Forget a post's history. Only for a post that is itself being deleted —
/// otherwise the rows would attach to whichever post is assigned that id next.
pub async fn revisions_clear(db: &impl ConnectionTrait, post_id: i32) -> AppResult<()> {
    post_revision::Entity::delete_many()
        .filter(post_revision::Column::PostId.eq(post_id))
        .exec(db)
        .await?;
    Ok(())
}

/// Posts whose staging row is in `stage` (`"draft"` | `"published"`), trash
/// excluded — a thrown-away draft is not one of "the drafts".
pub async fn posts_in_stage(
    db: &DatabaseConnection,
    stage: String,
) -> AppResult<Vec<post::Model>> {
    let trashed = trashed_ids(db).await?;
    let ids: Vec<i32> = post_stage::Entity::find()
        .filter(post_stage::Column::Stage.eq(stage))
        .all(db)
        .await?
        .into_iter()
        .map(|s| s.post_id)
        .filter(|id| !trashed.contains(id))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn series_row(id: i32, slug: &str) -> series::Model {
        series::Model {
            id,
            slug: slug.to_string(),
            title: slug.to_string(),
            description: None,
            created_at: 0,
        }
    }

    fn post_row(slug: &str, series_id: Option<i32>, series_order: Option<i32>) -> post::Model {
        post::Model {
            id: 0,
            slug: slug.to_string(),
            title: slug.to_string(),
            excerpt: None,
            tags: None,
            published: true,
            published_at: None,
            series_id,
            series_order,
            created_at: 0,
            updated_at: 0,
        }
    }

    /// Seed a local series and a post filed under it, and hand back both ids.
    async fn local_post_in_series(
        db: &DatabaseConnection,
        series_slug: &str,
        post_slug: &str,
    ) -> (i32, i32) {
        let series = create::<series::Model>(db, series_row(0, series_slug)).await.unwrap();
        let post = create::<post::Model>(db, post_row(post_slug, Some(series.id), Some(3)))
            .await
            .unwrap();
        (series.id, post.id)
    }

    /// The mapping must go through the slug, never the integer. Here the same
    /// series is row 1 locally and row 42 in the cloud — if the remote id were
    /// stored as-is, the post would point at nothing.
    #[tokio::test]
    async fn a_remote_series_resolves_by_slug_not_by_id() {
        let db = connect_in_memory().await.unwrap();
        let (local_series, _) = local_post_in_series(&db, "rust", "a-post").await;

        let remote_series = vec![series_row(42, "rust")];
        mirror_posts(&db, vec![post_row("a-post", Some(42), Some(7))], &remote_series)
            .await
            .unwrap();

        let post = post_by_slug(&db, "a-post").await.unwrap().unwrap();
        assert_eq!(post.series_id, Some(local_series));
        assert_ne!(post.series_id, Some(42), "the remote id was stored verbatim");
        assert_eq!(post.series_order, Some(7));
    }

    /// The data loss this issue is about: series are not synced, so a post filed
    /// under a local series has nothing on the remote row saying so, and a
    /// refresh used to clear it.
    #[tokio::test]
    async fn a_refresh_keeps_a_local_series_the_cloud_knows_nothing_about() {
        let db = connect_in_memory().await.unwrap();
        let (local_series, _) = local_post_in_series(&db, "local-only", "a-post").await;

        // The cloud has the post, but no series at all.
        mirror_posts(&db, vec![post_row("a-post", None, None)], &[]).await.unwrap();

        let post = post_by_slug(&db, "a-post").await.unwrap().unwrap();
        assert_eq!(post.series_id, Some(local_series), "the refresh dropped the series");
        assert_eq!(post.series_order, Some(3));
    }

    /// An unresolvable reference keeps what is already there rather than
    /// guessing — the series may simply not have reached this machine.
    #[tokio::test]
    async fn an_unknown_remote_series_leaves_the_local_grouping_alone() {
        let db = connect_in_memory().await.unwrap();
        let (local_series, _) = local_post_in_series(&db, "rust", "a-post").await;

        // Remote post points at a series that is in no series table we have.
        mirror_posts(&db, vec![post_row("a-post", Some(99), Some(1))], &[]).await.unwrap();

        let post = post_by_slug(&db, "a-post").await.unwrap().unwrap();
        assert_eq!(post.series_id, Some(local_series));
        assert_eq!(post.series_order, Some(3));
    }

    /// A post that has never been in a series stays out of one.
    #[tokio::test]
    async fn a_post_with_no_series_on_either_side_stays_unfiled() {
        let db = connect_in_memory().await.unwrap();
        create::<post::Model>(&db, post_row("plain", None, None)).await.unwrap();

        mirror_posts(&db, vec![post_row("plain", None, None)], &[]).await.unwrap();

        let post = post_by_slug(&db, "plain").await.unwrap().unwrap();
        assert_eq!(post.series_id, None);
        assert_eq!(post.series_order, None);
    }

    /// A post arriving for the first time takes the cloud's grouping, translated.
    #[tokio::test]
    async fn a_new_remote_post_lands_in_the_matching_local_series() {
        let db = connect_in_memory().await.unwrap();
        let series = create::<series::Model>(&db, series_row(0, "rust")).await.unwrap();

        mirror_posts(&db, vec![post_row("fresh", Some(5), Some(2))], &[series_row(5, "rust")])
            .await
            .unwrap();

        let post = post_by_slug(&db, "fresh").await.unwrap().unwrap();
        assert_eq!(post.series_id, Some(series.id));
        assert_eq!(post.series_order, Some(2));
    }

    /// A local edit records a fingerprint the cloud has never accepted, which
    /// is what `Modified` is derived from.
    #[tokio::test]
    async fn a_local_edit_leaves_the_synced_fingerprint_behind() {
        let db = connect_in_memory().await.unwrap();
        let post = create::<post::Model>(&db, post_row("a-post", None, None)).await.unwrap();

        sync_set_local(&db, post.id, "v1".into()).await.unwrap();
        let row = sync_get(&db, post.id).await.unwrap().unwrap();
        assert_eq!(row.local_hash, "v1");
        assert_eq!(row.synced_hash, None, "nothing has been pushed yet");

        // A successful push brings the two into line…
        sync_mark_synced(&db, post.id, "v1".into(), None, 1_700_000_000).await.unwrap();
        let row = sync_get(&db, post.id).await.unwrap().unwrap();
        assert_eq!(row.synced_hash.as_deref(), Some("v1"));
        assert_eq!(row.synced_at, Some(1_700_000_000));

        // …and the next local edit parts them again, without disturbing the
        // record of what the cloud actually holds.
        sync_set_local(&db, post.id, "v2".into()).await.unwrap();
        let row = sync_get(&db, post.id).await.unwrap().unwrap();
        assert_eq!(row.local_hash, "v2");
        assert_eq!(row.synced_hash.as_deref(), Some("v1"), "the cloud still holds v1");
    }

    /// A refresh rewrites SQLite and leaves the cached `<slug>.md` alone, and
    /// `read_post_markdown` prefers that file — so a post with unpushed body
    /// edits is still serving them to the editor afterwards. Discarding its
    /// fingerprint would make the editor open those edits and call them clean,
    /// hiding the very thing the badge exists to show.
    #[tokio::test]
    async fn a_refresh_keeps_the_fingerprint_of_a_post_with_unpushed_edits() {
        let db = connect_in_memory().await.unwrap();
        let post = create::<post::Model>(&db, post_row("a-post", None, None)).await.unwrap();
        sync_set_local(&db, post.id, "local-only-edit".into()).await.unwrap();

        mirror_posts(&db, vec![post_row("a-post", None, None)], &[]).await.unwrap();

        let row = sync_get(&db, post.id).await.unwrap();
        assert!(row.is_some(), "the refresh discarded a pending local edit");
        assert_eq!(
            crate::sync_state::derive(None, row.as_ref()),
            crate::sync_state::SyncState::Modified,
            "the post stopped reporting its unpublished edits"
        );
    }

    /// "Absent from D1" means two different things, and only one of them is a
    /// deletion. A draft that has never been pushed is simply local work, and
    /// deleting it for not having been published yet — on a path reached by
    /// pressing Refresh — destroys it for good.
    #[tokio::test]
    async fn an_unpushed_draft_survives_a_refresh_that_does_not_mention_it() {
        let db = connect_in_memory().await.unwrap();
        let draft = create::<post::Model>(&db, post_row("local-draft", None, None)).await.unwrap();
        sync_set_local(&db, draft.id, "never-pushed".into()).await.unwrap();

        // A refresh that knows nothing about it.
        let (_, deleted) = mirror_posts(&db, vec![post_row("other", None, None)], &[])
            .await
            .unwrap();

        assert_eq!(deleted, 0);
        assert!(
            post_by_slug(&db, "local-draft").await.unwrap().is_some(),
            "the refresh deleted an unpublished draft"
        );
    }

    /// The other half: a post the cloud *did* have and no longer does really was
    /// deleted there, and the local copy follows.
    #[tokio::test]
    async fn a_post_deleted_in_the_cloud_is_removed_locally() {
        let db = connect_in_memory().await.unwrap();
        let post = create::<post::Model>(&db, post_row("was-live", None, None)).await.unwrap();
        sync_mark_synced(&db, post.id, "v1".into(), None, 1_700_000_000).await.unwrap();

        let (_, deleted) = mirror_posts(&db, vec![], &[]).await.unwrap();

        assert_eq!(deleted, 1);
        assert!(post_by_slug(&db, "was-live").await.unwrap().is_none());
        assert!(sync_get(&db, post.id).await.unwrap().is_none());
    }

    /// Keeping the local copy settles the conflict without touching the edit:
    /// the cloud's version is accounted for, and the post drops to an ordinary
    /// pending change that still has to be published deliberately.
    #[tokio::test]
    async fn keeping_local_settles_a_conflict_without_publishing_it() {
        let db = connect_in_memory().await.unwrap();
        let post = create::<post::Model>(&db, post_row("a-post", None, None)).await.unwrap();
        sync_set_local(&db, post.id, "my-edit".into()).await.unwrap();

        let mut remote = post_row("a-post", None, None);
        remote.updated_at = 500;
        mirror_posts(&db, vec![remote.clone()], &[]).await.unwrap();
        remote.updated_at = 900;
        mirror_posts(&db, vec![remote], &[]).await.unwrap();

        let row = sync_get(&db, post.id).await.unwrap().unwrap();
        assert_eq!(
            crate::sync_state::derive(None, Some(&row)),
            crate::sync_state::SyncState::Conflict
        );

        sync_accept_remote_baseline(&db, post.id, row.remote_seen_at).await.unwrap();

        let row = sync_get(&db, post.id).await.unwrap().unwrap();
        assert_eq!(
            crate::sync_state::derive(None, Some(&row)),
            crate::sync_state::SyncState::Modified,
            "the conflict did not settle"
        );
        assert_eq!(row.local_hash, "my-edit", "the local edit was disturbed");
        assert_eq!(row.synced_hash, None, "the edit was marked as published");
    }

    /// A locally-edited post has a sync row that knows nothing about the cloud,
    /// so the first refresh has an observation and no baseline. Treating that as
    /// a change would call every such post a conflict the moment anyone pressed
    /// Refresh; the first look is the baseline instead.
    #[tokio::test]
    async fn the_first_look_at_the_cloud_is_a_baseline_not_a_change() {
        let db = connect_in_memory().await.unwrap();
        let post = create::<post::Model>(&db, post_row("a-post", None, None)).await.unwrap();
        sync_set_local(&db, post.id, "local-edit".into()).await.unwrap();

        let mut remote = post_row("a-post", None, None);
        remote.updated_at = 500;
        mirror_posts(&db, vec![remote], &[]).await.unwrap();

        let row = sync_get(&db, post.id).await.unwrap().unwrap();
        assert_eq!(row.remote_updated_at, Some(500), "the first look set no baseline");
        assert_eq!(
            crate::sync_state::derive(None, Some(&row)),
            crate::sync_state::SyncState::Modified,
            "a first refresh invented a conflict"
        );
    }

    /// Once there is a baseline, the cloud moving under a local edit is the
    /// genuine article — and neither copy may be applied over the other.
    #[tokio::test]
    async fn the_cloud_moving_under_a_local_edit_is_a_conflict() {
        let db = connect_in_memory().await.unwrap();
        let post = create::<post::Model>(&db, post_row("a-post", None, None)).await.unwrap();
        sync_set_local(&db, post.id, "local-edit".into()).await.unwrap();

        // First refresh establishes where the cloud stood…
        let mut remote = post_row("a-post", None, None);
        remote.updated_at = 500;
        mirror_posts(&db, vec![remote.clone()], &[]).await.unwrap();

        // …and a later one finds it somewhere else.
        remote.updated_at = 900;
        mirror_posts(&db, vec![remote], &[]).await.unwrap();

        let row = sync_get(&db, post.id).await.unwrap().unwrap();
        assert_eq!(
            crate::sync_state::derive(None, Some(&row)),
            crate::sync_state::SyncState::Conflict
        );
        // And still nothing was applied over the local copy.
        assert_eq!(row.local_hash, "local-edit");
    }

    /// And the post itself is left whole. Applying the cloud's metadata while
    /// keeping the fingerprint would leave the row describing neither copy —
    /// the cloud's title over the local body — and a metadata-only edit
    /// reporting changes it no longer has.
    #[tokio::test]
    async fn a_refresh_does_not_half_apply_itself_to_a_locally_edited_post() {
        let db = connect_in_memory().await.unwrap();
        let mut local = post_row("a-post", None, None);
        local.title = "Local title".into();
        let post = create::<post::Model>(&db, local).await.unwrap();
        sync_set_local(&db, post.id, "local-only-edit".into()).await.unwrap();

        let mut remote = post_row("a-post", None, None);
        remote.title = "Cloud title".into();
        mirror_posts(&db, vec![remote], &[]).await.unwrap();

        let after = post_by_slug(&db, "a-post").await.unwrap().unwrap();
        assert_eq!(after.title, "Local title", "the refresh overwrote a local edit");
    }

    /// Where there is nothing pending, the refresh takes the cloud's copy and
    /// records *which* version it took.
    ///
    /// Keeping that stamp is the whole basis for noticing a later remote change:
    /// throwing it away would leave the next local edit with no baseline, and
    /// the change after that — made on another machine — would read as nothing
    /// at all.
    #[tokio::test]
    async fn a_refresh_records_the_version_of_the_cloud_it_accepted() {
        let db = connect_in_memory().await.unwrap();
        let post = create::<post::Model>(&db, post_row("a-post", None, None)).await.unwrap();
        sync_mark_synced(&db, post.id, "v1".into(), None, 1_700_000_000).await.unwrap();

        let mut remote = post_row("a-post", None, None);
        remote.updated_at = 4_242;
        mirror_posts(&db, vec![remote], &[]).await.unwrap();

        let row = sync_get(&db, post.id).await.unwrap().unwrap();
        assert_eq!(row.remote_updated_at, Some(4_242));
        assert_eq!(row.remote_seen_at, Some(4_242));
        assert_eq!(
            crate::sync_state::derive(None, Some(&row)),
            crate::sync_state::SyncState::Clean,
            "a post taken wholesale from the cloud is not pending anything"
        );
    }

    /// The sequence the baseline exists for: pull a clean post, edit it here,
    /// let another machine publish, refresh. Without the baseline that first
    /// pull recorded, the other machine's change reads as nothing and the next
    /// publish overwrites it silently.
    #[tokio::test]
    async fn another_machines_change_after_a_local_edit_is_a_conflict() {
        let db = connect_in_memory().await.unwrap();
        let post = create::<post::Model>(&db, post_row("a-post", None, None)).await.unwrap();

        // A clean pull, which records where the cloud stood.
        let mut remote = post_row("a-post", None, None);
        remote.updated_at = 100;
        mirror_posts(&db, vec![remote.clone()], &[]).await.unwrap();

        // Edited here…
        sync_set_local(&db, post.id, "my-edit".into()).await.unwrap();
        // …while another machine publishes, and we refresh.
        remote.updated_at = 700;
        mirror_posts(&db, vec![remote], &[]).await.unwrap();

        let row = sync_get(&db, post.id).await.unwrap().unwrap();
        assert_eq!(
            crate::sync_state::derive(None, Some(&row)),
            crate::sync_state::SyncState::Conflict,
            "the other machine's change was about to be overwritten silently"
        );
    }

    /// A trashed post is deleted as far as every listing is concerned, while
    /// nothing about it is actually gone.
    #[tokio::test]
    async fn a_trashed_post_leaves_the_library_and_comes_back_whole() {
        let db = connect_in_memory().await.unwrap();
        let kept = create::<post::Model>(&db, post_row("kept", None, None)).await.unwrap();
        let mut binned = post_row("binned", None, None);
        binned.title = "Thrown away".into();
        let binned = create::<post::Model>(&db, binned).await.unwrap();
        revision_add(&db, revision_row(binned.id, "an old draft", 1)).await.unwrap();

        trash_set(&db, binned.id, 1_700_000_000).await.unwrap();

        let active: Vec<i32> = list_active_posts(&db).await.unwrap().iter().map(|p| p.id).collect();
        assert_eq!(active, vec![kept.id]);
        let trashed = list_trashed_posts(&db).await.unwrap();
        assert_eq!(trashed.len(), 1);
        assert_eq!(trashed[0].0.title, "Thrown away");
        assert_eq!(trashed[0].1.trashed_at, 1_700_000_000);
        // The history is not part of what a soft delete takes away.
        assert_eq!(revisions_for_post(&db, binned.id).await.unwrap().len(), 1);

        trash_clear(&db, binned.id).await.unwrap();

        assert_eq!(list_active_posts(&db).await.unwrap().len(), 2);
        assert!(list_trashed_posts(&db).await.unwrap().is_empty());
        assert_eq!(
            get::<post::Model>(&db, binned.id).await.unwrap().unwrap().title,
            "Thrown away",
            "the post came back changed"
        );
    }

    /// The cloud has nothing to say about a post that has been thrown away here.
    /// Applying the refresh would rewrite the copy being kept in case it is
    /// wanted back.
    #[tokio::test]
    async fn a_refresh_leaves_a_trashed_post_alone() {
        let db = connect_in_memory().await.unwrap();
        let mut local = post_row("a-post", None, None);
        local.title = "Local title".into();
        let post = create::<post::Model>(&db, local).await.unwrap();
        trash_set(&db, post.id, 1).await.unwrap();

        let mut remote = post_row("a-post", None, None);
        remote.title = "Cloud title".into();
        mirror_posts(&db, vec![remote], &[]).await.unwrap();

        assert_eq!(
            get::<post::Model>(&db, post.id).await.unwrap().unwrap().title,
            "Local title",
            "a refresh overwrote a post in the trash"
        );
    }

    /// And the other direction: a post deleted in the cloud must not empty the
    /// local trash, which is the only place its recoverable copy lives.
    #[tokio::test]
    async fn a_refresh_does_not_empty_the_trash() {
        let db = connect_in_memory().await.unwrap();
        let post = create::<post::Model>(&db, post_row("was-live", None, None)).await.unwrap();
        sync_mark_synced(&db, post.id, "v1".into(), None, 1_700_000_000).await.unwrap();
        trash_set(&db, post.id, 1).await.unwrap();

        let (_, deleted) = mirror_posts(&db, vec![], &[]).await.unwrap();

        assert_eq!(deleted, 0);
        assert!(
            get::<post::Model>(&db, post.id).await.unwrap().is_some(),
            "a refresh permanently deleted a post from the trash"
        );
    }

    /// Trashing something twice must not reorder the view under the person
    /// looking at it.
    #[tokio::test]
    async fn re_trashing_keeps_the_time_it_first_went() {
        let db = connect_in_memory().await.unwrap();
        let post = create::<post::Model>(&db, post_row("a-post", None, None)).await.unwrap();

        trash_set(&db, post.id, 100).await.unwrap();
        let again = trash_set(&db, post.id, 900).await.unwrap();

        assert_eq!(again.trashed_at, 100);
    }

    /// A trashed draft is not one of "the drafts".
    #[tokio::test]
    async fn stage_listings_skip_the_trash() {
        let db = connect_in_memory().await.unwrap();
        let kept = create::<post::Model>(&db, post_row("kept", None, None)).await.unwrap();
        let binned = create::<post::Model>(&db, post_row("binned", None, None)).await.unwrap();
        for id in [kept.id, binned.id] {
            stage_set(
                &db,
                post_stage::Model { post_id: id, stage: post_stage::DRAFT.into(), staged_at: 0 },
            )
            .await
            .unwrap();
        }

        trash_set(&db, binned.id, 1).await.unwrap();

        let drafts = posts_in_stage(&db, post_stage::DRAFT.to_string()).await.unwrap();
        assert_eq!(drafts.iter().map(|p| p.id).collect::<Vec<_>>(), vec![kept.id]);
    }

    fn revision_row(post_id: i32, body: &str, at: i64) -> post_revision::Model {
        post_revision::Model {
            id: 0,
            post_id,
            title: "A post".into(),
            excerpt: None,
            tags: None,
            published: false,
            body: Some(body.to_string()),
            origin: post_revision::SAVE.into(),
            created_at: at,
        }
    }

    /// History is read newest first, and snapshots sharing a second — a save and
    /// the publish right after it — must still come back in the order they were
    /// taken rather than in whatever order the rows were scanned.
    #[tokio::test]
    async fn revisions_come_back_newest_first() {
        let db = connect_in_memory().await.unwrap();
        let post = create::<post::Model>(&db, post_row("a-post", None, None)).await.unwrap();

        for (body, at) in [("oldest", 100), ("middle", 200), ("newest", 200)] {
            revision_add(&db, revision_row(post.id, body, at)).await.unwrap();
        }

        let bodies: Vec<String> = revisions_for_post(&db, post.id)
            .await
            .unwrap()
            .into_iter()
            .filter_map(|r| r.body)
            .collect();
        assert_eq!(bodies, ["newest", "middle", "oldest"]);
        assert_eq!(
            revision_head(&db, post.id).await.unwrap().and_then(|r| r.body).as_deref(),
            Some("newest")
        );
    }

    /// Whole bodies are stored, so without a bound the table only ever grows.
    /// The cap has to drop the *oldest* end: pruning the newest would leave a
    /// history that cannot reach the version somebody just left.
    #[tokio::test]
    async fn the_oldest_revisions_are_pruned_once_the_cap_is_reached() {
        let db = connect_in_memory().await.unwrap();
        let post = create::<post::Model>(&db, post_row("a-post", None, None)).await.unwrap();

        let total = REVISIONS_PER_POST as i64 + 5;
        for n in 0..total {
            revision_add(&db, revision_row(post.id, &format!("body {n}"), n)).await.unwrap();
        }

        let kept = revisions_for_post(&db, post.id).await.unwrap();
        assert_eq!(kept.len(), REVISIONS_PER_POST);
        assert_eq!(kept.first().unwrap().body.as_deref(), Some(format!("body {}", total - 1).as_str()));
        assert_eq!(
            kept.last().unwrap().body.as_deref(),
            Some(format!("body {}", total - REVISIONS_PER_POST as i64).as_str()),
            "pruning took from the wrong end"
        );
    }

    /// One post's history is not another's — the pruning and the clearing both
    /// have to stay inside the post they were asked about.
    #[tokio::test]
    async fn clearing_a_history_leaves_every_other_post_alone() {
        let db = connect_in_memory().await.unwrap();
        let mine = create::<post::Model>(&db, post_row("mine", None, None)).await.unwrap();
        let yours = create::<post::Model>(&db, post_row("yours", None, None)).await.unwrap();

        revision_add(&db, revision_row(mine.id, "mine", 1)).await.unwrap();
        revision_add(&db, revision_row(yours.id, "yours", 1)).await.unwrap();

        revisions_clear(&db, mine.id).await.unwrap();

        assert!(revisions_for_post(&db, mine.id).await.unwrap().is_empty());
        assert_eq!(revisions_for_post(&db, yours.id).await.unwrap().len(), 1);
    }

    /// A post deleted in the cloud takes its local history with it. Leaving the
    /// rows behind would hand a stranger's drafts to whichever post is assigned
    /// that id next.
    #[tokio::test]
    async fn a_post_removed_by_a_refresh_takes_its_history_with_it() {
        let db = connect_in_memory().await.unwrap();
        let post = create::<post::Model>(&db, post_row("was-live", None, None)).await.unwrap();
        sync_mark_synced(&db, post.id, "v1".into(), None, 1_700_000_000).await.unwrap();
        revision_add(&db, revision_row(post.id, "an old draft", 1)).await.unwrap();

        mirror_posts(&db, vec![], &[]).await.unwrap();

        assert!(revisions_for_post(&db, post.id).await.unwrap().is_empty());
    }

    /// What goes up carries the cloud's id for the series, never this machine's
    /// — the mistake that files a post under an unrelated remote series.
    #[tokio::test]
    async fn a_post_pushed_upward_carries_the_remote_series_id() {
        let db = connect_in_memory().await.unwrap();
        let local = create::<series::Model>(&db, series_row(0, "rust")).await.unwrap();
        let map = SeriesMap::build(&db, &[series_row(42, "rust")]).await.unwrap();

        let mut post = post_row("a-post", Some(local.id), Some(3));
        map.apply_outbound(&mut post);

        assert_eq!(post.series_id, Some(42));
        assert_eq!(post.series_order, Some(3));
    }

    /// A series the cloud has never heard of cannot be pointed at, so the post
    /// goes up unfiled rather than carrying a number that means something else
    /// there. The pull rule is what keeps the local grouping through the round
    /// trip.
    #[tokio::test]
    async fn a_local_only_series_pushes_unfiled() {
        let db = connect_in_memory().await.unwrap();
        let local = create::<series::Model>(&db, series_row(0, "local-only")).await.unwrap();
        let map = SeriesMap::build(&db, &[]).await.unwrap();

        let mut post = post_row("a-post", Some(local.id), Some(3));
        map.apply_outbound(&mut post);

        assert_eq!(post.series_id, None);
        assert_eq!(post.series_order, None, "an order without a series is meaningless");
    }

    /// The push direction of the same translation: what goes up must be the
    /// cloud's id for the series, not this machine's.
    #[tokio::test]
    async fn the_map_translates_in_both_directions() {
        let db = connect_in_memory().await.unwrap();
        let local = create::<series::Model>(&db, series_row(0, "rust")).await.unwrap();

        let map = SeriesMap::build(&db, &[series_row(42, "rust"), series_row(7, "elsewhere")])
            .await
            .unwrap();

        assert_eq!(map.to_local(42), Some(local.id));
        assert_eq!(map.to_remote(local.id), Some(42));
        // A remote series with no local counterpart maps nowhere.
        assert_eq!(map.to_local(7), None);
        assert_eq!(map.to_remote(999), None);
    }
}
