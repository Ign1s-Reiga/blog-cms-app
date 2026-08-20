use reqwest::Client;
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{
    ColumnTrait, DbBackend, EntityTrait, Insert, QueryFilter, QueryOrder, QueryTrait, UpdateMany,
    Value, Values,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::entities::record::{Id, Record};
use crate::entities::{post, post_schedule, series};
use crate::error::{AppError, AppResult};

// ─── Config ───────────────────────────────────────────────────────────────────

/// Cloudflare credentials resolved from environment variables at call time.
/// Set these before launching the app:
///   CF_ACCOUNT_ID      — Cloudflare account ID
///   CF_API_TOKEN       — API token with R2:Edit and D1:Edit permissions
///   CF_R2_BUCKET       — R2 bucket name
///   CF_D1_DATABASE_ID  — D1 database ID (UUID from the dashboard)
#[derive(Clone, Serialize, Deserialize)]
pub struct CloudflareConfig {
    pub account_id:    String,
    pub api_token:     String,
    pub r2_bucket:     String,
    pub d1_database_id: String,
    /// Public origin the bucket is served from, e.g. `https://cdn.example.com`.
    /// Written into published Markdown as the base for image URLs, so it must
    /// match the blog's `R2_PUBLIC_URL`. Empty for credentials saved before
    /// this field existed; publishing reports that rather than emitting broken
    /// links.
    pub r2_public_url: String,
    /// Key layout for a post's thumbnail. Must match the blog's `thumbnailKey`,
    /// which derives it from the slug alone.
    pub thumbnail_key_pattern: String,
    /// Key layout for images used in a post body. Free to change: the reader
    /// never derives these, it follows the URL written into the Markdown.
    pub media_key_pattern: String,
}

impl CloudflareConfig {
    pub fn from_env() -> AppResult<Self> {
        let missing = AppError::MissingEnv;
        Ok(Self {
            account_id:     std::env::var("CF_ACCOUNT_ID")    .map_err(|_| missing("CF_ACCOUNT_ID"))?,
            api_token:      std::env::var("CF_API_TOKEN")     .map_err(|_| missing("CF_API_TOKEN"))?,
            r2_bucket:      std::env::var("CF_R2_BUCKET")     .map_err(|_| missing("CF_R2_BUCKET"))?,
            d1_database_id: std::env::var("CF_D1_DATABASE_ID").map_err(|_| missing("CF_D1_DATABASE_ID"))?,
            r2_public_url:  std::env::var("CF_R2_PUBLIC_URL") .map_err(|_| missing("CF_R2_PUBLIC_URL"))?,
            thumbnail_key_pattern: std::env::var("CF_THUMBNAIL_KEY_PATTERN")
                .unwrap_or_else(|_| crate::media_keys::DEFAULT_THUMBNAIL_PATTERN.to_string()),
            media_key_pattern: std::env::var("CF_MEDIA_KEY_PATTERN")
                .unwrap_or_else(|_| crate::media_keys::DEFAULT_MEDIA_PATTERN.to_string()),
        })
    }
}

// ─── R2 ───────────────────────────────────────────────────────────────────────

fn r2_object_url(config: &CloudflareConfig, key: &str) -> String {
    format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/r2/buckets/{}/objects/{}",
        config.account_id, config.r2_bucket, key
    )
}

/// Turn a non-success response into an error carrying its status and body.
///
/// Consumes the response, because reading the body is what makes the message
/// worth having — Cloudflare explains the refusal there, not in the status.
async fn status_error(
    service: &'static str,
    op: &'static str,
    response: reqwest::Response,
) -> AppError {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    AppError::Cloudflare { service, op, status, body }
}

/// Upload raw bytes to R2 at `key` with the given `content_type`.
pub async fn upload_bytes_to_r2(
    client: &Client,
    config: &CloudflareConfig,
    key: &str,
    bytes: Vec<u8>,
    content_type: &str,
) -> AppResult<()> {
    let response = client
        .put(r2_object_url(config, key))
        .header("Authorization", format!("Bearer {}", config.api_token))
        .header("Content-Type", content_type)
        .body(bytes)
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(status_error("R2", "upload", response).await);
    }
    Ok(())
}

/// Upload UTF-8 text (e.g. a post's Markdown) to R2.
pub async fn upload_to_r2(
    client: &Client,
    config: &CloudflareConfig,
    key: &str,
    content: &str,
) -> AppResult<()> {
    upload_bytes_to_r2(
        client,
        config,
        key,
        content.as_bytes().to_vec(),
        "text/markdown; charset=utf-8",
    )
    .await
}

/// Download an object's bytes from R2. Returns `Ok(None)` when it doesn't exist.
pub async fn download_bytes_from_r2(
    client: &Client,
    config: &CloudflareConfig,
    key: &str,
) -> AppResult<Option<Vec<u8>>> {
    let response = client
        .get(r2_object_url(config, key))
        .header("Authorization", format!("Bearer {}", config.api_token))
        .send()
        .await?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(status_error("R2", "download", response).await);
    }

    let bytes = response.bytes().await?;
    Ok(Some(bytes.to_vec()))
}

/// Download an object's text from R2. Returns `Ok(None)` when it doesn't exist.
pub async fn download_from_r2(
    client: &Client,
    config: &CloudflareConfig,
    key: &str,
) -> AppResult<Option<String>> {
    match download_bytes_from_r2(client, config, key).await? {
        Some(bytes) => Ok(Some(String::from_utf8(bytes)?)),
        None => Ok(None),
    }
}

/// Delete an object from R2. A missing object (404) is treated as success.
pub async fn delete_from_r2(
    client: &Client,
    config: &CloudflareConfig,
    key: &str,
) -> AppResult<()> {
    let response = client
        .delete(r2_object_url(config, key))
        .header("Authorization", format!("Bearer {}", config.api_token))
        .send()
        .await?;

    if response.status() == reqwest::StatusCode::NOT_FOUND || response.status().is_success() {
        return Ok(());
    }
    Err(status_error("R2", "delete", response).await)
}

/// An object listed from R2.
#[derive(Deserialize, Serialize, Clone)]
pub struct R2Object {
    pub key: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Deserialize)]
struct R2ListResponse {
    success: bool,
    #[serde(default)]
    errors: Vec<D1Error>,
    #[serde(default)]
    result: Vec<R2Object>,
    /// Where the listing stopped. Absent on responses that carry the whole set.
    #[serde(default)]
    result_info: R2ListInfo,
}

/// The paging half of an R2 listing.
#[derive(Deserialize, Default)]
struct R2ListInfo {
    /// Opaque marker to resume from, sent back as `cursor`.
    #[serde(default)]
    cursor: Option<String>,
    /// Whether R2 stopped early because the page filled up.
    #[serde(default)]
    is_truncated: bool,
}

/// List objects in R2 whose key starts with `prefix` (e.g. `"media/"`).
///
/// Follows the cursor to the end. R2 returns at most a thousand keys per call
/// and reports that in `result_info`; a caller that stops at the first page gets
/// a partial bucket with nothing to say so. That matters most to `media_usage`,
/// which decides whether an object is safe to delete — a missing page reads as
/// nothing using it.
pub async fn list_r2(
    client: &Client,
    config: &CloudflareConfig,
    prefix: &str,
) -> AppResult<Vec<R2Object>> {
    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/r2/buckets/{}/objects",
        config.account_id, config.r2_bucket
    );

    let mut objects: Vec<R2Object> = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let mut request = client
            .get(&url)
            .query(&[("prefix", prefix)])
            .header("Authorization", format!("Bearer {}", config.api_token));
        if let Some(from) = &cursor {
            request = request.query(&[("cursor", from.as_str())]);
        }

        let response = request.send().await?;
        if !response.status().is_success() {
            return Err(status_error("R2", "list", response).await);
        }
        let text = response.text().await.unwrap_or_default();

        let parsed: R2ListResponse =
            serde_json::from_str(&text).map_err(|e| AppError::json("R2 list parse error", e))?;
        if !parsed.success {
            return Err(AppError::CloudflareApi {
                service: "R2",
                op: "list",
                message: parsed
                    .errors
                    .first()
                    .map(|e| e.message.clone())
                    .unwrap_or_else(|| "unknown R2 error".to_string()),
            });
        }

        objects.extend(parsed.result);

        if !parsed.result_info.is_truncated {
            break;
        }
        let next = parsed.result_info.cursor.filter(|c| !c.is_empty());
        match next {
            // A cursor that has not moved would fetch the same page forever.
            // A partial answer is bad; not returning is worse.
            Some(from) if Some(&from) == cursor.as_ref() => {
                log::warn!("R2 listing of `{prefix}` returned the same cursor twice; stopping");
                break;
            }
            Some(from) => cursor = Some(from),
            // Truncated with nowhere to resume from. Nothing further to ask for,
            // but the caller is about to act on a partial listing, so say so.
            None => {
                log::warn!("R2 listing of `{prefix}` is truncated but carries no cursor");
                break;
            }
        }
    }

    Ok(objects)
}

// ─── D1 ───────────────────────────────────────────────────────────────────────
//
// D1 has no raw SQLite wire protocol, so Sea ORM can't connect to it directly.
// Instead we build each statement with Sea ORM for the SQLite backend
// (`.build(DbBackend::Sqlite)` → SQL + bound params) and run it against D1's
// HTTP `/query` endpoint. The `series` and `blog-db` tables must already exist
// in D1 (created by the web app's Drizzle migrations).

/// Cloudflare's query-response envelope (only the parts we read).
#[derive(Deserialize)]
struct D1Envelope {
    success: bool,
    #[serde(default)]
    errors: Vec<D1Error>,
    #[serde(default)]
    result: Vec<D1QueryResult>,
}

#[derive(Deserialize)]
struct D1QueryResult {
    #[serde(default)]
    results: Vec<serde_json::Value>,
    #[serde(default)]
    meta: D1Meta,
}

#[derive(Deserialize, Default)]
struct D1Meta {
    #[serde(default)]
    last_row_id: i64,
    /// Rows the statement actually altered. The only way to find out whether a
    /// conditional update matched anything — which is how a cancellation asks
    /// "was this still mine to cancel?".
    #[serde(default)]
    changes: i64,
}

#[derive(Deserialize)]
struct D1Error {
    message: String,
}

/// Convert a built statement's bound values into D1's JSON `params` array.
/// Booleans map to `0`/`1` to match SQLite's integer storage.
fn params_json(values: Option<Values>) -> Vec<serde_json::Value> {
    values
        .map(|vs| vs.0.into_iter().map(value_json).collect())
        .unwrap_or_default()
}

fn value_json(v: Value) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        Value::Bool(Some(b)) => J::from(i32::from(b)),
        Value::Int(Some(i)) => J::from(i),
        Value::BigInt(Some(i)) => J::from(i),
        Value::String(Some(s)) => J::String(s.to_string()),
        _ => J::Null,
    }
}

/// Build a Sea ORM statement for the SQLite backend and run it against D1.
async fn d1_run<S: QueryTrait>(
    client: &Client,
    config: &CloudflareConfig,
    stmt: S,
) -> AppResult<D1Envelope> {
    let statement = stmt.build(DbBackend::Sqlite);
    let params = params_json(statement.values.clone());
    d1_query(client, config, statement.sql.clone(), params).await
}

/// POST raw SQL + params to the D1 HTTP query endpoint and surface any errors.
async fn d1_query(
    client: &Client,
    config: &CloudflareConfig,
    sql: String,
    params: Vec<serde_json::Value>,
) -> AppResult<D1Envelope> {
    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/d1/database/{}/query",
        config.account_id, config.d1_database_id
    );

    let body = serde_json::json!({ "sql": sql, "params": params });

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", config.api_token))
        .json(&body)
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(status_error("D1", "HTTP", response).await);
    }
    let text = response.text().await.unwrap_or_default();

    let env: D1Envelope =
        serde_json::from_str(&text).map_err(|e| AppError::json("D1 response parse error", e))?;
    if !env.success {
        return Err(AppError::CloudflareApi {
            service: "D1",
            op: "query",
            message: env
                .errors
                .first()
                .map(|e| e.message.clone())
                .unwrap_or_else(|| "unknown D1 error".to_string()),
        });
    }
    Ok(env)
}

/// Decode the first result set's rows into models.
fn decode_rows<M: DeserializeOwned>(env: D1Envelope) -> AppResult<Vec<M>> {
    let rows = env
        .result
        .into_iter()
        .next()
        .map(|r| r.results)
        .unwrap_or_default();
    rows.into_iter()
        .map(|row| {
            serde_json::from_value::<M>(row).map_err(|e| AppError::json("D1 row decode error", e))
        })
        .collect()
}

/// The auto-assigned row id reported by an INSERT.
fn last_row_id(env: &D1Envelope) -> i64 {
    env.result.first().map(|r| r.meta.last_row_id).unwrap_or_default()
}

/// How many rows a statement changed.
fn rows_changed(env: &D1Envelope) -> i64 {
    env.result.first().map(|r| r.meta.changes).unwrap_or_default()
}

// ─── D1 CRUD ──────────────────────────────────────────────────────────────────
//
// One implementation per operation, shared by every entity implementing
// `Record`. Update stays per-entity below: it names each column explicitly, so
// there is nothing common to factor out.

/// Insert a row, returning the id D1 assigned it.
pub async fn d1_insert<M>(client: &Client, config: &CloudflareConfig, model: M) -> AppResult<i64>
where
    M: Record,
    Insert<<M::Entity as EntityTrait>::ActiveModel>: QueryTrait,
{
    let env = d1_run(client, config, M::Entity::insert(model.into_insert())).await?;
    Ok(last_row_id(&env))
}

/// Every row, newest first by the record's own ordering column.
pub async fn d1_list<M>(client: &Client, config: &CloudflareConfig) -> AppResult<Vec<M>>
where
    M: Record + DeserializeOwned,
{
    let env = d1_run(
        client,
        config,
        M::Entity::find().order_by_desc(M::order_column()),
    )
    .await?;
    decode_rows(env)
}

/// One row by primary key, or `None` when it does not exist.
pub async fn d1_get<M>(
    client: &Client,
    config: &CloudflareConfig,
    id: Id<M>,
) -> AppResult<Option<M>>
where
    M: Record + DeserializeOwned,
{
    let env = d1_run(client, config, M::Entity::find_by_id(id)).await?;
    Ok(decode_rows::<M>(env)?.into_iter().next())
}

/// Delete by primary key.
pub async fn d1_delete<M: Record>(
    client: &Client,
    config: &CloudflareConfig,
    id: Id<M>,
) -> AppResult<()> {
    d1_run(client, config, M::Entity::delete_by_id(id))
        .await
        .map(|_| ())
}

// ── Posts ──────────────────────────────────────────────────────────────────────

/// Insert a post into D1, or update the existing row with the same `slug`
/// (local wins). Used by the sync action to push local posts to the cloud
/// without needing the D1 row id.
/// Write a post's schedule to D1, replacing any existing one for that slug.
///
/// This is the row the Worker's cron reads, so it is the schedule — the local
/// copy is a mirror of it. Upserted rather than inserted because rescheduling
/// and cancelling are the same operation with a different `publish_at` or
/// `state`, and a post has at most one pending publication.
pub async fn d1_schedule_upsert(
    client: &Client,
    config: &CloudflareConfig,
    model: post_schedule::Model,
) -> AppResult<()> {
    let stmt = post_schedule::Entity::insert(model.into_insert()).on_conflict(
        OnConflict::column(post_schedule::Column::Slug)
            .update_columns([
                post_schedule::Column::PublishAt,
                post_schedule::Column::State,
                post_schedule::Column::Error,
                post_schedule::Column::UpdatedAt,
            ])
            .to_owned(),
    );
    d1_run(client, config, stmt).await.map(|_| ())
}

/// Call off a pending publication, if it is still pending.
///
/// Conditional on purpose, and the condition is the whole point. A Worker run
/// claims a schedule by moving it to `publishing` before it acts; an
/// unconditional cancellation landing at that moment would leave a row marked
/// `cancelled` for a post that went live seconds later — and the Worker's own
/// write-back, which is equally guarded, would find nothing to update and say
/// nothing about it.
///
/// Returns `false` when there was nothing to cancel: already claimed, already
/// published, or already settled some other way.
pub async fn d1_schedule_cancel(
    client: &Client,
    config: &CloudflareConfig,
    slug: &str,
    now: i64,
) -> AppResult<bool> {
    let stmt = post_schedule::Entity::update_many()
        .col_expr(
            post_schedule::Column::State,
            Expr::value(post_schedule::CANCELLED),
        )
        .col_expr(post_schedule::Column::Error, Expr::value(Value::String(None)))
        .col_expr(post_schedule::Column::UpdatedAt, Expr::value(now))
        .filter(post_schedule::Column::Slug.eq(slug))
        .filter(post_schedule::Column::State.eq(post_schedule::PENDING));

    let env = d1_run(client, config, stmt).await?;
    Ok(rows_changed(&env) > 0)
}

pub async fn d1_post_upsert(
    client: &Client,
    config: &CloudflareConfig,
    model: post::Model,
) -> AppResult<()> {
    let stmt = post::Entity::insert(model.into_insert()).on_conflict(
        OnConflict::column(post::Column::Slug)
            .update_columns([
                post::Column::Title,
                post::Column::Excerpt,
                post::Column::Tags,
                post::Column::Published,
                post::Column::PublishedAt,
                post::Column::SeriesId,
                post::Column::SeriesOrder,
                post::Column::CreatedAt,
                post::Column::UpdatedAt,
            ])
            .to_owned(),
    );
    d1_run(client, config, stmt).await.map(|_| ())
}

/// The UPDATE behind [`d1_post_update`]. Split out so a test can read the SQL
/// without a Cloudflare account to send it to — the predicate is the whole point
/// of this function, and it is invisible from the outside.
///
/// The slug is the key, so it is not among the columns written.
fn post_update_stmt(model: post::Model) -> UpdateMany<post::Entity> {
    // UpdateOne isn't a `QueryTrait`, so build an UPDATE … WHERE explicitly.
    post::Entity::update_many()
        .col_expr(post::Column::Title, Expr::value(model.title))
        .col_expr(post::Column::Excerpt, Expr::value(model.excerpt))
        .col_expr(post::Column::Tags, Expr::value(model.tags))
        .col_expr(post::Column::Published, Expr::value(model.published))
        .col_expr(post::Column::PublishedAt, Expr::value(model.published_at))
        .col_expr(post::Column::SeriesId, Expr::value(model.series_id))
        .col_expr(post::Column::SeriesOrder, Expr::value(model.series_order))
        .col_expr(post::Column::CreatedAt, Expr::value(model.created_at))
        .col_expr(post::Column::UpdatedAt, Expr::value(model.updated_at))
        .filter(post::Column::Slug.eq(model.slug))
}

/// Update a post in D1, finding it **by slug**.
///
/// Not by id, though the model carries one. The two databases hand out ids
/// independently and nothing ever copies one across: [`d1_post_upsert`] inserts
/// with the id unset and settles collisions on the slug, so D1 numbers its own
/// rows, and a post pulled the other way is inserted locally with whatever
/// SQLite assigns (`db::upsert_post_from_remote`). A local id is therefore not a
/// name for anything up there. Used as one it addresses whichever post happens
/// to hold that number in D1 — a *different* article, rewritten with this one's
/// title and tags and taken off the blog — or no row at all.
///
/// The slug is the name both sides agree on: unique in each, what the R2 keys
/// are built from, fixed when the post is created and preserved by every save
/// afterwards, so it still names the same post later.
pub async fn d1_post_update(
    client: &Client,
    config: &CloudflareConfig,
    model: post::Model,
) -> AppResult<()> {
    let slug = model.slug.clone();
    let env = d1_run(client, config, post_update_stmt(model)).await?;
    // Nothing with that slug is up there. Reported rather than returned as
    // success, because the caller has already written the local row to match a
    // cloud change that did not happen — and quiet agreement between the two is
    // how a post comes to read as a draft here while readers are still served it.
    if rows_changed(&env) == 0 {
        return Err(AppError::RemotePostMissing(slug));
    }
    Ok(())
}

// ── Series ─────────────────────────────────────────────────────────────────────

pub async fn d1_series_update(
    client: &Client,
    config: &CloudflareConfig,
    model: series::Model,
) -> AppResult<()> {
    let stmt = series::Entity::update_many()
        .col_expr(series::Column::Slug, Expr::value(model.slug))
        .col_expr(series::Column::Title, Expr::value(model.title))
        .col_expr(series::Column::Description, Expr::value(model.description))
        .col_expr(series::Column::CreatedAt, Expr::value(model.created_at))
        .filter(series::Column::Id.eq(model.id));
    d1_run(client, config, stmt).await.map(|_| ())
}

// ─── Client ─────────────────────────────────────────────────────────────────

/// A reqwest client plus the signed-in Cloudflare credentials.
pub fn cf() -> AppResult<(Client, CloudflareConfig)> {
    let config = crate::auth::get_creds().ok_or(AppError::NotConfigured)?;
    Ok((Client::new(), config))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_post() -> post::Model {
        post::Model {
            // Deliberately not 1: a local id that means nothing in D1 is the
            // whole hazard, and a value that could coincide with the first row
            // there would hide it.
            id: 4321,
            slug: "hello-world".to_string(),
            title: "Hello".to_string(),
            excerpt: None,
            tags: None,
            published: false,
            published_at: None,
            series_id: None,
            series_order: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    /// The post being updated is found by slug, never by id.
    ///
    /// Ids are assigned separately by the two databases, so an UPDATE keyed on a
    /// local one addresses a different article in D1 or nothing at all. That is
    /// not visible from the call site — the model carries both fields and the
    /// wrong one compiles — so the predicate is asserted here.
    #[test]
    fn post_update_is_keyed_by_slug() {
        let sql = post_update_stmt(sample_post()).build(DbBackend::Sqlite).sql;
        let (set_clause, where_clause) = sql.split_once("WHERE").expect("update must be filtered");

        assert!(
            where_clause.contains("slug"),
            "expected the row to be found by slug, got: {sql}"
        );
        assert!(
            !where_clause.contains("\"id\""),
            "the id must not narrow the update: {sql}"
        );
        // The key is not also cargo: writing it would be a no-op at best, and at
        // worst would look like a rename this statement cannot perform.
        assert!(
            !set_clause.contains("\"slug\""),
            "slug is the key, so it should not be assigned: {sql}"
        );
    }

    /// A truncated page carries the marker to resume from. Parsed here because
    /// dropping these two fields is what made a large library list its first
    /// thousand objects and call that the bucket.
    #[test]
    fn a_truncated_listing_parses_its_cursor() {
        let body = r#"{
            "success": true,
            "errors": [],
            "result": [{"key": "media/a.avif", "size": 12}],
            "result_info": {"cursor": "next-page", "is_truncated": true, "per_page": 1000}
        }"#;
        let parsed: R2ListResponse = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.result.len(), 1);
        assert!(parsed.result_info.is_truncated);
        assert_eq!(parsed.result_info.cursor.as_deref(), Some("next-page"));
    }

    /// A complete listing has no `result_info` at all, and must not be mistaken
    /// for a truncated one.
    #[test]
    fn a_complete_listing_has_no_cursor() {
        let body = r#"{"success": true, "errors": [], "result": []}"#;
        let parsed: R2ListResponse = serde_json::from_str(body).unwrap();
        assert!(!parsed.result_info.is_truncated);
        assert_eq!(parsed.result_info.cursor, None);
    }

    /// The columns a caller expects to travel with a publish or unpublish.
    #[test]
    fn post_update_writes_the_readers_facing_columns() {
        let sql = post_update_stmt(sample_post()).build(DbBackend::Sqlite).sql;
        for column in ["title", "excerpt", "tags", "published", "published_at"] {
            assert!(sql.contains(column), "expected `{column}` in: {sql}");
        }
    }
}
