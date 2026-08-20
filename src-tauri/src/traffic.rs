//! Readership, read from Cloudflare Web Analytics.
//!
//! [`crate::analytics`] reads what the *infrastructure* did — R2 operations, D1
//! queries, both account-scoped and both filtered to the bucket and database
//! the app is already configured with. Those are bills, not readers. This module
//! reads the other half: how many people opened which post.
//!
//! The dataset is `rumPageloadEventsAdaptiveGroups`, filtered by a Web Analytics
//! **site tag**. That is the one new piece of configuration, and it is picked
//! from the sites on the account rather than pasted — see [`list_web_analytics_sites`].
//!
//! No new token permission. RUM sits under the same `Account Analytics: Read`
//! that the dashboard's usage card already asks for, so a token that can draw
//! that card can draw this one, and a token that cannot fails the same way with
//! the same [`ErrorKind::Permission`].
//!
//! **Traffic is per path; posts are per slug.** Nothing in Cloudflare knows the
//! blog's URL shape, and this app deliberately does not ask for it: a path is
//! attributed to a post when its last segment is that post's slug, which holds
//! for `/posts/<slug>`, `/blog/<slug>` and `/<slug>` alike. What does not match
//! is *reported*, not discarded — an index page, a tag listing, or a post whose
//! URL does not end in its slug are all things worth seeing rather than
//! silently dropping on the floor.

use std::collections::HashMap;

use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::analytics::{AnalyticsError, ErrorKind};
use crate::cloudflare::CloudflareConfig;
use crate::entities::post::Model as PostModel;

const GRAPHQL_URL: &str = "https://api.cloudflare.com/client/v4/graphql";

// ─── Payloads ───────────────────────────────────────────────────────────────

/// A Web Analytics site on the account, as something to choose between.
#[derive(Clone, Debug, Serialize)]
pub struct Site {
    pub site_tag: String,
    /// The hostname it is installed on, when Cloudflare reports one. Falls back
    /// to the tag, which is at least unambiguous.
    pub name: String,
}

/// One day of one post's readership.
#[derive(Clone, Debug, Serialize)]
pub struct DailyViews {
    /// `YYYY-MM-DD`.
    pub date: String,
    pub views: u64,
}

/// What a post was read.
#[derive(Debug, Serialize)]
pub struct PostTraffic {
    pub id: i32,
    pub slug: String,
    pub title: String,
    pub published: bool,
    pub views: u64,
    pub visits: u64,
    /// One entry per day in the window, zeroes included, so a chart has a
    /// continuous axis rather than a gap where nobody visited.
    pub days: Vec<DailyViews>,
}

/// Traffic that could not be attributed to a post.
///
/// Kept and shown. An index page and a mistyped slug look identical in the
/// totals, and only one of them is worth doing something about.
#[derive(Debug, Serialize)]
pub struct UnattributedPath {
    pub path: String,
    pub views: u64,
}

#[derive(Debug, Serialize)]
pub struct TrafficReport {
    /// The date axis every post's `days` is aligned to.
    pub dates: Vec<String>,
    /// Ranked, most-read first.
    pub posts: Vec<PostTraffic>,
    /// Ranked, most-read first.
    pub unattributed: Vec<UnattributedPath>,
    pub total_views: u64,
    /// Views that reached a post, as opposed to the rest of the blog.
    pub attributed_views: u64,
}

// ─── Site listing (REST) ────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SiteListResponse {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    result: Vec<SiteInfo>,
    #[serde(default)]
    errors: Vec<ApiError>,
}

#[derive(Deserialize)]
struct ApiError {
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct SiteInfo {
    #[serde(default)]
    site_tag: String,
    #[serde(default)]
    ruleset: Option<Ruleset>,
    #[serde(default)]
    rules: Vec<Rule>,
}

#[derive(Deserialize)]
struct Ruleset {
    #[serde(default)]
    zone_name: Option<String>,
}

#[derive(Deserialize)]
struct Rule {
    #[serde(default)]
    host: Option<String>,
}

impl SiteInfo {
    /// The most recognisable name Cloudflare offers for this site.
    ///
    /// A zone-attached site carries its zone name; a manually installed one
    /// carries the hosts its rules match. Neither is guaranteed, so the tag is
    /// the last resort — unhelpful to read, but never wrong.
    fn display_name(&self) -> String {
        self.ruleset
            .as_ref()
            .and_then(|r| r.zone_name.clone())
            .filter(|n| !n.is_empty())
            .or_else(|| {
                self.rules
                    .iter()
                    .filter_map(|r| r.host.clone())
                    .find(|h| !h.is_empty() && h != "*")
            })
            .unwrap_or_else(|| self.site_tag.clone())
    }
}

fn http_client() -> Result<reqwest::Client, AnalyticsError> {
    // Same allowance as the usage card: a few kilobytes of JSON, with somebody
    // waiting in front of it. Without a bound a black-holed endpoint leaves the
    // request in flight indefinitely.
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| {
            AnalyticsError::new(ErrorKind::Network, format!("Could not create an HTTP client: {e}"))
        })
}

/// The Web Analytics sites on this account, to choose the blog from.
///
/// Offered as a list rather than a field to paste a tag into: a site tag is a
/// 32-character hex string that appears in no URL and means nothing on sight, so
/// asking for one typed correctly is asking for a silent misconfiguration.
#[tauri::command]
pub async fn list_web_analytics_sites() -> Result<Vec<Site>, AnalyticsError> {
    let config = crate::auth::get_creds().ok_or_else(|| {
        AnalyticsError::new(ErrorKind::NotConfigured, "Not signed in to Cloudflare")
    })?;

    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{}/rum/site_info/list",
        config.account_id
    );
    let response = http_client()?
        .get(&url)
        .header("Authorization", format!("Bearer {}", config.api_token))
        .send()
        .await
        .map_err(|e| {
            AnalyticsError::new(ErrorKind::Network, format!("Could not reach Cloudflare: {e}"))
        })?;

    let status = response.status();
    if status == 401 || status == 403 {
        return Err(AnalyticsError::new(
            ErrorKind::Permission,
            "Cloudflare rejected the token for Web Analytics",
        ));
    }

    let payload: SiteListResponse = response.json().await.map_err(|e| {
        AnalyticsError::new(ErrorKind::Query, format!("Unreadable site list: {e}"))
    })?;

    if !payload.success {
        let joined = payload
            .errors
            .iter()
            .map(|e| e.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        let message = if joined.is_empty() {
            format!("Cloudflare returned {status}")
        } else {
            joined
        };
        return Err(if crate::analytics::is_permission_error(&message) {
            AnalyticsError::new(ErrorKind::Permission, message)
        } else {
            AnalyticsError::new(ErrorKind::Query, message)
        });
    }

    Ok(payload
        .result
        .iter()
        .filter(|s| !s.site_tag.is_empty())
        .map(|s| Site { site_tag: s.site_tag.clone(), name: s.display_name() })
        .collect())
}

// ─── Traffic (GraphQL) ──────────────────────────────────────────────────────

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
    #[serde(default, rename = "rumPageloadEventsAdaptiveGroups")]
    pageloads: Vec<PageloadGroup>,
}

#[derive(Deserialize)]
struct PageloadGroup {
    #[serde(default)]
    count: Option<u64>,
    #[serde(default)]
    sum: Option<PageloadSum>,
    #[serde(default)]
    dimensions: Option<PageloadDimensions>,
}

#[derive(Deserialize)]
struct PageloadSum {
    #[serde(default)]
    visits: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageloadDimensions {
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    request_path: Option<String>,
}

/// Page loads by day and path.
///
/// `count` is page views and `sum { visits }` is sessions; both are reported
/// because they answer different questions — how often a post was opened, and
/// by how many arrivals.
const QUERY: &str = r#"
query PostTraffic($accountTag: string!, $siteTag: string!, $start: Date!, $end: Date!) {
  viewer {
    accounts(filter: { accountTag: $accountTag }) {
      rumPageloadEventsAdaptiveGroups(
        limit: 10000
        filter: { siteTag: $siteTag, date_geq: $start, date_leq: $end }
        orderBy: [date_ASC]
      ) {
        count
        sum { visits }
        dimensions { date requestPath }
      }
    }
  }
}
"#;

/// The slug a path would belong to: its last non-empty segment, with any query
/// string, fragment or `.html` taken off.
///
/// Deliberately lenient about the shape of everything before it. The blog owns
/// its URLs and may nest posts under anything; what it does not do is end a
/// post's URL in something other than the post's slug.
fn path_slug(path: &str) -> Option<&str> {
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let last = path.rsplit('/').find(|segment| !segment.is_empty())?;
    let last = last.strip_suffix(".html").unwrap_or(last);
    (!last.is_empty()).then_some(last)
}

/// Attribute grouped page loads to posts, and keep what did not match.
fn attribute(
    groups: &[PageloadGroup],
    posts: &[PostModel],
    dates: &[String],
) -> (Vec<PostTraffic>, Vec<UnattributedPath>, u64) {
    let by_slug: HashMap<&str, &PostModel> = posts.iter().map(|p| (p.slug.as_str(), p)).collect();

    // Per post: totals plus a day → views map, filled onto the full axis after.
    let mut per_post: HashMap<i32, (u64, u64, HashMap<String, u64>)> = HashMap::new();
    let mut per_path: HashMap<String, u64> = HashMap::new();
    let mut total = 0u64;

    for group in groups {
        let Some(dims) = group.dimensions.as_ref() else { continue };
        let Some(path) = dims.request_path.as_deref() else { continue };
        let views = group.count.unwrap_or(0);
        let visits = group.sum.as_ref().and_then(|s| s.visits).unwrap_or(0);
        total += views;

        match path_slug(path).and_then(|slug| by_slug.get(slug)) {
            Some(post) => {
                let entry = per_post.entry(post.id).or_insert_with(|| (0, 0, HashMap::new()));
                entry.0 += views;
                entry.1 += visits;
                if let Some(date) = dims.date.as_deref() {
                    *entry.2.entry(date.to_string()).or_insert(0) += views;
                }
            }
            // Two paths can land on the same post; two unmatched paths stay
            // separate, because the path is the only thing identifying them.
            None => *per_path.entry(path.to_string()).or_insert(0) += views,
        }
    }

    let mut ranked: Vec<PostTraffic> = per_post
        .into_iter()
        .filter_map(|(id, (views, visits, by_date))| {
            let post = posts.iter().find(|p| p.id == id)?;
            Some(PostTraffic {
                id,
                slug: post.slug.clone(),
                title: post.title.clone(),
                published: post.published,
                views,
                visits,
                days: dates
                    .iter()
                    .map(|date| DailyViews {
                        date: date.clone(),
                        views: by_date.get(date).copied().unwrap_or(0),
                    })
                    .collect(),
            })
        })
        .collect();
    // Ties broken by slug so the order does not shuffle between reads of the
    // same numbers.
    ranked.sort_by(|a, b| b.views.cmp(&a.views).then_with(|| a.slug.cmp(&b.slug)));

    let mut rest: Vec<UnattributedPath> = per_path
        .into_iter()
        .map(|(path, views)| UnattributedPath { path, views })
        .collect();
    rest.sort_by(|a, b| b.views.cmp(&a.views).then_with(|| a.path.cmp(&b.path)));

    (ranked, rest, total)
}

/// Read the last `days` of readership and attribute it to posts.
#[tauri::command]
pub async fn fetch_post_traffic(
    conn: State<'_, DatabaseConnection>,
    days: Option<u32>,
) -> Result<TrafficReport, AnalyticsError> {
    let config = crate::auth::get_creds().ok_or_else(|| {
        AnalyticsError::new(ErrorKind::NotConfigured, "Not signed in to Cloudflare")
    })?;
    if config.web_analytics_site_tag.trim().is_empty() {
        return Err(AnalyticsError::new(
            ErrorKind::NotConfigured,
            "No Web Analytics site is selected, so there is nothing to read readership from.",
        ));
    }

    // The local library, which is what the paths are matched against. Read
    // before the network call so a failure here is not mistaken for one there.
    let posts = crate::db::list::<PostModel>(conn.inner()).await.map_err(|e| {
        AnalyticsError::new(ErrorKind::Local, format!("Could not read the local posts: {e}"))
    })?;

    let days = days.unwrap_or(7).max(1);
    let end = chrono::Utc::now().date_naive();
    let start = end - chrono::Duration::days((days - 1) as i64);
    let dates: Vec<String> = (0..days)
        .map(|offset| (start + chrono::Duration::days(offset as i64)).to_string())
        .collect();

    let body = serde_json::json!({
        "query": QUERY,
        "variables": {
            "accountTag": config.account_id,
            "siteTag": config.web_analytics_site_tag.trim(),
            "start": start.to_string(),
            "end": end.to_string(),
        }
    });

    let groups = request(&config, body).await?;
    let (posts, unattributed, total_views) = attribute(&groups, &posts, &dates);
    let attributed_views = posts.iter().map(|p| p.views).sum();

    Ok(TrafficReport { dates, posts, unattributed, total_views, attributed_views })
}

/// Send the query and unwrap the account out of the response.
///
/// GraphQL answers a rejected query with HTTP 200 and an `errors` array, so a
/// naive caller sees success and no data. Every response is inspected.
async fn request(
    config: &CloudflareConfig,
    body: serde_json::Value,
) -> Result<Vec<PageloadGroup>, AnalyticsError> {
    let response = http_client()?
        .post(GRAPHQL_URL)
        .header("Authorization", format!("Bearer {}", config.api_token))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            AnalyticsError::new(ErrorKind::Network, format!("Could not reach Cloudflare: {e}"))
        })?;

    let status = response.status();
    let payload: GraphQlResponse = response.json().await.map_err(|e| {
        AnalyticsError::new(ErrorKind::Query, format!("Unreadable traffic response: {e}"))
    })?;

    if let Some(errors) = payload.errors.filter(|e| !e.is_empty()) {
        let joined = errors.iter().map(|e| e.message.as_str()).collect::<Vec<_>>().join("; ");
        return Err(
            if status == 403 || status == 401 || crate::analytics::is_permission_error(&joined) {
                AnalyticsError::new(ErrorKind::Permission, joined)
            } else {
                AnalyticsError::new(ErrorKind::Query, joined)
            },
        );
    }
    if status == 403 || status == 401 {
        return Err(AnalyticsError::new(
            ErrorKind::Permission,
            "Cloudflare rejected the token for Web Analytics",
        ));
    }
    if !status.is_success() {
        return Err(AnalyticsError::new(ErrorKind::Query, format!("Cloudflare returned {status}")));
    }

    Ok(payload
        .data
        .and_then(|d| d.viewer)
        .map(|v| v.accounts)
        .and_then(|mut a| if a.is_empty() { None } else { Some(a.remove(0)) })
        .ok_or_else(|| {
            AnalyticsError::new(ErrorKind::Query, "Cloudflare returned no account data")
        })?
        .pageloads)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(id: i32, slug: &str) -> PostModel {
        PostModel {
            id,
            slug: slug.to_string(),
            title: format!("Post {slug}"),
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

    fn group(date: &str, path: &str, views: u64, visits: u64) -> PageloadGroup {
        PageloadGroup {
            count: Some(views),
            sum: Some(PageloadSum { visits: Some(visits) }),
            dimensions: Some(PageloadDimensions {
                date: Some(date.to_string()),
                request_path: Some(path.to_string()),
            }),
        }
    }

    /// The blog owns its URL shape and this app does not ask for it, so every
    /// nesting has to land on the same slug.
    #[test]
    fn a_slug_is_read_out_of_any_url_shape() {
        for path in ["/posts/my-post", "/blog/my-post", "/my-post", "/a/b/c/my-post"] {
            assert_eq!(path_slug(path), Some("my-post"), "{path}");
        }
        // Trailing slash, extension, query and fragment are all noise here.
        assert_eq!(path_slug("/posts/my-post/"), Some("my-post"));
        assert_eq!(path_slug("/posts/my-post.html"), Some("my-post"));
        assert_eq!(path_slug("/posts/my-post?utm_source=x"), Some("my-post"));
        assert_eq!(path_slug("/posts/my-post#section"), Some("my-post"));
        // The site root belongs to no post.
        assert_eq!(path_slug("/"), None);
        assert_eq!(path_slug(""), None);
    }

    #[test]
    fn views_land_on_the_post_whose_slug_the_path_ends_in() {
        let posts = vec![post(1, "first"), post(2, "second")];
        let dates = vec!["2026-08-19".to_string(), "2026-08-20".to_string()];
        let groups = vec![
            group("2026-08-19", "/posts/first", 10, 8),
            group("2026-08-20", "/posts/first", 5, 4),
            group("2026-08-20", "/posts/second", 3, 3),
        ];

        let (ranked, rest, total) = attribute(&groups, &posts, &dates);

        assert_eq!(total, 18);
        assert!(rest.is_empty());
        assert_eq!(ranked.len(), 2);
        // Ranked by views, so the most-read post is first.
        assert_eq!(ranked[0].slug, "first");
        assert_eq!(ranked[0].views, 15);
        assert_eq!(ranked[0].visits, 12);
        // The per-day series covers the axis, including days with nothing on
        // them for that post.
        assert_eq!(ranked[1].slug, "second");
        assert_eq!(
            ranked[1].days.iter().map(|d| d.views).collect::<Vec<_>>(),
            vec![0, 3]
        );
    }

    /// The half that would otherwise round itself up to "no traffic".
    #[test]
    fn what_matches_no_post_is_reported_rather_than_dropped() {
        let posts = vec![post(1, "first")];
        let dates = vec!["2026-08-20".to_string()];
        let groups = vec![
            group("2026-08-20", "/posts/first", 4, 4),
            group("2026-08-20", "/", 30, 25),
            group("2026-08-20", "/tags/rust", 7, 6),
            group("2026-08-20", "/posts/deleted-post", 2, 2),
        ];

        let (ranked, rest, total) = attribute(&groups, &posts, &dates);

        assert_eq!(total, 43);
        assert_eq!(ranked.iter().map(|p| p.views).sum::<u64>(), 4);
        // Ranked, so the busiest unattributed path is the one worth looking at.
        assert_eq!(rest.len(), 3);
        assert_eq!(rest[0].path, "/");
        assert_eq!(rest[0].views, 30);
        assert!(rest.iter().any(|u| u.path == "/posts/deleted-post"));
    }

    /// Two URLs for one post are one post, not two rows that each look small.
    #[test]
    fn several_paths_for_one_post_add_up() {
        let posts = vec![post(1, "same")];
        let dates = vec!["2026-08-20".to_string()];
        let groups = vec![
            group("2026-08-20", "/posts/same", 4, 4),
            group("2026-08-20", "/posts/same/", 6, 5),
        ];

        let (ranked, rest, _) = attribute(&groups, &posts, &dates);
        assert!(rest.is_empty());
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].views, 10);
        assert_eq!(ranked[0].days[0].views, 10);
    }

    #[test]
    fn a_site_is_named_by_its_zone_then_its_rules_then_its_tag() {
        let zone = SiteInfo {
            site_tag: "abc".into(),
            ruleset: Some(Ruleset { zone_name: Some("example.com".into()) }),
            rules: vec![],
        };
        assert_eq!(zone.display_name(), "example.com");

        let by_rule = SiteInfo {
            site_tag: "abc".into(),
            ruleset: Some(Ruleset { zone_name: None }),
            rules: vec![Rule { host: Some("*".into()) }, Rule { host: Some("blog.example.com".into()) }],
        };
        assert_eq!(by_rule.display_name(), "blog.example.com");

        let bare = SiteInfo { site_tag: "abc".into(), ruleset: None, rules: vec![] };
        assert_eq!(bare.display_name(), "abc");
    }
}
