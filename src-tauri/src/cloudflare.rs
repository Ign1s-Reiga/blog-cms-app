use reqwest::Client;
use serde::Deserialize;

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

/// Minimal subset of the D1 query response needed to detect failures.
#[derive(Deserialize)]
struct D1Response {
    success: bool,
    #[serde(default)]
    errors: Vec<D1Error>,
}

#[derive(Deserialize)]
struct D1Error {
    message: String,
}

/// Insert a post metadata record into the D1 `posts` table.
///
/// Expected schema:
/// ```sql
/// CREATE TABLE posts (
///   id                TEXT PRIMARY KEY,
///   title             TEXT NOT NULL,
///   upload_date       TEXT NOT NULL,
///   last_updated_date TEXT NOT NULL,
///   tags              TEXT NOT NULL DEFAULT ''
/// );
/// ```
pub async fn insert_d1_record(
    client: &Client,
    config: &CloudflareConfig,
    id: &str,
    title: &str,
    upload_date: &str,
    last_updated_date: &str,
    tags: &str,
) -> Result<(), String> {
    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/d1/database/{}/query",
        config.account_id, config.d1_database_id
    );

    let body = serde_json::json!({
        "sql": "INSERT INTO posts (id, title, upload_date, last_updated_date, tags) \
                VALUES (?1, ?2, ?3, ?4, ?5)",
        "params": [id, title, upload_date, last_updated_date, tags]
    });

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

    // Parse the Cloudflare wrapper to surface any logical errors
    match serde_json::from_str::<D1Response>(&text) {
        Ok(resp) if !resp.success => {
            let msg = resp
                .errors
                .first()
                .map(|e| e.message.as_str())
                .unwrap_or("unknown D1 error");
            Err(format!("D1 insert failed: {msg}"))
        }
        Ok(_) => Ok(()),
        Err(e) => Err(format!("D1 response parse error: {e}")),
    }
}
