//! Taking the "before" picture, on every path that overwrites a post.
//!
//! A post can now be changed from four places — the editor, an MCP client, an
//! approved publish, and a conflict settled in the cloud's favour — and until
//! this existed, a bad edit from any of them could only be undone by hand out of
//! R2. Each of those paths calls [`snapshot`] before it writes, so whatever the
//! app is about to overwrite has been written down first. See
//! [`crate::entities::post_revision`] for why the snapshot is of the state
//! *before* the edit rather than after it.
//!
//! ## Nothing here touches the network
//!
//! The body comes from the local cache and from nowhere else. That is what makes
//! the history work offline, and it is also the only honest reading available:
//! reaching for R2 mid-save would put a network round trip inside a local
//! operation, and would fetch what the *cloud* holds — which, for a post carrying
//! unpublished edits, is not the thing about to be overwritten.

use sea_orm::ConnectionTrait;
use tauri::Manager;

use crate::db;
use crate::entities::post::Model as PostModel;
use crate::entities::post_revision;
use crate::error::AppResult;
use crate::media_keys;

/// A post's Markdown as it is cached on this machine, or `None` when it is not
/// cached at all.
///
/// Deliberately *not* [`crate::commands::read_post_markdown`], which falls back
/// to downloading from R2: see the module docs.
pub async fn cached_body(app: &tauri::AppHandle, slug: &str) -> Option<String> {
    // The slug builds a file path. Everything reaching here comes from our own
    // rows, but those rows are populated from D1, so the check stays.
    if !media_keys::is_safe_slug(slug) {
        log::warn!("Not snapshotting a body for an unsafe slug: {slug}");
        return None;
    }
    let path = app
        .path()
        .app_data_dir()
        .ok()?
        .join("posts")
        .join(format!("{slug}.md"));
    tokio::fs::read_to_string(path).await.ok()
}

/// Record what `post` looks like right now, attributed to the edit that is about
/// to replace it.
///
/// `origin` is one of the constants in [`post_revision`]. `Ok(None)` means the
/// snapshot was skipped as a duplicate — see [`duplicates_head`]. The returned
/// row is mostly of interest to tests; callers on the write paths ignore it.
pub async fn snapshot(
    app: &tauri::AppHandle,
    conn: &impl ConnectionTrait,
    post: &PostModel,
    origin: &str,
) -> AppResult<Option<post_revision::Model>> {
    let body = cached_body(app, &post.slug).await;
    record(conn, post, origin, body).await
}

/// [`snapshot`] with the body already in hand — everything about taking a
/// snapshot except reading the file, so the policy can be tested without an
/// `AppHandle` and a real app data directory behind it.
pub async fn record(
    conn: &impl ConnectionTrait,
    post: &PostModel,
    origin: &str,
    body: Option<String>,
) -> AppResult<Option<post_revision::Model>> {
    let candidate = post_revision::Model {
        id: 0, // assigned on insert
        post_id: post.id,
        title: post.title.clone(),
        excerpt: post.excerpt.clone(),
        tags: post.tags.clone(),
        published: post.published,
        body,
        origin: origin.to_string(),
        created_at: chrono::Utc::now().timestamp(),
    };

    let head = db::revision_head(conn, post.id).await?;
    if head.as_ref().is_some_and(|head| duplicates_head(&candidate, head)) {
        return Ok(None);
    }
    db::revision_add(conn, candidate).await.map(Some)
}

/// Would this snapshot be a second copy of the newest one already stored?
///
/// Two paths in a row can each dutifully record "the state before my edit" for a
/// post nobody has actually changed in between — save with no edits, publish
/// straight after a save, an approved MCP publish following the agent's own save.
/// Storing the repeats costs the history its whole point: the cap is per post,
/// so fifty identical rows push the version somebody actually wants out of the
/// table.
///
/// Content only. `origin` and `created_at` differ by construction and say
/// nothing about what would be restored.
fn duplicates_head(candidate: &post_revision::Model, head: &post_revision::Model) -> bool {
    candidate.title == head.title
        && candidate.excerpt == head.excerpt
        && candidate.tags == head.tags
        && candidate.published == head.published
        && candidate.body == head.body
}

/// [`snapshot`], for the paths where failing to record history must not fail the
/// edit itself.
///
/// The asymmetry is deliberate. An ordinary save is the author's work arriving;
/// refusing it because a *record of the previous version* could not be written
/// would throw away the newer thing to protect the older one, which is exactly
/// backwards. A restore is the opposite case and calls [`snapshot`] directly —
/// there, the snapshot is what makes the operation reversible, so a restore that
/// cannot take one must not proceed.
pub async fn snapshot_or_log(
    app: &tauri::AppHandle,
    conn: &impl ConnectionTrait,
    post: &PostModel,
    origin: &str,
) {
    if let Err(e) = snapshot(app, conn, post, origin).await {
        log::warn!("Could not record a revision of post {} ({origin}): {e}", post.id);
    }
}

/// The post's row as one of its own snapshots would restore it: the metadata a
/// reader sees, taken from the revision, and everything else left alone.
///
/// The body is not here because it does not live in this row — it is a file, and
/// putting it back is the caller's job, done the same staged-write way an
/// ordinary save does it.
///
/// `published` is deliberately not restored either. A revision taken while the
/// post was a draft would otherwise take a live post off the blog as a side
/// effect of a content rollback — the same trap [`crate::commands::save_post`]
/// avoids when it refuses to let a local save clear the flag. Slug, dates and
/// series membership stay put for the same reason: none of them is what "roll
/// this text back" asks for, and a changed slug would break every link already
/// published.
pub fn apply(mut post: PostModel, revision: &post_revision::Model) -> PostModel {
    post.title = revision.title.clone();
    post.excerpt = revision.excerpt.clone();
    post.tags = revision.tags.clone();
    post
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::post_revision::Model as RevisionModel;

    fn post(title: &str) -> PostModel {
        PostModel {
            id: 1,
            slug: "a-post".into(),
            title: title.into(),
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

    /// The row a save is about to overwrite is the row that gets written down.
    #[tokio::test]
    async fn a_snapshot_holds_the_post_as_it_stood() {
        let db = db::connect_in_memory().await.unwrap();
        let saved = db::create::<PostModel>(&db, post("Original")).await.unwrap();

        let revision = record(&db, &saved, post_revision::SAVE, Some("first draft".into()))
            .await
            .unwrap()
            .expect("the first snapshot of a post is never a duplicate");

        assert_eq!(revision.post_id, saved.id);
        assert_eq!(revision.title, "Original");
        assert_eq!(revision.body.as_deref(), Some("first draft"));
        assert_eq!(revision.origin, post_revision::SAVE);
    }

    /// Two paths in a row can each record "the state before my edit" for a post
    /// nobody changed in between — a save, then the publish straight after it.
    /// Storing the repeat would spend the cap on nothing.
    #[tokio::test]
    async fn an_unchanged_post_is_not_snapshotted_twice() {
        let db = db::connect_in_memory().await.unwrap();
        let saved = db::create::<PostModel>(&db, post("Original")).await.unwrap();

        assert!(
            record(&db, &saved, post_revision::SAVE, Some("body".into()))
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            record(&db, &saved, post_revision::PUBLISH, Some("body".into()))
                .await
                .unwrap()
                .is_none(),
            "the same content was stored twice"
        );
        assert_eq!(db::revisions_for_post(&db, saved.id).await.unwrap().len(), 1);

        // A real change is recorded, on either side of the row.
        let mut edited = saved.clone();
        edited.title = "Retitled".into();
        assert!(record(&db, &edited, post_revision::SAVE, Some("body".into())).await.unwrap().is_some());
        assert!(record(&db, &edited, post_revision::SAVE, Some("new body".into())).await.unwrap().is_some());
        assert_eq!(db::revisions_for_post(&db, saved.id).await.unwrap().len(), 3);
    }

    /// "The body was not cached" and "the body was empty" are different facts,
    /// and only one of them is safe to restore. They must not collapse into each
    /// other on the way into the table.
    #[tokio::test]
    async fn an_absent_body_is_not_an_empty_one() {
        let db = db::connect_in_memory().await.unwrap();
        let saved = db::create::<PostModel>(&db, post("Original")).await.unwrap();

        let none = record(&db, &saved, post_revision::SAVE, None).await.unwrap().unwrap();
        assert_eq!(none.body, None);

        let empty = record(&db, &saved, post_revision::SAVE, Some(String::new()))
            .await
            .unwrap()
            .expect("an empty body is a change from no body at all");
        assert_eq!(empty.body.as_deref(), Some(""));
    }

    /// Restoring is a content rollback, not an editorial one. Putting a draft's
    /// text back must not take a live post off the blog as a side effect, and
    /// must not move the slug every published link points at.
    #[test]
    fn restoring_content_leaves_publication_and_identity_alone() {
        let mut live = post("Current title");
        live.published = true;
        live.published_at = Some(1_700_000_000);
        live.series_id = Some(4);

        let revision = RevisionModel {
            id: 9,
            post_id: 1,
            title: "Old title".into(),
            excerpt: Some("Old excerpt".into()),
            tags: Some(r#"["rust"]"#.into()),
            // Taken while the post was still a draft.
            published: false,
            body: Some("old body".into()),
            origin: post_revision::SAVE.into(),
            created_at: 0,
        };

        let restored = apply(live.clone(), &revision);

        assert_eq!(restored.title, "Old title");
        assert_eq!(restored.excerpt.as_deref(), Some("Old excerpt"));
        assert_eq!(restored.tags.as_deref(), Some(r#"["rust"]"#));
        assert!(restored.published, "a content rollback unpublished the post");
        assert_eq!(restored.published_at, live.published_at);
        assert_eq!(restored.slug, live.slug, "the slug moved, breaking published links");
        assert_eq!(restored.series_id, live.series_id);
    }
}
