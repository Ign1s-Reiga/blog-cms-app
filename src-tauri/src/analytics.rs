//! Basic usage analytics, read from Cloudflare's GraphQL Analytics API.
//!
//! Two account-scoped datasets, filtered to the bucket and database the app is
//! already configured with, so no extra settings are needed:
//!
//! - `r2OperationsAdaptiveGroups` — requests against the R2 bucket
//! - `d1AnalyticsAdaptiveGroups`  — queries against the D1 database
//!
//! **These need a token permission the app does not otherwise use.** The sign-in
//! screen asks for an "R2 + D1 edit" token; reading analytics additionally needs
//! `Account Analytics: Read`. A token without it is perfectly valid for
//! everything else, so this failure is expected rather than exceptional — it is
//! reported as its own [`ErrorKind::Permission`] so the UI can say plainly what
//! is missing instead of showing an empty chart.
//!
//! GraphQL answers a rejected query with HTTP 200 and an `errors` array, so a
//! naive caller sees success and no data. Every response is inspected here.

use serde::{Deserialize, Serialize};

use crate::cloudflare::CloudflareConfig;

const GRAPHQL_URL: &str = "https://api.cloudflare.com/client/v4/graphql";

/// One day of usage. Days with no activity are still present, with zeroes, so
/// the chart shows a continuous axis rather than a gap.
#[derive(Clone, Debug, Serialize)]
pub struct DailyUsage {
    /// `YYYY-MM-DD`.
    pub date: String,
    pub r2_requests: u64,
    pub d1_queries: u64,
}

#[derive(Serialize)]
pub struct Analytics {
    pub days: Vec<DailyUsage>,
    pub r2_total: u64,
    pub d1_total: u64,
}

/// Why analytics could not be read. `kind` is what the UI switches on; the
/// message is for display underneath.
#[derive(Serialize)]
pub struct AnalyticsError {
    pub kind: ErrorKind,
    pub message: String,
}

#[derive(Serialize, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    /// The token is valid but lacks `Account Analytics: Read`.
    Permission,
    /// Not signed in.
    NotConfigured,
    /// The request never completed — offline, DNS, TLS.
    Network,
    /// The API answered, but not with data we could use. A schema drift on
    /// Cloudflare's side lands here rather than being mistaken for "no data".
    Query,
}

impl AnalyticsError {
    fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }
}

// ─── GraphQL wire types ─────────────────────────────────────────────────────

#[derive(Deserialize)]
struct GraphQlResponse {
    #[serde(default)]
    data: Option<ResponseData>,
    #[serde(default)]
    errors: Option<Vec<GraphQlError>>,
}

#[derive(Deserialize)]
struct GraphQlError {
    message: String,
}

#[derive(Deserialize)]
struct ResponseData {
    viewer: Option<Viewer>,
}

#[derive(Deserialize)]
struct Viewer {
    #[serde(default)]
    accounts: Vec<Account>,
}

#[derive(Deserialize)]
struct Account {
    #[serde(default, rename = "r2OperationsAdaptiveGroups")]
    r2: Vec<Group>,
    #[serde(default, rename = "d1AnalyticsAdaptiveGroups")]
    d1: Vec<Group>,
}

#[derive(Deserialize)]
struct Group {
    #[serde(default)]
    sum: Option<GroupSum>,
    #[serde(default)]
    dimensions: Option<Dimensions>,
}

/// GraphQL returns `readQueries` / `writeQueries`; the rename keeps the Rust
/// side snake_case without a field-by-field annotation.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GroupSum {
    #[serde(default)]
    requests: Option<u64>,
    #[serde(default)]
    read_queries: Option<u64>,
    #[serde(default)]
    write_queries: Option<u64>,
}

#[derive(Deserialize)]
struct Dimensions {
    #[serde(default)]
    date: Option<String>,
}

// ─── Query ──────────────────────────────────────────────────────────────────

/// Both datasets in one round trip. Grouped by `date`, which every
/// `*AdaptiveGroups` dataset exposes, so the result is already a daily series.
const QUERY: &str = r#"
query BasicUsage($accountTag: string!, $start: Date!, $end: Date!, $bucket: string!, $database: string!) {
  viewer {
    accounts(filter: { accountTag: $accountTag }) {
      r2OperationsAdaptiveGroups(
        limit: 10000
        filter: { date_geq: $start, date_leq: $end, bucketName: $bucket }
        orderBy: [date_ASC]
      ) {
        sum { requests }
        dimensions { date }
      }
      d1AnalyticsAdaptiveGroups(
        limit: 10000
        filter: { date_geq: $start, date_leq: $end, databaseId: $database }
        orderBy: [date_ASC]
      ) {
        sum { readQueries writeQueries }
        dimensions { date }
      }
    }
  }
}
"#;

/// True when a GraphQL error is Cloudflare telling us the token is not allowed
/// to read this, rather than the query being wrong.
///
/// Cloudflare phrases this several ways across datasets, so this matches on the
/// recognisable fragments instead of one exact string.
fn is_permission_error(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    [
        "not authorized",
        "unauthorized",
        "permission",
        "forbidden",
        "access denied",
        "not entitled",
        "authentication error",
    ]
    .iter()
    .any(|needle| m.contains(needle))
}

async fn query(config: &CloudflareConfig, days: u32) -> Result<Analytics, AnalyticsError> {
    let end = chrono::Utc::now().date_naive();
    // `days` inclusive of today, so 7 spans today and the six before it.
    let start = end - chrono::Duration::days((days.max(1) - 1) as i64);

    let body = serde_json::json!({
        "query": QUERY,
        "variables": {
            "accountTag": config.account_id,
            "start": start.to_string(),
            "end": end.to_string(),
            "bucket": config.r2_bucket,
            "database": config.d1_database_id,
        }
    });

    let client = reqwest::Client::new();
    let response = client
        .post(GRAPHQL_URL)
        .header("Authorization", format!("Bearer {}", config.api_token))
        .json(&body)
        .send()
        .await
        .map_err(|e| AnalyticsError::new(ErrorKind::Network, format!("Could not reach Cloudflare: {e}")))?;

    let status = response.status();
    let payload: GraphQlResponse = response.json().await.map_err(|e| {
        AnalyticsError::new(ErrorKind::Query, format!("Unreadable analytics response: {e}"))
    })?;

    // A rejected token can surface either as an HTTP status or as a 200 with
    // errors, depending on where it is refused.
    if let Some(errors) = payload.errors.filter(|e| !e.is_empty()) {
        let joined = errors.iter().map(|e| e.message.as_str()).collect::<Vec<_>>().join("; ");
        return Err(if status == 403 || status == 401 || is_permission_error(&joined) {
            AnalyticsError::new(ErrorKind::Permission, joined)
        } else {
            AnalyticsError::new(ErrorKind::Query, joined)
        });
    }
    if status == 403 || status == 401 {
        return Err(AnalyticsError::new(
            ErrorKind::Permission,
            "Cloudflare rejected the token for analytics",
        ));
    }
    if !status.is_success() {
        return Err(AnalyticsError::new(ErrorKind::Query, format!("Cloudflare returned {status}")));
    }

    let account = payload
        .data
        .and_then(|d| d.viewer)
        .map(|v| v.accounts)
        .and_then(|mut a| if a.is_empty() { None } else { Some(a.remove(0)) })
        .ok_or_else(|| {
            AnalyticsError::new(ErrorKind::Query, "Cloudflare returned no account data")
        })?;

    Ok(assemble(&account, start, days.max(1)))
}

/// Fold the two grouped result sets onto one continuous run of days.
fn assemble(account: &Account, start: chrono::NaiveDate, days: u32) -> Analytics {
    let mut series: Vec<DailyUsage> = (0..days)
        .map(|offset| DailyUsage {
            date: (start + chrono::Duration::days(offset as i64)).to_string(),
            r2_requests: 0,
            d1_queries: 0,
        })
        .collect();

    let find = |series: &mut Vec<DailyUsage>, date: &str| -> Option<usize> {
        series.iter().position(|d| d.date == date)
    };

    for group in &account.r2 {
        let (Some(date), Some(sum)) = (group.dimensions.as_ref().and_then(|d| d.date.as_deref()), group.sum.as_ref())
        else {
            continue;
        };
        if let Some(i) = find(&mut series, date) {
            series[i].r2_requests += sum.requests.unwrap_or(0);
        }
    }

    for group in &account.d1 {
        let (Some(date), Some(sum)) = (group.dimensions.as_ref().and_then(|d| d.date.as_deref()), group.sum.as_ref())
        else {
            continue;
        };
        if let Some(i) = find(&mut series, date) {
            series[i].d1_queries += sum.read_queries.unwrap_or(0) + sum.write_queries.unwrap_or(0);
        }
    }

    Analytics {
        r2_total: series.iter().map(|d| d.r2_requests).sum(),
        d1_total: series.iter().map(|d| d.d1_queries).sum(),
        days: series,
    }
}

// ─── Command ────────────────────────────────────────────────────────────────

/// Read the last `days` (default 7) of R2 and D1 usage.
///
/// Called on every dashboard mount, so it is a plain fetch with no caching —
/// the numbers should reflect the moment the page was opened.
#[tauri::command]
pub async fn fetch_analytics(days: Option<u32>) -> Result<Analytics, AnalyticsError> {
    let config = crate::auth::get_creds().ok_or_else(|| {
        AnalyticsError::new(ErrorKind::NotConfigured, "Not signed in to Cloudflare")
    })?;
    query(&config, days.unwrap_or(7)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_errors_are_told_apart_from_query_errors() {
        for msg in [
            "You do not have permission to access this resource",
            "Unauthorized to access requested resource",
            "authentication error",
            "Access denied for dataset",
        ] {
            assert!(is_permission_error(msg), "should be a permission error: {msg}");
        }
        for msg in [
            "Unknown field 'wobble' on type 'Account'",
            "limit must be less than 10000",
        ] {
            assert!(!is_permission_error(msg), "should not be a permission error: {msg}");
        }
    }

    #[test]
    fn series_covers_every_day_even_with_sparse_results() {
        let account = Account {
            r2: vec![Group {
                sum: Some(GroupSum { requests: Some(5), read_queries: None, write_queries: None }),
                dimensions: Some(Dimensions { date: Some("2026-08-02".into()) }),
            }],
            d1: vec![Group {
                sum: Some(GroupSum { requests: None, read_queries: Some(3), write_queries: Some(4) }),
                dimensions: Some(Dimensions { date: Some("2026-08-03".into()) }),
            }],
        };
        let start = chrono::NaiveDate::from_ymd_opt(2026, 8, 1).unwrap();
        let out = assemble(&account, start, 3);

        assert_eq!(out.days.len(), 3, "quiet days must still appear");
        assert_eq!(out.days[0].r2_requests, 0);
        assert_eq!(out.days[1].r2_requests, 5);
        // Read and write queries are summed into one D1 figure.
        assert_eq!(out.days[2].d1_queries, 7);
        assert_eq!(out.r2_total, 5);
        assert_eq!(out.d1_total, 7);
    }

    /// A day Cloudflare reports outside the requested window must not be
    /// silently folded into an unrelated day.
    #[test]
    fn out_of_range_days_are_ignored() {
        let account = Account {
            r2: vec![Group {
                sum: Some(GroupSum { requests: Some(99), read_queries: None, write_queries: None }),
                dimensions: Some(Dimensions { date: Some("1999-01-01".into()) }),
            }],
            d1: vec![],
        };
        let start = chrono::NaiveDate::from_ymd_opt(2026, 8, 1).unwrap();
        let out = assemble(&account, start, 3);
        assert_eq!(out.r2_total, 0);
    }
}
