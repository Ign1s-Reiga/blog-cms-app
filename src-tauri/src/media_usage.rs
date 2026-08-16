//! Which posts still depend on a media object.
//!
//! The media library is a pool of reusable images under `media/`, and deleting
//! from it used to be a decision made blind: nothing said whether a post would
//! be left with a broken image, and a published post's images are served to
//! readers rather than to the person pressing delete.
//!
//! ## Why this cannot be a text search
//!
//! A post never mentions a library key. Inserting a library image *copies* it
//! into the post's own staging area under a fresh name
//! (`stage_media_from_library` → `assets/<uuid>.<ext>`), and publishing copies it
//! again into `posts/<slug>/<sha256>.<ext>` and writes that absolute URL into the
//! Markdown. Searching bodies for `media/<uuid>.avif` finds nothing, ever.
//!
//! What survives both copies is the bytes. Body images are already
//! content-addressed — that is what makes publishing idempotent — so the sha256
//! in a published URL *is* the image's identity, and a staged asset can be
//! hashed to get the same answer. Usage is therefore matched by content:
//!
//! ```text
//! media/3f2b….avif ──hash──> 9c1e…
//!                                  └──> assets/7d0a….avif        (hashed here)
//!                                  └──> …/posts/my-post/9c1e….avif (read from the URL)
//! ```
//!
//! Two library objects holding identical bytes are, by this reckoning, the same
//! image — which is exactly right for the question being asked, since deleting
//! either leaves the other serving the same picture.
//!
//! ## Derived, never indexed
//!
//! The answer is computed from the posts themselves each time it is asked for.
//! That is what the issue asked for, and it is also what makes it correct for
//! free: editing a post, restoring an old revision, trashing one or deleting it
//! for good all change the bodies on disk, and nothing has to remember to update
//! a table it does not know about. A blog's worth of posts and images is small
//! enough that hashing them is not worth avoiding.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use sea_orm::DatabaseConnection;
use tauri::Manager;

use crate::db;
use crate::entities::post::Model as PostModel;
use crate::error::{AppError, AppResult};
use crate::media_keys;

/// A post that still depends on a media object.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UsingPost {
    pub id: i32,
    pub slug: String,
    pub title: String,
    /// Whether the post is in the trash. Still a reference — a trashed post can
    /// be restored, and restoring it to a broken image would be a poor trade —
    /// but one the media view names separately so a warning about it can be read
    /// for what it is.
    pub trashed: bool,
    /// Whether the post is live on the blog. A reference from a published post
    /// is the one that is actually being served to readers.
    pub published: bool,
}

/// One media object and everything that references it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MediaUsage {
    /// R2 key, e.g. `media/3f2b….avif`.
    pub key: String,
    pub posts: Vec<UsingPost>,
}

/// Every `assets/<file>` name a body mentions.
///
/// Deliberately the same loose scan `commands::r2` uses on the publish path: it
/// is looking at the same references, and a stricter parser here would report a
/// post as unaffected by a deletion that the publish path would then go looking
/// for.
fn asset_names(body: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut rest = body;
    while let Some(pos) = rest.find("assets/") {
        let after = &rest[pos + "assets/".len()..];
        let end = after
            .find(|c: char| c == ')' || c == ']' || c == '"' || c == '\'' || c.is_whitespace())
            .unwrap_or(after.len());
        let name = &after[..end];
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
        rest = &after[end..];
    }
    names
}

/// Every sha256 that appears in a published image URL in this body.
///
/// Read out of the key rather than reconstructed, because the key's *layout* is
/// configurable (`media_key_pattern`) and may have changed since the post was
/// published — while the hash in it is the one thing that cannot have. Any
/// 64-character run of hex is taken as one: nothing else in a Markdown body
/// looks like that, and a false positive can only match a library object whose
/// bytes really do hash to it.
fn published_digests(body: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut run = String::new();

    // A trailing flush after the loop would repeat this block; pushing a
    // sentinel that cannot be hex ends the last run instead.
    for ch in body.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_hexdigit() {
            run.push(ch.to_ascii_lowercase());
            continue;
        }
        if run.len() == 64 && !found.contains(&run) {
            found.push(run.clone());
        }
        run.clear();
    }
    found
}

/// Hash every cached media object, giving the digest each library key stands
/// for.
///
/// Only what is cached locally: the media page caches every object it lists, so
/// in practice this is the library, and reaching to R2 for the rest would put
/// the network inside a question about local files. An object that is somehow
/// not cached reports no usage, which the media view shows as "usage unknown"
/// rather than as "unused" — the difference matters, because one of those
/// readings invites a delete.
async fn library_digests(media_dir: &Path) -> HashMap<String, String> {
    let mut digests = HashMap::new();
    let Ok(mut entries) = tokio::fs::read_dir(media_dir).await else {
        return digests;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Ok(bytes) = tokio::fs::read(entry.path()).await {
            digests.insert(format!("media/{name}"), media_keys::content_digest(&bytes));
        }
    }
    digests
}

/// The digests one post's body depends on, staged and published alike.
async fn digests_used_by(app: &tauri::AppHandle, post: &PostModel) -> AppResult<HashSet<String>> {
    let Some(body) = crate::revisions::cached_body(app, &post.slug).await else {
        // Nothing cached locally means nothing to read; the body is in R2 and
        // fetching it here would make listing the media library a network
        // operation. A post nobody has opened on this machine contributes no
        // references, which the media view reports as such.
        return Ok(HashSet::new());
    };

    let assets_dir = app
        .path()
        .app_data_dir()
        .map_err(AppError::AppDataDir)?
        .join("assets");

    let mut digests: HashSet<String> = published_digests(&body).into_iter().collect();
    for name in asset_names(&body) {
        // The same guard the publish path uses: these names reach the
        // filesystem, and a body is not necessarily something a human typed.
        if !is_plain_name(name) {
            continue;
        }
        if let Ok(bytes) = tokio::fs::read(assets_dir.join(name)).await {
            digests.insert(media_keys::content_digest(&bytes));
        }
    }
    Ok(digests)
}

/// One ordinary file name and nothing else — no separators, no traversal.
fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && name != "."
        && name != ".."
        && !name.contains(':')
}

/// Every media object in the local library, with the posts that reference it.
///
/// Objects with no references are included with an empty list: "used by nothing"
/// is the answer the media view most needs to show, and leaving those rows out
/// would make it indistinguishable from "not asked about".
pub async fn survey(
    app: &tauri::AppHandle,
    conn: &DatabaseConnection,
) -> AppResult<Vec<MediaUsage>> {
    let media_dir = app
        .path()
        .app_data_dir()
        .map_err(AppError::AppDataDir)?
        .join("media");
    let library = library_digests(&media_dir).await;

    // Trashed posts count, and are marked. They still hold the reference, and a
    // restore that brought back a post with a hole in it would be a poor trade
    // for a delete made on the strength of "nothing uses this".
    let trashed = db::trashed_ids(conn).await?;
    let posts = db::list::<PostModel>(conn).await?;

    let mut by_digest: HashMap<String, Vec<UsingPost>> = HashMap::new();
    for post in posts {
        let using = UsingPost {
            id: post.id,
            slug: post.slug.clone(),
            title: post.title.clone(),
            trashed: trashed.contains(&post.id),
            published: post.published,
        };
        for digest in digests_used_by(app, &post).await? {
            by_digest.entry(digest).or_default().push(using.clone());
        }
    }

    let mut usage: Vec<MediaUsage> = library
        .into_iter()
        .map(|(key, digest)| MediaUsage {
            posts: by_digest.get(&digest).cloned().unwrap_or_default(),
            key,
        })
        .collect();
    usage.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(usage)
}

/// The posts referencing one object, for the check that guards a deletion.
pub async fn users_of(
    app: &tauri::AppHandle,
    conn: &DatabaseConnection,
    key: &str,
) -> AppResult<Vec<UsingPost>> {
    Ok(survey(app, conn)
        .await?
        .into_iter()
        .find(|u| u.key == key)
        .map(|u| u.posts)
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published half: a body carries absolute URLs, and the hash in them is
    /// the image's identity however the key is laid out around it.
    #[test]
    fn digests_are_read_out_of_published_urls() {
        let digest = "a".repeat(64);
        let body = format!(
            "Text\n\n![pic](https://cdn.example.com/posts/my-post/{digest}.avif)\n\nMore text.\n"
        );
        assert_eq!(published_digests(&body), vec![digest]);
    }

    /// A different layout is still readable — which is the point of taking the
    /// hash from the text rather than rebuilding the key from the pattern.
    #[test]
    fn the_key_layout_around_the_hash_does_not_matter() {
        let digest = "b".repeat(64);
        for url in [
            format!("https://cdn.example.com/img/{digest}.png"),
            format!("https://cdn.example.com/2026/{digest}/full.webp"),
        ] {
            assert_eq!(published_digests(&url), vec![digest.clone()]);
        }
    }

    /// Ordinary prose does not contain 64-character hex runs, and near misses
    /// must not be mistaken for one.
    #[test]
    fn nothing_else_reads_as_a_digest() {
        assert!(published_digests("a normal post about deadbeef and cafe").is_empty());
        assert!(published_digests(&"c".repeat(63)).is_empty());
        // Longer than a digest is not a digest either — it is something else.
        assert!(published_digests(&"d".repeat(65)).is_empty());
        // Two in one body, both found.
        let (x, y) = ("e".repeat(64), "f".repeat(64));
        assert_eq!(published_digests(&format!("{x} and {y}")), vec![x, y]);
    }

    /// The unpublished half: a staged image is referenced by name, and the name
    /// is what gets hashed off disk.
    #[test]
    fn staged_asset_names_are_extracted() {
        let body = "![one](assets/7d0a.avif) and ![two](assets/9b11.png) and again assets/7d0a.avif";
        assert_eq!(asset_names(body), vec!["7d0a.avif", "9b11.png"]);
    }

    /// Names that would reach outside the assets directory are not read. The
    /// scan is deliberately loose, so this is where they have to stop.
    #[test]
    fn traversal_names_are_refused_before_they_reach_the_disk() {
        assert!(is_plain_name("7d0a.avif"));
        assert!(!is_plain_name("../secret.env"));
        assert!(!is_plain_name("nested/name.png"));
        assert!(!is_plain_name("nested\\name.png"));
        assert!(!is_plain_name("C:secret.env"));
        assert!(!is_plain_name(""));
    }
}
