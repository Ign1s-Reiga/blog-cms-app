//! Searching what posts actually say, not only what they are called.
//!
//! The obstacle is not the matching, it is that the bodies are not reliably
//! here. `sync_posts_from_cloud` mirrors metadata only, so a post pulled down
//! and never opened has its Markdown in R2 and nowhere on this machine, and a
//! refresh can leave a cached body older than the post it belongs to.
//!
//! So a search over bodies has two answers to give, not one: what matched, and
//! what could not be looked at. Reporting only the first would let "no results"
//! stand for "not searched", which is the same mistake
//! [`crate::media_usage`] exists to avoid when it refuses to call an object
//! unused on the strength of posts nobody could read. The vocabulary is
//! borrowed from there deliberately — [`Unchecked`] already names these states,
//! and a second set of names for the same facts would only drift.
//!
//! Nothing here reaches the network. Filling the gaps is a separate, explicit
//! act: see `cache_bodies` in [`crate::commands`].

use sea_orm::DatabaseConnection;
use serde::Serialize;

use crate::db;
use crate::entities::post::Model as PostModel;
use crate::error::AppResult;
use crate::media_usage::Unchecked;

/// A post whose body the search could not read, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unsearched {
    pub id: i32,
    pub title: String,
    pub reason: Unchecked,
}

/// What a body search found, and what it could not see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct BodyMatches {
    /// Ids of posts whose cached Markdown contains the query.
    pub matched: Vec<i32>,
    /// Posts that were not searched. While this is non-empty, an absence of
    /// matches is not evidence of an absence of the text.
    pub unsearched: Vec<Unsearched>,
}

/// Case-insensitive substring match, the same shape the title and tag filters
/// use, so one query behaves the same way across all three.
///
/// Both sides are lowered. Lowering only the query is the bug that made a
/// `Cloudflare` tag unreachable however it was typed.
fn contains(haystack: &str, needle_lower: &str) -> bool {
    haystack.to_lowercase().contains(needle_lower)
}

/// Search the bodies this machine has, over the posts given.
///
/// The caller passes the posts rather than this reading them, because the
/// listing it already holds is the one the results have to line up with.
pub async fn search(
    app: &tauri::AppHandle,
    conn: &DatabaseConnection,
    posts: &[PostModel],
    query: &str,
) -> AppResult<BodyMatches> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Ok(BodyMatches::default());
    }

    let mut found = BodyMatches::default();
    for post in posts {
        // Not cached at all: pulled from the cloud and never opened. Nothing of
        // it can be read, and saying so is the whole point.
        let Some(body) = crate::revisions::cached_body(app, &post.slug).await else {
            found.unsearched.push(Unsearched {
                id: post.id,
                title: post.title.clone(),
                reason: Unchecked::BodyNotCached,
            });
            continue;
        };

        // Cached, and known to be behind the cloud's copy. What is here can
        // still match — and a match is a fact worth keeping — but a *miss* says
        // nothing about the version readers are being served.
        if db::body_is_stale(conn, &post.slug).await? {
            if contains(&body, &needle) {
                found.matched.push(post.id);
            } else {
                found.unsearched.push(Unsearched {
                    id: post.id,
                    title: post.title.clone(),
                    reason: Unchecked::BodyStale,
                });
            }
            continue;
        }

        if contains(&body, &needle) {
            found.matched.push(post.id);
        }
    }

    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_ignores_case_on_both_sides() {
        assert!(contains("The Cloudflare Worker", "cloudflare"));
        assert!(contains("the cloudflare worker", "CLOUDFLARE".to_lowercase().as_str()));
        assert!(!contains("nothing here", "cloudflare"));
    }

    #[test]
    fn a_match_can_be_a_substring_of_a_word() {
        // Deliberate: the title and tag filters do the same, and a search that
        // required whole words would disagree with the box above it.
        assert!(contains("serialisation", "serial"));
    }

    /// The distinction the whole module is for, as a shape rather than as prose:
    /// an empty `matched` with a non-empty `unsearched` is not "no results".
    #[test]
    fn nothing_found_and_nothing_searched_are_different_answers() {
        let nothing = BodyMatches::default();
        let blind = BodyMatches {
            matched: vec![],
            unsearched: vec![Unsearched {
                id: 1,
                title: "Pulled, never opened".to_string(),
                reason: Unchecked::BodyNotCached,
            }],
        };
        assert_ne!(nothing, blind);
        assert!(nothing.unsearched.is_empty());
        assert!(!blind.unsearched.is_empty());
    }
}
