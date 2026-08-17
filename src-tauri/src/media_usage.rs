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

/// A post the survey could not read, and therefore could not match anything
/// against.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UnreadPost {
    pub id: i32,
    pub title: String,
}

/// What a survey found, and what it could not see.
///
/// The second half matters as much as the first. A post pulled from the cloud
/// and never opened has its Markdown in R2 and nowhere on this machine, and
/// `sync_posts_from_cloud` mirrors metadata only — so a library can contain
/// posts whose references are simply unknown here. Reporting the objects alone
/// would let every image used by such a post read as "not used", which is the
/// one answer that invites a delete.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UsageReport {
    pub objects: Vec<MediaUsage>,
    /// Empty when every post's body was readable, which is the ordinary case.
    /// While it is not, "no known users" cannot be read as "unused".
    pub unread_posts: Vec<UnreadPost>,
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
        // Every 64-character window of the run, not the run itself. The key
        // layout is configurable, and a pattern is free to put `{hash}` next to
        // something else hexadecimal — `posts/{slug}/a{hash}.{ext}`, or a slug
        // ending in `beef` — which makes the digest a *substring* of a longer
        // run rather than the whole of it. An exact-length check misses those
        // and reports the image as unused, which is the answer that invites a
        // delete.
        //
        // Widening this cannot produce a false positive that matters: a window
        // only counts if some library object's bytes actually hash to it.
        for window in run.as_bytes().windows(64) {
            let candidate = String::from_utf8_lossy(window).into_owned();
            if !found.contains(&candidate) {
                found.push(candidate);
            }
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

/// The digests one post's body depends on, staged and published alike, or
/// `None` when its Markdown is not on this machine to be read.
///
/// `None` is not the same as "no references", and collapsing the two is what
/// would make a post pulled from the cloud and never opened look like a post
/// that uses no images. Fetching the body instead would turn listing the media
/// library into a download of the whole blog, so the honest move is to say the
/// answer is unknown and let the caller carry that upwards.
async fn digests_used_by(
    app: &tauri::AppHandle,
    post: &PostModel,
) -> AppResult<Option<HashSet<String>>> {
    let Some(body) = crate::revisions::cached_body(app, &post.slug).await else {
        return Ok(None);
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
    Ok(Some(digests))
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
) -> AppResult<UsageReport> {
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
    let mut unread_posts = Vec::new();
    for post in posts {
        let using = UsingPost {
            id: post.id,
            slug: post.slug.clone(),
            title: post.title.clone(),
            trashed: trashed.contains(&post.id),
            published: post.published,
        };
        match digests_used_by(app, &post).await? {
            Some(digests) => {
                for digest in digests {
                    by_digest.entry(digest).or_default().push(using.clone());
                }
            }
            // Its Markdown is in R2 and nowhere here — pulled from the cloud and
            // never opened. Whatever it references cannot be seen from this
            // machine, so it is named rather than passed over in silence.
            None => unread_posts.push(UnreadPost { id: post.id, title: post.title }),
        }
    }

    let mut objects: Vec<MediaUsage> = library
        .into_iter()
        .map(|(key, digest)| MediaUsage {
            posts: by_digest.get(&digest).cloned().unwrap_or_default(),
            key,
        })
        .collect();
    objects.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(UsageReport { objects, unread_posts })
}

/// What the deletion guard needs to know about one object: who is known to use
/// it, and whether anything could not be checked.
///
/// The second half is what stops an unprovable "unused" from reading as a
/// proven one. A library with a post nobody has opened on this machine cannot
/// say that any object is unreferenced, and the guard treats that the same way
/// it treats a known reference: it asks first.
pub struct DeletionCheck {
    pub users: Vec<UsingPost>,
    pub unread_posts: Vec<UnreadPost>,
    /// The object itself could not be hashed, so nothing was matched against
    /// it. Distinct from an empty `users`, which is a real answer.
    pub unknown: bool,
}

impl DeletionCheck {
    /// Can this object be deleted without asking anybody?
    ///
    /// Only when the question was actually answered: no post references it, no
    /// post's body was unreadable, and the object itself could be hashed.
    /// Anything else is "nobody checked", which must not be allowed to look
    /// like "nothing uses it".
    pub fn is_safe(&self) -> bool {
        !self.unknown && self.users.is_empty() && self.unread_posts.is_empty()
    }
}

/// The posts referencing one object, for the check that guards a deletion.
pub async fn users_of(
    app: &tauri::AppHandle,
    conn: &DatabaseConnection,
    key: &str,
) -> AppResult<DeletionCheck> {
    let report = survey(app, conn).await?;
    let found = report.objects.into_iter().find(|u| u.key == key);
    Ok(DeletionCheck {
        // An object the survey has no row for is one it could not hash — not
        // cached locally, so there were no bytes to match posts against.
        // Defaulting that to "no users" would make the backend's own guard say
        // an unchecked object is safe to delete, which is exactly the guarantee
        // it exists to provide.
        unknown: found.is_none(),
        users: found.map(|u| u.posts).unwrap_or_default(),
        unread_posts: report.unread_posts,
    })
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
        // Two in one body, both found.
        let (x, y) = ("e".repeat(64), "f".repeat(64));
        assert_eq!(published_digests(&format!("{x} and {y}")), vec![x, y]);
    }

    /// A run longer than a digest *contains* digests, and which of its windows
    /// is the real one cannot be known from here.
    ///
    /// The key layout is configurable, so a pattern may place `{hash}` against
    /// something else hexadecimal — `posts/{slug}/a{hash}.{ext}`, or any slug
    /// ending in `beef`. Reading only exact-length runs missed the digest in
    /// those layouts and reported the image as unused, which is the one answer
    /// that invites a delete.
    #[test]
    fn a_hash_is_found_even_when_something_hexadecimal_abuts_it() {
        let digest = "a".repeat(64);
        let found = published_digests(&format!("https://cdn.example.com/posts/beef/{digest}.avif"));
        assert!(found.contains(&digest), "the digest was missed next to a hex slug");

        // Every *distinct* window of an over-long run is offered; only one can
        // match a real object's bytes, so the extra candidates cost nothing.
        // A run of one repeated character has just the one window to offer.
        assert_eq!(published_digests(&format!("{}b", "d".repeat(64))).len(), 2);
        assert_eq!(published_digests(&"d".repeat(65)).len(), 1);
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
