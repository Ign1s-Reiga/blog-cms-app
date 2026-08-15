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
use crate::entities::{post, post_stage, post_sync, series};
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
pub async fn mirror_posts(
    db: &DatabaseConnection,
    remote: Vec<post::Model>,
    remote_series: &[series::Model],
) -> AppResult<(usize, usize)> {
    let remote_slugs: std::collections::HashSet<String> =
        remote.iter().map(|p| p.slug.clone()).collect();
    let upserted = remote.len();
    let series = SeriesMap::build(db, remote_series).await?;

    for post in remote {
        upsert_post_from_remote(db, post, &series).await?;
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
fn resolve_series(
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
) -> AppResult<()> {
    let existing = post_by_slug(db, &remote.slug).await?;

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

    // Only posts with nothing pending reach this point, so the record here
    // describes nothing and goes — which is what keeps "no record" meaning
    // "nothing has been touched here".
    sync_clear(db, saved.id).await?;
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
    at: i64,
) -> AppResult<post_sync::Model> {
    let existing = post_sync::Entity::find_by_id(post_id).one(db).await?;
    // A push makes this machine's copy the newest thing either side has, so the
    // cloud's observed version becomes the baseline: whatever the refresh last
    // saw has now been superseded by us, not by someone else.
    let observed = existing.as_ref().and_then(|row| row.remote_seen_at);
    let model = post_sync::ActiveModel {
        post_id: if existing.is_some() {
            sea_orm::ActiveValue::Unchanged(post_id)
        } else {
            Set(post_id)
        },
        local_hash: Set(hash.clone()),
        synced_hash: Set(Some(hash)),
        synced_at: Set(Some(at)),
        remote_updated_at: Set(observed),
        remote_seen_at: Set(observed),
    };
    Ok(if existing.is_some() { model.update(db).await? } else { model.insert(db).await? })
}

/// Forget a post's sync record. Clearing an absent row is not an error.
pub async fn sync_clear(db: &impl ConnectionTrait, post_id: i32) -> AppResult<()> {
    post_sync::Entity::delete_by_id(post_id).exec(db).await?;
    Ok(())
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
        sync_mark_synced(&db, post.id, "v1".into(), 1_700_000_000).await.unwrap();
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
        sync_mark_synced(&db, post.id, "v1".into(), 1_700_000_000).await.unwrap();

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

    /// Where there is nothing pending, the record describes nothing and goes —
    /// which is what keeps "no record" meaning "nothing has been touched here".
    #[tokio::test]
    async fn a_refresh_forgets_the_record_of_a_post_with_nothing_pending() {
        let db = connect_in_memory().await.unwrap();
        let post = create::<post::Model>(&db, post_row("a-post", None, None)).await.unwrap();
        sync_mark_synced(&db, post.id, "v1".into(), 1_700_000_000).await.unwrap();

        mirror_posts(&db, vec![post_row("a-post", None, None)], &[]).await.unwrap();

        assert!(sync_get(&db, post.id).await.unwrap().is_none());
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
