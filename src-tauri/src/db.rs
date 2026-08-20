//! Local SQLite cache, accessed through the full Sea ORM entity API.
//!
//! The database file lives in the app data dir and holds the offline editing
//! state that later syncs to Cloudflare D1 (see `cloudflare::d1_*`). The tables
//! mirror the Drizzle schema (`series` and `blog-db`).

use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ColumnTrait, ConnectionTrait, Database,
    DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, QuerySelect, Schema,
    Set, TransactionTrait,
};
use tauri::Manager;

use crate::entities::record::{Id, Record};
use crate::entities::{
    post, post_body_stale, post_revision, post_schedule, post_stage, post_sync, post_tombstone,
    post_trash, series,
};
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

    // The local mirror of what is scheduled in the cloud. Unlike the tables
    // around it this one *does* have a D1 counterpart — the Worker reads it
    // there — and the copy here is what lets the app show a schedule offline.
    let schedule_tbl = {
        let mut tbl = schema.create_table_from_entity(post_schedule::Entity);
        tbl.if_not_exists();
        tbl
    };
    db.execute(&schedule_tbl)
        .await
        .map_err(|e| AppError::db_init("Failed to create `post_schedule` table", e))?;

    // Local-only as well: whose cached Markdown a refresh has outdated.
    let mut stale_tbl = schema.create_table_from_entity(post_body_stale::Entity);
    stale_tbl.if_not_exists();
    db.execute(&stale_tbl)
        .await
        .map_err(|e| AppError::db_init("Failed to create `post_body_stale` table", e))?;

    // Local-only as well: which slugs have been deleted here for good. See
    // `post_tombstone` for why "forever" needs writing down.
    let mut tombstone_tbl = schema.create_table_from_entity(post_tombstone::Entity);
    tombstone_tbl.if_not_exists();
    db.execute(&tombstone_tbl)
        .await
        .map_err(|e| AppError::db_init("Failed to create `post_tombstone` table", e))?;

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

/// The local series wearing this slug, if any.
///
/// Slug is the identity the two databases agree on — see [`SeriesMap`] — so it
/// is what a series is looked up by whenever a remote row has to be matched to
/// a local one.
pub async fn series_by_slug(
    db: &impl ConnectionTrait,
    slug: &str,
) -> AppResult<Option<series::Model>> {
    Ok(series::Entity::find()
        .filter(series::Column::Slug.eq(slug))
        .one(db)
        .await?)
}

/// Take every post out of a series, and report how many were moved.
///
/// Deleting a series without this leaves its posts pointing at a row that is
/// gone: the id still reads as a number, so nothing complains, and the post is
/// filed under a series nobody can name. Run in the same transaction as the
/// delete, so there is no moment where one has happened and the other has not.
pub async fn unfile_series(db: &impl ConnectionTrait, series_id: i32) -> AppResult<u64> {
    let done = post::Entity::update_many()
        .col_expr(post::Column::SeriesId, sea_orm::sea_query::Expr::value(None::<i32>))
        .col_expr(post::Column::SeriesOrder, sea_orm::sea_query::Expr::value(None::<i32>))
        .filter(post::Column::SeriesId.eq(series_id))
        .exec(db)
        .await?;
    Ok(done.rows_affected)
}

/// Bring a series the cloud has into the local table, matched **by slug**.
///
/// The local id is kept where a row already exists, because posts point at it:
/// taking the remote id would refile every post in the series under a number
/// that means something else here. That is the same rule [`SeriesMap`] follows,
/// applied to the rows themselves rather than to the references.
///
/// `created_at` is left as the local row has it. Both sides should agree on when
/// a series was made, and if they do not, the difference is not worth a write.
pub async fn upsert_series_from_remote(
    db: &impl ConnectionTrait,
    remote: series::Model,
) -> AppResult<()> {
    match series_by_slug(db, &remote.slug).await? {
        Some(local) => {
            let model = series::Model {
                id: local.id,
                created_at: local.created_at,
                ..remote
            };
            update::<series::Model>(db, model).await?;
        }
        None => {
            create::<series::Model>(db, remote).await?;
        }
    }
    Ok(())
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

/// What a mirror did.
pub struct Mirrored {
    pub upserted: usize,
    pub deleted: usize,
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
) -> AppResult<Mirrored> {
    let remote_slugs: std::collections::HashSet<String> =
        remote.iter().map(|p| p.slug.clone()).collect();
    let upserted = remote.len();
    let series = SeriesMap::build(db, remote_series).await?;

    // Any tombstone whose post the cloud no longer has is finished: there is
    // nothing left for it to keep out, and leaving it would silently refuse a
    // slug somebody may want to use again.
    for slug in tombstoned_slugs(db).await? {
        if !remote_slugs.contains(&slug) {
            let _ = tombstone_clear(db, &slug).await;
        }
    }

    // ── One transaction per post ──────────────────────────────────────────────
    //
    // A refresh is a long walk: a library's worth of reads and writes, over
    // which somebody can perfectly well throw a post away or delete one for
    // good. Every decision below therefore reads its precondition *inside* the
    // transaction that acts on it. Checking first and writing afterwards — even
    // immediately afterwards — leaves a window in which the answer changes
    // between the two, and the losses on the other side of that window are the
    // permanent kind: a trashed post overwritten or deleted outright, a
    // "Delete forever" undone by the pull that follows it.
    for post in remote {
        let txn = db.begin().await?;
        upsert_post_from_remote(&txn, post, &series).await?;
        txn.commit().await?;
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

        // Same rule as the upsert loop: the conditions are read inside the
        // transaction that deletes, so a post thrown away while this walk was
        // running cannot be deleted out from under the trash — which would turn
        // a recoverable action into permanent loss and leave a trash row
        // pointing at nothing.
        let txn = db.begin().await?;

        if trash_get(&txn, local.id).await?.is_some() {
            // Already thrown away here, and its absence upstream says nothing
            // about whether the person still wants it back.
            txn.rollback().await?;
            continue;
        }
        // Work the cloud has never been given, which covers both a post never
        // pushed at all and one pushed once and edited since — `local_changed`
        // answers for both, since the first has no synced hash to match.
        // Deleting either takes the row, its stage, its sync record and every
        // revision of it, with no trash row and no undo.
        //
        // The same question the upsert branch asks before declining to overwrite
        // a post; the two must agree.
        let has_unpushed_work = sync_get(&txn, local.id)
            .await?
            .is_some_and(|sync| crate::sync_state::local_changed(&sync));
        if has_unpushed_work {
            log::info!(
                "Post `{}` is absent from the cloud but carries work that was never pushed; keeping it",
                local.slug
            );
            txn.rollback().await?;
            continue;
        }

        post_stage::Entity::delete_by_id(local.id).exec(&txn).await?;
        sync_clear(&txn, local.id).await?;
        revisions_clear(&txn, local.id).await?;
        post::Entity::delete_by_id(local.id).exec(&txn).await?;
        txn.commit().await?;
        deleted += 1;
    }

    Ok(Mirrored { upserted, deleted })
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
    db: &(impl ConnectionTrait + TransactionTrait<Transaction = sea_orm::DatabaseTransaction>),
    remote: post::Model,
    series: &SeriesMap,
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
    match existing.as_ref() {
        Some(local) => {
            if trash_get(db, local.id).await?.is_some() {
                log::info!("Post `{}` is in the trash; leaving it out of the refresh", remote.slug);
                return Ok(());
            }
        }
        // No local row and a tombstone means this post was deleted here for
        // good. The cloud's copy is deliberately left alone by that deletion, so
        // this is the only thing standing between "Delete forever" and the very
        // next pull putting the post back.
        None => {
            if tombstoned_slugs(db).await?.contains(&remote.slug) {
                log::info!("Post `{}` was deleted here for good; not pulling it back", remote.slug);
                return Ok(());
            }
        }
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
    // And the version this machine held, to tell a refresh that changed
    // something from one that changed nothing.
    let previous_updated_at = existing.as_ref().map(|local| local.updated_at);
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

    // Whether the body cached for this post can still be trusted. The refresh
    // never fetches it, so a cloud copy that has moved since the version this
    // machine held means the cached Markdown is an older one. A post arriving
    // for the first time has no cached body to be stale.
    //
    // Recorded here, in the transaction that advances the metadata, rather than
    // by deleting the cached file: a deletion can fail, and by then the baseline
    // has moved and no later refresh would notice again. The row commits with
    // the metadata that made it true, and every reader of a cached body consults
    // it — see `post_body_stale`.
    if previous_updated_at.is_some_and(|had| had != remote_updated_at) {
        body_stale_set(db, &saved.slug, chrono::Utc::now().timestamp()).await?;
    }
    Ok(())
}

/// Refuse to write a side-table row for a post that no longer exists.
///
/// `post_stage`, `post_sync` and `post_revision` are keyed by the post's id and
/// carry no foreign key, so nothing at the database level stops a row outliving
/// the post it describes. That matters more than it sounds: the primary key is a
/// plain `INTEGER PRIMARY KEY` rather than `AUTOINCREMENT`, so SQLite is free to
/// hand a deleted post's id to the next one — which would inherit its stage, its
/// idea of what the cloud holds, and its draft history.
///
/// A save that loses a race with a permanent deletion is exactly how that
/// happens: its metadata commits, the post is deleted, and its remaining writes
/// land afterwards. Checked here rather than in each caller so no future path
/// has to remember, and inside whatever transaction the caller passes, which is
/// what makes it airtight where one is used.
async fn require_post(db: &impl ConnectionTrait, post_id: i32) -> AppResult<()> {
    match post::Entity::find_by_id(post_id).one(db).await? {
        Some(_) => Ok(()),
        None => Err(AppError::PostVanished(post_id)),
    }
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
    db: &(impl ConnectionTrait + TransactionTrait<Transaction = sea_orm::DatabaseTransaction>),
    model: post_stage::Model,
) -> AppResult<post_stage::Model> {
    let txn = db.begin().await?;
    require_post(&txn, model.post_id).await?;
    let db = &txn;
    let exists = post_stage::Entity::find_by_id(model.post_id)
        .one(db)
        .await?
        .is_some();

    let written = if exists {
        let active = post_stage::ActiveModel {
            post_id: sea_orm::ActiveValue::Unchanged(model.post_id),
            stage: Set(model.stage),
            staged_at: Set(model.staged_at),
        };
        active.update(db).await?
    } else {
        let active = post_stage::ActiveModel {
            post_id: Set(model.post_id),
            stage: Set(model.stage),
            staged_at: Set(model.staged_at),
        };
        active.insert(db).await?
    };
    txn.commit().await?;
    Ok(written)
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
    db: &(impl ConnectionTrait + TransactionTrait<Transaction = sea_orm::DatabaseTransaction>),
    post_id: i32,
    local_hash: String,
) -> AppResult<post_sync::Model> {
    let txn = db.begin().await?;
    require_post(&txn, post_id).await?;
    let db = &txn;
    let existing = post_sync::Entity::find_by_id(post_id).one(db).await?;
    let written = match existing {
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
    };
    txn.commit().await?;
    Ok(written)
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
    db: &(impl ConnectionTrait + TransactionTrait<Transaction = sea_orm::DatabaseTransaction>),
    post_id: i32,
    hash: String,
    remote_updated_at: Option<i64>,
    at: i64,
) -> AppResult<post_sync::Model> {
    let txn = db.begin().await?;
    require_post(&txn, post_id).await?;
    let db = &txn;
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
    let written = if exists { model.update(db).await? } else { model.insert(db).await? };
    txn.commit().await?;
    Ok(written)
}

/// Record that the cloud has accepted exactly this content: the two
/// fingerprints agree from here until the next local edit.
pub async fn sync_mark_synced(
    db: &(impl ConnectionTrait + TransactionTrait<Transaction = sea_orm::DatabaseTransaction>),
    post_id: i32,
    hash: String,
    remote_updated_at: Option<i64>,
    at: i64,
) -> AppResult<post_sync::Model> {
    let txn = db.begin().await?;
    require_post(&txn, post_id).await?;
    let db = &txn;
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
    let written = if existing.is_some() { model.update(db).await? } else { model.insert(db).await? };
    txn.commit().await?;
    Ok(written)
}

/// Forget a post's sync record. Clearing an absent row is not an error.
pub async fn sync_clear(db: &impl ConnectionTrait, post_id: i32) -> AppResult<()> {
    post_sync::Entity::delete_by_id(post_id).exec(db).await?;
    Ok(())
}

// ─── Schedules (mirrored from D1) ─────────────────────────────────────────────

/// Every schedule this machine knows about, soonest first.
pub async fn schedules_all(
    db: &impl ConnectionTrait,
) -> AppResult<Vec<post_schedule::Model>> {
    Ok(post_schedule::Entity::find()
        .order_by_asc(post_schedule::Column::PublishAt)
        .all(db)
        .await?)
}

pub async fn schedule_get(
    db: &impl ConnectionTrait,
    slug: &str,
) -> AppResult<Option<post_schedule::Model>> {
    Ok(post_schedule::Entity::find_by_id(slug.to_string()).one(db).await?)
}

/// Write a schedule row, replacing any existing one for that slug.
pub async fn schedule_set(
    db: &impl ConnectionTrait,
    model: post_schedule::Model,
) -> AppResult<post_schedule::Model> {
    let exists = schedule_get(db, &model.slug).await?.is_some();
    Ok(if exists {
        model.into_update().update(db).await?
    } else {
        model.into_insert().insert(db).await?
    })
}

/// Replace the local mirror with what the cloud holds.
///
/// Wholesale rather than row by row, because the cloud is authoritative here:
/// the Worker is the only thing that moves a schedule from `pending` to
/// `published` or `failed`, and a row that has disappeared there has been dealt
/// with. Keeping a local row the cloud no longer has would show a publication
/// still pending that nothing will ever carry out.
///
/// **Must be given a transaction.** It empties the table before it refills it,
/// and the gap in between is a local mirror that says no publication is pending
/// for any post. `trash_post` reads that mirror to refuse deleting a post the
/// cloud is about to publish, so a failure partway through would leave the app
/// briefly willing to throw away a post the Worker then puts on the blog.
pub async fn mirror_schedules(
    db: &impl ConnectionTrait,
    remote: Vec<post_schedule::Model>,
) -> AppResult<usize> {
    post_schedule::Entity::delete_many().exec(db).await?;
    let count = remote.len();
    for row in remote {
        row.into_insert().insert(db).await?;
    }
    Ok(count)
}

/// Forget a post's schedule locally. Clearing an absent row is not an error.
pub async fn schedule_clear(db: &impl ConnectionTrait, slug: &str) -> AppResult<()> {
    post_schedule::Entity::delete_by_id(slug.to_string()).exec(db).await?;
    Ok(())
}

// ─── Stale cached bodies (local only) ─────────────────────────────────────────

/// Record that a post's cached Markdown is older than the cloud's — see
/// [`post_body_stale`].
pub async fn body_stale_set(
    db: &impl ConnectionTrait,
    slug: &str,
    since: i64,
) -> AppResult<()> {
    if post_body_stale::Entity::find_by_id(slug.to_string()).one(db).await?.is_some() {
        return Ok(());
    }
    post_body_stale::ActiveModel { slug: Set(slug.to_string()), since: Set(since) }
        .insert(db)
        .await?;
    Ok(())
}

/// Is this post's cached body known to be out of date?
pub async fn body_is_stale(db: &impl ConnectionTrait, slug: &str) -> AppResult<bool> {
    Ok(post_body_stale::Entity::find_by_id(slug.to_string()).one(db).await?.is_some())
}

/// Every slug whose cached body is known to be out of date.
///
/// Nothing in the app asks this: a reader asks about the one post in front of it
/// via [`body_is_stale`]. It is the tests' way of seeing the whole table, which
/// is what tells a mark that was written from one that was written *and* left
/// behind.
#[cfg(test)]
pub async fn stale_bodies(
    db: &impl ConnectionTrait,
) -> AppResult<Vec<post_body_stale::Model>> {
    Ok(post_body_stale::Entity::find().all(db).await?)
}

/// The cached body is gone or has been replaced, so the mark goes with it.
pub async fn body_stale_clear(db: &impl ConnectionTrait, slug: &str) -> AppResult<()> {
    post_body_stale::Entity::delete_by_id(slug.to_string()).exec(db).await?;
    Ok(())
}

// ─── Tombstones (local only) ──────────────────────────────────────────────────

/// Record that a slug was permanently deleted here — see [`post_tombstone`].
pub async fn tombstone_set(
    db: &impl ConnectionTrait,
    slug: &str,
    deleted_at: i64,
) -> AppResult<()> {
    if post_tombstone::Entity::find_by_id(slug.to_string())
        .one(db)
        .await?
        .is_some()
    {
        return Ok(());
    }
    post_tombstone::ActiveModel {
        slug: Set(slug.to_string()),
        deleted_at: Set(deleted_at),
    }
    .insert(db)
    .await?;
    Ok(())
}

/// Every slug this machine has deleted for good.
pub async fn tombstoned_slugs(
    db: &impl ConnectionTrait,
) -> AppResult<std::collections::HashSet<String>> {
    Ok(post_tombstone::Entity::find()
        .all(db)
        .await?
        .into_iter()
        .map(|t| t.slug)
        .collect())
}

/// Forget a tombstone, because the thing it was keeping out is gone.
pub async fn tombstone_clear(db: &impl ConnectionTrait, slug: &str) -> AppResult<()> {
    post_tombstone::Entity::delete_by_id(slug.to_string()).exec(db).await?;
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
    db: &(impl ConnectionTrait + TransactionTrait<Transaction = sea_orm::DatabaseTransaction>),
    model: post_revision::Model,
) -> AppResult<post_revision::Model> {
    let post_id = model.post_id;
    let txn = db.begin().await?;
    require_post(&txn, post_id).await?;
    let created = model.into_insert().insert(&txn).await?;
    prune_revisions(&txn, post_id).await?;
    txn.commit().await?;
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
        let deleted = mirror_posts(&db, vec![post_row("other", None, None)], &[]).await.unwrap().deleted;

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

        let deleted = mirror_posts(&db, vec![], &[]).await.unwrap().deleted;

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

    fn schedule_row(slug: &str, publish_at: i64, state: &str) -> post_schedule::Model {
        post_schedule::Model {
            slug: slug.to_string(),
            publish_at,
            state: state.to_string(),
            error: None,
            updated_at: 0,
        }
    }

    /// The cloud is authoritative about schedules: the Worker is the only thing
    /// that moves one to `published` or `failed`, so a row that has gone there
    /// has been dealt with. Keeping a local row the cloud no longer has would
    /// show a publication still pending that nothing will ever carry out.
    #[tokio::test]
    async fn a_refresh_replaces_the_local_schedules_wholesale() {
        let db = connect_in_memory().await.unwrap();
        schedule_set(&db, schedule_row("stale", 100, post_schedule::PENDING)).await.unwrap();
        schedule_set(&db, schedule_row("kept", 200, post_schedule::PENDING)).await.unwrap();

        // The cloud has moved one on and knows nothing about the other.
        let count = mirror_schedules(
            &db,
            vec![schedule_row("kept", 200, post_schedule::PUBLISHED)],
        )
        .await
        .unwrap();

        assert_eq!(count, 1);
        assert!(schedule_get(&db, "stale").await.unwrap().is_none());
        assert_eq!(
            schedule_get(&db, "kept").await.unwrap().unwrap().state,
            post_schedule::PUBLISHED
        );
    }

    /// Rescheduling is the same row with a different time, not a second one.
    #[tokio::test]
    async fn setting_a_schedule_twice_replaces_it() {
        let db = connect_in_memory().await.unwrap();
        schedule_set(&db, schedule_row("a-post", 100, post_schedule::PENDING)).await.unwrap();
        schedule_set(&db, schedule_row("a-post", 900, post_schedule::PENDING)).await.unwrap();

        let all = schedules_all(&db).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].publish_at, 900);
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

        let deleted = mirror_posts(&db, vec![], &[]).await.unwrap().deleted;

        assert_eq!(deleted, 0);
        assert!(
            get::<post::Model>(&db, post.id).await.unwrap().is_some(),
            "a refresh permanently deleted a post from the trash"
        );
    }

    /// A row in a side table can outlive the post it describes, and the primary
    /// key is a plain `INTEGER PRIMARY KEY` — so SQLite may hand a deleted
    /// post's id to the next one, which would inherit its stage, its idea of
    /// what the cloud holds, and its draft history. A save that loses a race
    /// with a permanent deletion is how that happens, and refusing the write is
    /// what makes it impossible rather than unlikely.
    #[tokio::test]
    async fn side_tables_refuse_a_post_that_no_longer_exists() {
        let db = connect_in_memory().await.unwrap();
        let post = create::<post::Model>(&db, post_row("a-post", None, None)).await.unwrap();
        let id = post.id;
        delete::<post::Model>(&db, id).await.unwrap();

        assert!(
            stage_set(
                &db,
                post_stage::Model { post_id: id, stage: post_stage::DRAFT.into(), staged_at: 0 }
            )
            .await
            .is_err(),
            "a stage row outlived its post"
        );
        assert!(
            sync_set_local(&db, id, "v1".into()).await.is_err(),
            "a sync row outlived its post"
        );
        assert!(
            sync_mark_synced(&db, id, "v1".into(), None, 0).await.is_err(),
            "a sync row outlived its post"
        );
        assert!(
            revision_add(&db, revision_row(id, "somebody else's draft", 1)).await.is_err(),
            "a revision outlived its post"
        );
    }

    /// A refresh writes metadata and never fetches bodies, so a post whose
    /// cloud copy has moved on is left with a cached body from before the move.
    /// Nothing downstream can tell — the editor prefers the cache and would
    /// open the older text, and the media survey would read it as the post's
    /// current references — so the mirror names those slugs for the caller to
    /// clear.
    #[tokio::test]
    async fn a_refresh_names_the_bodies_it_has_left_behind() {
        let db = connect_in_memory().await.unwrap();
        let mut local = post_row("a-post", None, None);
        local.updated_at = 100;
        let post = create::<post::Model>(&db, local).await.unwrap();
        sync_mark_synced(&db, post.id, "v1".into(), Some(100), 100).await.unwrap();

        // Somebody else published a new version.
        let mut remote = post_row("a-post", None, None);
        remote.updated_at = 700;
        mirror_posts(&db, vec![remote.clone()], &[]).await.unwrap();
        let marked: Vec<String> =
            stale_bodies(&db).await.unwrap().into_iter().map(|r| r.slug).collect();
        assert_eq!(marked, ["a-post"]);

        // Refreshing again against the same version changes nothing, and the
        // mark is idempotent rather than piling up.
        body_stale_clear(&db, "a-post").await.unwrap();
        mirror_posts(&db, vec![remote], &[]).await.unwrap();
        assert!(stale_bodies(&db).await.unwrap().is_empty());
    }

    /// A post arriving for the first time has no cached body to be stale, and a
    /// post with unpushed edits is left out of the refresh entirely — its body
    /// is the author's own work, not a stale copy of the cloud's.
    #[tokio::test]
    async fn nothing_is_cleared_for_a_new_post_or_a_locally_edited_one() {
        let db = connect_in_memory().await.unwrap();
        mirror_posts(&db, vec![post_row("fresh", None, None)], &[]).await.unwrap();
        assert!(stale_bodies(&db).await.unwrap().is_empty(), "a first arrival has no cached body");

        let edited = create::<post::Model>(&db, post_row("mine", None, None)).await.unwrap();
        sync_set_local(&db, edited.id, "my-edit".into()).await.unwrap();
        let mut remote = post_row("mine", None, None);
        remote.updated_at = 900;
        mirror_posts(&db, vec![remote], &[]).await.unwrap();
        assert!(
            stale_bodies(&db).await.unwrap().is_empty(),
            "an author's own body was called stale"
        );
    }

    /// "Delete forever" has to survive the next Refresh. The cloud's copy is
    /// deliberately left alone by a local deletion, so without a tombstone the
    /// mirror reads the remote post as one this machine has never seen and
    /// inserts it straight back.
    #[tokio::test]
    async fn a_permanently_deleted_post_is_not_pulled_back() {
        let db = connect_in_memory().await.unwrap();
        let post = create::<post::Model>(&db, post_row("was-live", None, None)).await.unwrap();

        // What `purge` leaves behind.
        delete::<post::Model>(&db, post.id).await.unwrap();
        tombstone_set(&db, "was-live", 1_700_000_000).await.unwrap();

        // The cloud still has it, as it always would.
        mirror_posts(&db, vec![post_row("was-live", None, None)], &[]).await.unwrap();

        assert!(
            post_by_slug(&db, "was-live").await.unwrap().is_none(),
            "a refresh undid a permanent deletion"
        );
    }

    /// And the tombstone is not forever either: once the cloud's copy is gone
    /// there is nothing left to keep out, and a slug nobody is using should not
    /// stay quietly refused.
    #[tokio::test]
    async fn a_tombstone_is_dropped_once_the_cloud_forgets_the_post_too() {
        let db = connect_in_memory().await.unwrap();
        tombstone_set(&db, "was-live", 1_700_000_000).await.unwrap();

        mirror_posts(&db, vec![], &[]).await.unwrap();

        assert!(tombstoned_slugs(&db).await.unwrap().is_empty());

        // So the same slug can come back from the cloud afterwards.
        mirror_posts(&db, vec![post_row("was-live", None, None)], &[]).await.unwrap();
        assert!(post_by_slug(&db, "was-live").await.unwrap().is_some());
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

#[cfg(test)]
mod refresh_deletion_tests {
    use super::*;

    fn a_post(slug: &str) -> post::Model {
        post::Model {
            id: 0,
            slug: slug.to_string(),
            title: slug.to_string(),
            excerpt: None,
            tags: None,
            published: true,
            published_at: None,
            series_id: None,
            series_order: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    /// The loss this guards against: a post that was published, then edited
    /// here, then removed from D1 somewhere else. The local edits exist nowhere
    /// but this machine, and a refresh used to delete the post along with its
    /// revision history because it *had* been pushed once.
    #[tokio::test]
    async fn a_refresh_keeps_a_post_whose_local_edits_were_never_pushed() {
        let db = connect_in_memory().await.unwrap();
        let post = create::<post::Model>(&db, a_post("edited-here")).await.unwrap();

        // Pushed once — both sides agreed on `published`.
        sync_agree(&db, post.id, "published".to_string(), Some(100), 100).await.unwrap();
        // Then edited locally, which is all autosave writes.
        sync_set_local(&db, post.id, "edited".to_string()).await.unwrap();

        // The cloud no longer has it.
        mirror_posts(&db, vec![], &[]).await.unwrap();

        assert!(
            post_by_slug(&db, "edited-here").await.unwrap().is_some(),
            "the refresh deleted a post carrying edits that exist nowhere else"
        );
    }

    /// The case that has always worked, kept honest: never pushed at all.
    #[tokio::test]
    async fn a_refresh_keeps_a_post_that_was_never_pushed() {
        let db = connect_in_memory().await.unwrap();
        let post = create::<post::Model>(&db, a_post("never-pushed")).await.unwrap();
        sync_set_local(&db, post.id, "local".to_string()).await.unwrap();

        mirror_posts(&db, vec![], &[]).await.unwrap();

        assert!(post_by_slug(&db, "never-pushed").await.unwrap().is_some());
    }

    /// And the other direction, so the guard above is not simply "keep
    /// everything": a post in agreement with the cloud holds nothing this
    /// machine would lose, so its removal upstream is a removal here.
    #[tokio::test]
    async fn a_refresh_still_deletes_a_post_that_matches_the_cloud() {
        let db = connect_in_memory().await.unwrap();
        let post = create::<post::Model>(&db, a_post("in-agreement")).await.unwrap();
        sync_agree(&db, post.id, "same".to_string(), Some(100), 100).await.unwrap();

        let result = mirror_posts(&db, vec![], &[]).await.unwrap();

        assert!(post_by_slug(&db, "in-agreement").await.unwrap().is_none());
        assert_eq!(result.deleted, 1);
    }
}

#[cfg(test)]
mod push_baseline_tests {
    use super::*;
    use crate::sync_state::{SyncState, derive};

    fn a_post(slug: &str) -> post::Model {
        post::Model {
            id: 0,
            slug: slug.to_string(),
            title: slug.to_string(),
            excerpt: None,
            tags: None,
            published: true,
            published_at: None,
            series_id: None,
            series_order: None,
            created_at: 0,
            updated_at: 500,
        }
    }

    /// Set up a post that was pushed at version 100, edited here since, and then
    /// pushed again at version 500 — `advance` being whether that second push
    /// recorded the version it wrote, which is what the fix does.
    async fn after_a_push(advance: bool) -> SyncState {
        let db = connect_in_memory().await.unwrap();
        let post = create::<post::Model>(&db, a_post("mine-alone")).await.unwrap();

        sync_agree(&db, post.id, "v1".to_string(), Some(100), 100).await.unwrap();
        sync_set_local(&db, post.id, "v2".to_string()).await.unwrap();
        if advance {
            sync_accept_remote_baseline(&db, post.id, Some(500)).await.unwrap();
        }

        // The refresh that follows sees the row this machine just wrote.
        sync_observe_remote(&db, post.id, 500).await.unwrap();

        let sync = sync_get(&db, post.id).await.unwrap();
        derive(None, sync.as_ref())
    }

    /// A post nobody else has touched must not come back from a refresh asking
    /// which side to keep. The cloud's version moved because *this machine*
    /// moved it, and recording that is what tells the two apart.
    #[tokio::test]
    async fn a_push_does_not_leave_the_post_in_conflict_with_itself() {
        assert_eq!(after_a_push(true).await, SyncState::Modified);
    }

    /// The same sequence without recording the pushed version — the bug, kept
    /// here so the test above cannot pass by accident.
    #[tokio::test]
    async fn without_recording_the_pushed_version_it_conflicts_with_itself() {
        assert_eq!(after_a_push(false).await, SyncState::Conflict);
    }
}

#[cfg(test)]
mod series_tests {
    use super::*;

    fn a_series(slug: &str) -> series::Model {
        series::Model {
            id: 0,
            slug: slug.to_string(),
            title: slug.to_string(),
            description: None,
            created_at: 100,
        }
    }

    fn a_post(slug: &str, series_id: Option<i32>, order: Option<i32>) -> post::Model {
        post::Model {
            id: 0,
            slug: slug.to_string(),
            title: slug.to_string(),
            excerpt: None,
            tags: None,
            published: false,
            published_at: None,
            series_id,
            series_order: order,
            created_at: 0,
            updated_at: 0,
        }
    }

    /// What `delete_series` runs first. Without it the posts keep an id that no
    /// longer names anything.
    #[tokio::test]
    async fn unfiling_empties_one_series_and_leaves_the_others() {
        let db = connect_in_memory().await.unwrap();
        let kept = create::<series::Model>(&db, a_series("kept")).await.unwrap();
        let going = create::<series::Model>(&db, a_series("going")).await.unwrap();

        create::<post::Model>(&db, a_post("in-going", Some(going.id), Some(1)))
            .await
            .unwrap();
        create::<post::Model>(&db, a_post("also-going", Some(going.id), Some(2)))
            .await
            .unwrap();
        create::<post::Model>(&db, a_post("in-kept", Some(kept.id), Some(1)))
            .await
            .unwrap();
        create::<post::Model>(&db, a_post("unfiled", None, None)).await.unwrap();

        assert_eq!(unfile_series(&db, going.id).await.unwrap(), 2);

        let posts = post::Entity::find().all(&db).await.unwrap();
        for post in &posts {
            match post.slug.as_str() {
                "in-going" | "also-going" => {
                    assert_eq!(post.series_id, None, "{} should have been unfiled", post.slug);
                    // The order goes with the membership: a position in a series
                    // the post is no longer in is a number about nothing.
                    assert_eq!(post.series_order, None);
                }
                "in-kept" => assert_eq!(post.series_id, Some(kept.id)),
                _ => assert_eq!(post.series_id, None),
            }
        }
    }

    /// The rule that keeps a pull from refiling every post: the local row keeps
    /// its own id, because that is the number the posts point at.
    #[tokio::test]
    async fn a_series_from_the_cloud_keeps_the_local_id_its_posts_use() {
        let db = connect_in_memory().await.unwrap();
        let local = create::<series::Model>(&db, a_series("shared")).await.unwrap();
        let post = create::<post::Model>(&db, a_post("filed", Some(local.id), Some(1)))
            .await
            .unwrap();

        // The cloud numbers its own rows, so the same series arrives wearing a
        // different id and a title edited elsewhere.
        let remote = series::Model {
            id: local.id + 999,
            slug: "shared".to_string(),
            title: "Renamed in the cloud".to_string(),
            description: Some("from up there".to_string()),
            created_at: 900,
        };
        upsert_series_from_remote(&db, remote).await.unwrap();

        let rows = series::Entity::find().all(&db).await.unwrap();
        assert_eq!(rows.len(), 1, "matched by slug, not inserted a second time");
        assert_eq!(rows[0].id, local.id);
        assert_eq!(rows[0].title, "Renamed in the cloud");
        assert_eq!(rows[0].description.as_deref(), Some("from up there"));
        // Local `created_at` is left where it was.
        assert_eq!(rows[0].created_at, 100);

        // The post is still filed where it was.
        let refreshed = get::<post::Model>(&db, post.id).await.unwrap().unwrap();
        assert_eq!(refreshed.series_id, Some(local.id));
    }

    #[tokio::test]
    async fn a_series_the_cloud_has_and_this_machine_does_not_is_added() {
        let db = connect_in_memory().await.unwrap();
        upsert_series_from_remote(&db, a_series("new-here")).await.unwrap();
        assert!(series_by_slug(&db, "new-here").await.unwrap().is_some());
    }
}
