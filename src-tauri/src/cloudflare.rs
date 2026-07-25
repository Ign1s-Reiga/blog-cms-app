use reqwest::Client;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ColumnTrait, DbBackend, EntityTrait, QueryFilter, QueryOrder, QueryTrait, Value, Values,
};
use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::entities::{post, series};

// ─── Config ───────────────────────────────────────────────────────────────────

/// Cloudflare credentials resolved from environment variables at call time.
/// Set these before launching the app:
///   CF_ACCOUNT_ID      — Cloudflare account ID
///   CF_API_TOKEN       — API token with R2:Edit and D1:Edit permissions
///   CF_R2_BUCKET       — R2 bucket name
///   CF_D1_DATABASE_ID  — D1 database ID (UUID from the dashboard)
pub struct CloudflareConfig {
    pub account_id:    String,
    pub api_token:     String,
    pub r2_bucket:     String,
    pub d1_database_id: String,
}

impl CloudflareConfig {
    pub fn from_env() -> Result<Self, String> {
        let missing = |name: &str| format!("Environment variable `{}` is not set", name);
        Ok(Self {
            account_id:     std::env::var("CF_ACCOUNT_ID")    .map_err(|_| missing("CF_ACCOUNT_ID"))?,
            api_token:      std::env::var("CF_API_TOKEN")     .map_err(|_| missing("CF_API_TOKEN"))?,
            r2_bucket:      std::env::var("CF_R2_BUCKET")     .map_err(|_| missing("CF_R2_BUCKET"))?,
            d1_database_id: std::env::var("CF_D1_DATABASE_ID").map_err(|_| missing("CF_D1_DATABASE_ID"))?,
        })
    }
}

// ─── R2 ───────────────────────────────────────────────────────────────────────

/// Upload `content` to R2 at the given object `key` (e.g. `"posts/uuid.md"`).
pub async fn upload_to_r2(
    client: &Client,
    config: &CloudflareConfig,
    key: &str,
    content: &str,
) -> Result<(), String> {
    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/r2/buckets/{}/objects/{}",
        config.account_id, config.r2_bucket, key
    );

    let response = client
        .put(&url)
        .header("Authorization", format!("Bearer {}", config.api_token))
        .header("Content-Type", "text/markdown; charset=utf-8")
        .body(content.to_owned())
        .send()
        .await
        .map_err(|e| format!("R2 request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("R2 upload error ({status}): {body}"));
    }

    Ok(())
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
) -> Result<D1Envelope, String> {
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
) -> Result<D1Envelope, String> {
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
        .await
        .map_err(|e| format!("D1 request failed: {e}"))?;

    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("D1 HTTP error ({status}): {text}"));
    }

    let env: D1Envelope =
        serde_json::from_str(&text).map_err(|e| format!("D1 response parse error: {e}"))?;
    if !env.success {
        let msg = env
            .errors
            .first()
            .map(|e| e.message.as_str())
            .unwrap_or("unknown D1 error");
        return Err(format!("D1 query failed: {msg}"));
    }
    Ok(env)
}

/// Decode the first result set's rows into models.
fn decode_rows<M: DeserializeOwned>(env: D1Envelope) -> Result<Vec<M>, String> {
    let rows = env
        .result
        .into_iter()
        .next()
        .map(|r| r.results)
        .unwrap_or_default();
    rows.into_iter()
        .map(|row| serde_json::from_value::<M>(row).map_err(|e| format!("D1 row decode error: {e}")))
        .collect()
}

/// The auto-assigned row id reported by an INSERT.
fn last_row_id(env: &D1Envelope) -> i64 {
    env.result.first().map(|r| r.meta.last_row_id).unwrap_or_default()
}

// ── Posts ──────────────────────────────────────────────────────────────────────

pub async fn d1_post_insert(
    client: &Client,
    config: &CloudflareConfig,
    model: post::Model,
) -> Result<i64, String> {
    let env = d1_run(client, config, post::Entity::insert(model.into_insert())).await?;
    Ok(last_row_id(&env))
}

pub async fn d1_post_update(
    client: &Client,
    config: &CloudflareConfig,
    model: post::Model,
) -> Result<(), String> {
    // UpdateOne isn't a `QueryTrait`, so build an UPDATE … WHERE id = ? explicitly.
    let stmt = post::Entity::update_many()
        .col_expr(post::Column::Slug, Expr::value(model.slug))
        .col_expr(post::Column::Title, Expr::value(model.title))
        .col_expr(post::Column::Excerpt, Expr::value(model.excerpt))
        .col_expr(post::Column::Tags, Expr::value(model.tags))
        .col_expr(post::Column::Published, Expr::value(model.published))
        .col_expr(post::Column::PublishedAt, Expr::value(model.published_at))
        .col_expr(post::Column::SeriesId, Expr::value(model.series_id))
        .col_expr(post::Column::SeriesOrder, Expr::value(model.series_order))
        .col_expr(post::Column::CreatedAt, Expr::value(model.created_at))
        .col_expr(post::Column::UpdatedAt, Expr::value(model.updated_at))
        .filter(post::Column::Id.eq(model.id));
    d1_run(client, config, stmt).await.map(|_| ())
}

pub async fn d1_post_delete(
    client: &Client,
    config: &CloudflareConfig,
    id: i32,
) -> Result<(), String> {
    d1_run(client, config, post::Entity::delete_by_id(id))
        .await
        .map(|_| ())
}

pub async fn d1_post_list(
    client: &Client,
    config: &CloudflareConfig,
) -> Result<Vec<post::Model>, String> {
    let env = d1_run(
        client,
        config,
        post::Entity::find().order_by_desc(post::Column::CreatedAt),
    )
    .await?;
    decode_rows(env)
}

pub async fn d1_post_get(
    client: &Client,
    config: &CloudflareConfig,
    id: i32,
) -> Result<Option<post::Model>, String> {
    let env = d1_run(client, config, post::Entity::find_by_id(id)).await?;
    Ok(decode_rows::<post::Model>(env)?.into_iter().next())
}

// ── Series ─────────────────────────────────────────────────────────────────────

pub async fn d1_series_insert(
    client: &Client,
    config: &CloudflareConfig,
    model: series::Model,
) -> Result<i64, String> {
    let env = d1_run(client, config, series::Entity::insert(model.into_insert())).await?;
    Ok(last_row_id(&env))
}

pub async fn d1_series_update(
    client: &Client,
    config: &CloudflareConfig,
    model: series::Model,
) -> Result<(), String> {
    let stmt = series::Entity::update_many()
        .col_expr(series::Column::Slug, Expr::value(model.slug))
        .col_expr(series::Column::Title, Expr::value(model.title))
        .col_expr(series::Column::Description, Expr::value(model.description))
        .col_expr(series::Column::CreatedAt, Expr::value(model.created_at))
        .filter(series::Column::Id.eq(model.id));
    d1_run(client, config, stmt).await.map(|_| ())
}

pub async fn d1_series_delete(
    client: &Client,
    config: &CloudflareConfig,
    id: i32,
) -> Result<(), String> {
    d1_run(client, config, series::Entity::delete_by_id(id))
        .await
        .map(|_| ())
}

pub async fn d1_series_list(
    client: &Client,
    config: &CloudflareConfig,
) -> Result<Vec<series::Model>, String> {
    let env = d1_run(
        client,
        config,
        series::Entity::find().order_by_desc(series::Column::CreatedAt),
    )
    .await?;
    decode_rows(env)
}

pub async fn d1_series_get(
    client: &Client,
    config: &CloudflareConfig,
    id: i32,
) -> Result<Option<series::Model>, String> {
    let env = d1_run(client, config, series::Entity::find_by_id(id)).await?;
    Ok(decode_rows::<series::Model>(env)?.into_iter().next())
}
