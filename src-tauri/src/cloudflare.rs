use reqwest::Client;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ColumnTrait, DbBackend, EntityTrait, QueryFilter, QueryOrder, QueryTrait, Value, Values,
};
use serde::Deserialize;

use crate::entities::post;

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
// HTTP `/query` endpoint. The `posts` table must exist in D1 with the same
// columns as `entities::post::Model`.

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
}

#[derive(Deserialize)]
struct D1Error {
    message: String,
}

/// Convert a built statement's bound values into D1's JSON `params` array. Every
/// `posts` column is TEXT, so each value is a string (empty optionals → null).
fn params_json(values: Option<Values>) -> Vec<serde_json::Value> {
    values
        .map(|vs| {
            vs.0.into_iter()
                .map(|v| match v {
                    Value::String(Some(s)) => serde_json::Value::String(s.to_string()),
                    _ => serde_json::Value::Null,
                })
                .collect()
        })
        .unwrap_or_default()
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

/// Decode the first result set's rows into `Post` models.
fn decode_rows(env: D1Envelope) -> Result<Vec<post::Model>, String> {
    let rows = env
        .result
        .into_iter()
        .next()
        .map(|r| r.results)
        .unwrap_or_default();
    rows.into_iter()
        .map(|row| {
            serde_json::from_value::<post::Model>(row).map_err(|e| format!("D1 row decode error: {e}"))
        })
        .collect()
}

// ── CRUD ─────────────────────────────────────────────────────────────────────

pub async fn d1_insert(
    client: &Client,
    config: &CloudflareConfig,
    model: post::Model,
) -> Result<(), String> {
    d1_run(client, config, post::Entity::insert(model.into_active_set()))
        .await
        .map(|_| ())
}

pub async fn d1_update(
    client: &Client,
    config: &CloudflareConfig,
    model: post::Model,
) -> Result<(), String> {
    // UpdateOne isn't a `QueryTrait`, so build an UPDATE … WHERE id = ? by hand.
    let stmt = post::Entity::update_many()
        .col_expr(post::Column::Title, Expr::value(model.title))
        .col_expr(post::Column::Status, Expr::value(model.status))
        .col_expr(post::Column::Tags, Expr::value(model.tags))
        .col_expr(post::Column::R2Key, Expr::value(model.r2_key))
        .col_expr(post::Column::UploadDate, Expr::value(model.upload_date))
        .col_expr(post::Column::LastUpdatedDate, Expr::value(model.last_updated_date))
        .filter(post::Column::Id.eq(model.id));
    d1_run(client, config, stmt).await.map(|_| ())
}

pub async fn d1_delete(
    client: &Client,
    config: &CloudflareConfig,
    id: String,
) -> Result<(), String> {
    d1_run(client, config, post::Entity::delete_by_id(id))
        .await
        .map(|_| ())
}

pub async fn d1_list(
    client: &Client,
    config: &CloudflareConfig,
) -> Result<Vec<post::Model>, String> {
    let env = d1_run(
        client,
        config,
        post::Entity::find().order_by_desc(post::Column::LastUpdatedDate),
    )
    .await?;
    decode_rows(env)
}

pub async fn d1_get(
    client: &Client,
    config: &CloudflareConfig,
    id: String,
) -> Result<Option<post::Model>, String> {
    let env = d1_run(client, config, post::Entity::find_by_id(id)).await?;
    Ok(decode_rows(env)?.into_iter().next())
}
