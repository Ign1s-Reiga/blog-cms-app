//! Commands that touch only the local SQLite cache (and the app's own files).
//!
//! Nothing here reaches the network, so these work offline and without
//! Cloudflare credentials. Anything that also writes to the cloud lives in
//! `d1` or `r2` instead.

use sea_orm::{DatabaseConnection, TransactionTrait};
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;
use crate::db;
use crate::entities::post::Model as PostModel;
use crate::entities::{post_revision, post_stage};
use crate::entities::series::Model as SeriesModel;
use crate::error::{AppError, AppResult};
use crate::media_keys;
use crate::revisions;
use super::*;

// ─── Front matter ─────────────────────────────────────────────────────────────

/// Remove a leading YAML front-matter block, if the file opens with one.
///
/// Nothing reads front matter: the blog takes a post's metadata from D1 and
/// renders the body as given, so a block left in place publishes as a
/// horizontal rule followed by a heading made of the raw `title:`/`tags:`
/// lines. Imported files may still carry one from whatever wrote them, so it is
/// dropped on the way in.
///
/// The block must open on the very first line and close on a line of its own.
/// Without a closing delimiter the document is returned untouched, so a file
/// that merely starts with a `---` rule is not truncated.
fn strip_frontmatter(content: &str) -> &str {
    let Some(after_open) = content.strip_prefix("---") else {
        return content;
    };
    // The opening `---` has to be alone on its line, or it is a rule or a
    // setext heading underline rather than a delimiter.
    let Some(body) = after_open
        .strip_prefix('\n')
        .or_else(|| after_open.strip_prefix("\r\n"))
    else {
        return content;
    };

    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        if matches!(line.trim_end_matches(['\r', '\n']), "---" | "...") {
            return body[offset + line.len()..].trim_start_matches(['\r', '\n']);
        }
        offset += line.len();
    }
    content
}

#[cfg(test)]
mod tests {
    use super::strip_frontmatter;

    #[test]
    fn strips_a_leading_block() {
        let doc = "---\ntitle: My Post\ntags: rust, tauri\n---\n\nReal body.\n";
        assert_eq!(strip_frontmatter(doc), "Real body.\n");
    }

    #[test]
    fn handles_crlf_and_the_dots_terminator() {
        assert_eq!(strip_frontmatter("---\r\ntitle: x\r\n---\r\nBody\r\n"), "Body\r\n");
        assert_eq!(strip_frontmatter("---\ntitle: x\n...\nBody\n"), "Body\n");
    }

    /// A document that merely opens with a rule must survive intact — otherwise
    /// importing it would silently delete everything up to the next `---`.
    #[test]
    fn leaves_documents_without_a_closing_delimiter_alone() {
        let rule_only = "---\n\nJust a rule, then prose.\n";
        assert_eq!(strip_frontmatter(rule_only), rule_only);
    }

    #[test]
    fn leaves_ordinary_documents_alone() {
        for doc in ["# Heading\n\nBody\n", "Body only\n", "", "----\nnot a delimiter\n"] {
            assert_eq!(strip_frontmatter(doc), doc);
        }
    }

    #[test]
    fn an_empty_block_still_goes() {
        assert_eq!(strip_frontmatter("---\n---\nBody\n"), "Body\n");
    }
}

// ─── Slugs ────────────────────────────────────────────────────────────────────

/// A slug no post is using yet: `base`, else `base-2`, `base-3`, and so on.
///
/// Two files can perfectly well share a name — `notes.md` from two folders, or
/// the same document imported twice after an edit — and a slug is derived from
/// that name. Without this the second import writes `posts/<slug>.md` over the
/// first post's body and only *then* fails on the unique index, having already
/// destroyed the thing it collided with.
///
/// The search terminates: every taken candidate is a distinct existing row, so
/// it runs out after at most one step per post in the library.
pub(super) async fn unique_slug(db: &DatabaseConnection, base: &str) -> AppResult<String> {
    if db::post_by_slug(db, base).await?.is_none() {
        return Ok(base.to_string());
    }
    // From 2, so the first duplicate reads as the second copy of the post.
    for n in 2.. {
        let candidate = format!("{base}-{n}");
        if db::post_by_slug(db, &candidate).await?.is_none() {
            return Ok(candidate);
        }
    }
    unreachable!("the loop returns once a candidate is free")
}

/// A slug no series is using yet, on the same rule as [`unique_slug`].
///
/// A series slug is not cosmetic: it is the name the local table and D1 agree
/// on, so two series sharing one would make each other's posts change hands on
/// the next sync.
pub(super) async fn unique_series_slug(db: &DatabaseConnection, base: &str) -> AppResult<String> {
    if db::series_by_slug(db, base).await?.is_none() {
        return Ok(base.to_string());
    }
    for n in 2.. {
        let candidate = format!("{base}-{n}");
        if db::series_by_slug(db, &candidate).await?.is_none() {
            return Ok(candidate);
        }
    }
    unreachable!("the loop returns once a candidate is free")
}

#[cfg(test)]
mod tag_tests {
    use super::rewrite_tags;

    fn tags(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_rename_keeps_the_other_tags_where_they_were() {
        assert_eq!(
            rewrite_tags(tags(&["rust", "Tauri", "sqlite"]), "Tauri", "tauri"),
            tags(&["rust", "tauri", "sqlite"])
        );
    }

    /// Renaming onto a tag the post already has *is* the merge. The post keeps
    /// the name once, in the position the renamed tag held.
    #[test]
    fn renaming_onto_an_existing_tag_folds_the_two_together() {
        assert_eq!(
            rewrite_tags(tags(&["Rust", "sqlite", "rust"]), "Rust", "rust"),
            tags(&["rust", "sqlite"])
        );
    }

    #[test]
    fn a_post_without_the_tag_is_untouched() {
        assert_eq!(
            rewrite_tags(tags(&["rust", "tauri"]), "python", "py"),
            tags(&["rust", "tauri"])
        );
    }

    /// A duplicate that was already in the column does not survive a rewrite,
    /// but nothing else about the list changes.
    #[test]
    fn an_existing_duplicate_is_collapsed() {
        assert_eq!(
            rewrite_tags(tags(&["rust", "rust", "tauri"]), "tauri", "tauri-2"),
            tags(&["rust", "tauri-2"])
        );
    }

    #[test]
    fn renaming_the_only_tag_leaves_one_tag() {
        assert_eq!(rewrite_tags(tags(&["old"]), "old", "new"), tags(&["new"]));
    }
}

#[cfg(test)]
mod slug_tests {
    use super::*;

    fn draft(slug: &str) -> PostModel {
        PostModel {
            id: 0,
            slug: slug.to_string(),
            title: slug.to_string(),
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

    #[tokio::test]
    async fn a_free_slug_is_left_alone() {
        let db = db::connect_in_memory().await.unwrap();
        assert_eq!(unique_slug(&db, "my-post").await.unwrap(), "my-post");
    }

    /// The collision the import path used to resolve by overwriting the post it
    /// collided with.
    #[tokio::test]
    async fn a_taken_slug_moves_to_the_next_free_number() {
        let db = db::connect_in_memory().await.unwrap();

        db::create::<PostModel>(&db, draft("my-post")).await.unwrap();
        assert_eq!(unique_slug(&db, "my-post").await.unwrap(), "my-post-2");

        // Suffixed slugs are themselves posts, so the search steps past them.
        db::create::<PostModel>(&db, draft("my-post-2")).await.unwrap();
        assert_eq!(unique_slug(&db, "my-post").await.unwrap(), "my-post-3");

        // Nothing here touches an unrelated name.
        assert_eq!(unique_slug(&db, "other-post").await.unwrap(), "other-post");
    }

    /// Why the insert now runs before the file write: this is what actually
    /// refuses a duplicate, and it has to get its answer while the post it would
    /// collide with still has its body on disk.
    /// A series slug collides on the same rule as a post's, and for a sharper
    /// reason: two series sharing one would make each other's posts change
    /// hands on the next sync.
    #[tokio::test]
    async fn a_taken_series_slug_moves_to_the_next_free_number() {
        let db = db::connect_in_memory().await.unwrap();
        let series = |slug: &str| SeriesModel {
            id: 0,
            slug: slug.to_string(),
            title: slug.to_string(),
            description: None,
            created_at: 0,
        };

        assert_eq!(unique_series_slug(&db, "rust").await.unwrap(), "rust");

        db::create::<SeriesModel>(&db, series("rust")).await.unwrap();
        assert_eq!(unique_series_slug(&db, "rust").await.unwrap(), "rust-2");

        db::create::<SeriesModel>(&db, series("rust-2")).await.unwrap();
        assert_eq!(unique_series_slug(&db, "rust").await.unwrap(), "rust-3");

        // Posts and series number their slugs separately.
        db::create::<PostModel>(&db, draft("rust")).await.unwrap();
        assert_eq!(unique_series_slug(&db, "tauri").await.unwrap(), "tauri");
    }

    #[tokio::test]
    async fn the_database_refuses_a_duplicate_slug() {
        let db = db::connect_in_memory().await.unwrap();
        db::create::<PostModel>(&db, draft("taken")).await.unwrap();
        assert!(db::create::<PostModel>(&db, draft("taken")).await.is_err());
    }
}

/// Write a post's Markdown into the local cache, creating the directory.
async fn write_body(app: &tauri::AppHandle, slug: &str, body: &str) -> AppResult<()> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(AppError::AppDataDir)?
        .join("posts");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::io("Failed to create posts dir", e))?;
    tokio::fs::write(dir.join(format!("{slug}.md")), body)
        .await
        .map_err(|e| AppError::io("Failed to write local markdown", e))
}

// ─── Command ──────────────────────────────────────────────────────────────────

/// Open a native file picker and import the selected Markdown file as a draft.
///
/// The post is created locally only — body in the app's cache, metadata in the
/// local database, staged as a draft. Publishing or an explicit push is what
/// sends it to the cloud.
///
/// Returns the post title on success.
/// Returns `Err("cancelled")` when the user dismisses the dialog without
/// choosing a file — the frontend treats this differently from real errors.
#[tauri::command]
pub async fn import_article(
    app: tauri::AppHandle,
    conn: State<'_, DatabaseConnection>,
) -> AppResult<String> {
    // ── 1. File picker ────────────────────────────────────────────────────────
    // `blocking_pick_file` must not run on a tokio thread; use spawn_blocking.
    let app_clone = app.clone();
    let picked = tokio::task::spawn_blocking(move || {
        app_clone
            .dialog()
            .file()
            .add_filter("Markdown", &["md", "markdown"])
            .blocking_pick_file()
    })
    .await
    .map_err(|e| AppError::join("Dialog thread panicked", e))?;

    // Resolve to a PathBuf; return "cancelled" if the dialog was dismissed.
    let file_path = match picked {
        None => return Err(AppError::Cancelled),
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        Some(tauri_plugin_dialog::FilePath::Path(p)) => p,
        #[allow(unreachable_patterns)]
        Some(_) => return Err(AppError::UnsupportedPathFormat),
    };

    // ── 2. Read file ──────────────────────────────────────────────────────────
    let content = tokio::fs::read_to_string(&file_path)
        .await
        .map_err(|e| AppError::io("Failed to read file", e))?;

    // ── 3. Extract metadata ───────────────────────────────────────────────────
    // The file name is the only metadata an imported document carries that the
    // blog can use; tags are added afterwards in the app.
    let body = strip_frontmatter(&content);

    let stem = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled");

    let title = stem.to_string();
    let tags = String::new();

    // ── 4. Derive slug + R2 key ───────────────────────────────────────────────
    // The id is auto-assigned by the DB, so the R2 object key is keyed by slug.
    let now = now_ts();
    let base = {
        let s = slugify(&title);
        let s = if s.is_empty() { slugify(stem) } else { s };
        // Fall back to a unique, non-empty slug (e.g. non-ASCII titles).
        if s.is_empty() { format!("post-{now}") } else { s }
    };
    let slug = unique_slug(conn.inner(), &base).await?;

    // ── 5. Record the metadata locally ───────────────────────────────────────
    // Metadata first, body second. `slug` is unique, so the insert is what
    // ultimately decides whether this import may exist — and running it before
    // the file write means a slug that slipped past `unique_slug` (two imports
    // racing for the same free name) is refused while the post it collided with
    // still has its body.
    let post = PostModel {
        id: 0, // ignored on insert (auto-increment)
        slug,
        title: title.clone(),
        excerpt: None,
        tags: Some(tags_to_json(&tags)),
        published: false,
        published_at: None,
        series_id: None,
        series_order: None,
        created_at: now,
        updated_at: now,
    };
    let created = db::create::<PostModel>(conn.inner(), post).await?;

    // ── 6. Cache the body locally ────────────────────────────────────────────
    // An import lands as a draft, and a draft is local-only — the same rule
    // `save_post` follows. Nothing reaches R2 or D1 until the post is published
    // from the editor or pushed with "Push to cloud", so importing needs no
    // credentials and never puts an unpublished body in the bucket.
    if let Err(e) = write_body(&app, &created.slug, body).await {
        // Take the row back out rather than leave a post whose body never
        // landed: it would open as an empty document indistinguishable from one
        // genuinely written that way.
        let _ = db::delete::<PostModel>(conn.inner(), created.id).await;
        return Err(e);
    }

    // Imported posts start staged as Draft.
    db::stage_set(
        conn.inner(),
        post_stage::Model {
            post_id: created.id,
            stage: post_stage::DRAFT.to_string(),
            staged_at: now,
        },
    )
    .await?;
    // Local content the cloud has never seen — which is what an unsynced
    // fingerprint records.
    db::sync_set_local(
        conn.inner(),
        created.id,
        crate::sync_state::content_hash(&created, body),
    )
    .await?;

    Ok(title)
}

// ─── Body search ────────────────────────────────────────────────────────────

/// Search post bodies for `query`, over the posts that are not in the trash.
///
/// Local only: it reads the cached Markdown and never fetches. A post whose body
/// is not on this machine is reported as unsearched rather than counted as a
/// miss — see [`crate::body_search`] for why that distinction is the point of
/// the command.
///
/// The caller is expected to debounce. This walks every cached body on every
/// call, which is right for a library of this size and wrong to run per
/// keystroke; there is no index, and adding one would mean invalidating it on
/// every save, publish, refresh and rollback.
#[tauri::command]
pub async fn search_post_bodies(
    app: tauri::AppHandle,
    conn: State<'_, DatabaseConnection>,
    query: String,
) -> AppResult<crate::body_search::BodyMatches> {
    let posts = db::list_active_posts(conn.inner()).await?;
    crate::body_search::search(&app, conn.inner(), &posts, &query).await
}

// ─── Tags ────────────────────────────────────────────────────────────────────

/// A tag, and how many posts carry it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TagCount {
    pub name: String,
    pub posts: usize,
}

/// A post carrying the tag that was left exactly as it was.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Skipped {
    pub id: i32,
    pub title: String,
    pub reason: SkipReason,
}

/// Why a post was left out of a tag sweep.
///
/// Both mean "the body here cannot stand in for the body the post actually
/// has", which is what [`crate::sync_state::content_hash`] needs, and both are
/// cured the same way — open the post once. They are told apart because the
/// sentence explaining them is not the same: one says the text was never
/// fetched, the other that what was fetched has been overtaken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// Its Markdown is in R2 and nowhere on this machine.
    BodyNotCached,
    /// Its cached Markdown is behind the cloud's — a refresh moved the metadata
    /// on and the body did not follow.
    BodyStale,
}

/// What a rename did, and what it deliberately did not do.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TagRenamed {
    /// Posts whose tag list changed.
    pub changed: usize,
    /// Posts carrying the tag that were **not** rewritten, because their body is
    /// not on this machine.
    ///
    /// Not a detail. Tags are part of `content_hash`, so a rewritten post can
    /// only be marked as edited where there is a body to fingerprint — and an
    /// unmarked row is one `upsert_post_from_remote` treats as clean, so the
    /// next Refresh would take the cloud's tags and put the old name back with
    /// nothing said. Leaving the post alone and naming it here is the honest
    /// half of that trade: a rename that visibly did not finish beats one that
    /// silently comes undone.
    ///
    /// Opening such a post once brings its body down and the rename can be run
    /// again.
    pub skipped: Vec<Skipped>,
}

/// Read a post's tags out of the JSON column.
///
/// A column that is not a JSON array reads as no tags rather than as an error.
/// Everything writes it through `tags_to_json`, so anything else got there
/// before this app did, and a tag screen is not where that should surface.
fn tags_of(post: &PostModel) -> Vec<String> {
    post.tags
        .as_deref()
        .and_then(|t| serde_json::from_str::<Vec<String>>(t).ok())
        .unwrap_or_default()
}

/// Every tag in use, with its count, most-used first and ties broken by name so
/// the order does not shuffle between reads of the same library.
///
/// Grouped by the exact string stored. `Rust` and `rust` are listed separately,
/// deliberately: they are two tags to everything that stores them, and seeing
/// both is how somebody notices there is a merge to do. Search matches them
/// together, which is what hid the problem in the first place.
#[tauri::command]
pub async fn list_tags(conn: State<'_, DatabaseConnection>) -> AppResult<Vec<TagCount>> {
    let posts = db::list_active_posts(conn.inner()).await?;

    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for post in &posts {
        // A post carrying the same tag twice counts once.
        let mut seen = std::collections::HashSet::new();
        for tag in tags_of(post) {
            if seen.insert(tag.clone()) {
                *counts.entry(tag).or_insert(0) += 1;
            }
        }
    }

    let mut out: Vec<TagCount> = counts
        .into_iter()
        .map(|(name, posts)| TagCount { name, posts })
        .collect();
    out.sort_by(|a, b| b.posts.cmp(&a.posts).then_with(|| a.name.cmp(&b.name)));
    Ok(out)
}

/// Replace `from` with `to` in one post's tag list.
///
/// Order is preserved and the renamed tag stays where it was: rebuilding the
/// list from a set would reorder every post's tags as a side effect of renaming
/// one. A name that ends up present twice — which is what renaming onto an
/// existing tag means — is kept once, and that is the whole of the merge.
fn rewrite_tags(tags: Vec<String>, from: &str, to: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    tags.into_iter()
        .map(|t| if t == from { to.to_string() } else { t })
        .filter(|t| seen.insert(t.clone()))
        .collect()
}

/// Rename `from` to `to` across every post that carries it.
///
/// **This is also the merge.** Renaming onto a tag that already exists folds the
/// two together, because a post ending up with the name twice keeps it once. One
/// operation rather than two that would have to agree with each other.
///
/// Local, like every other edit here. The rewritten posts are marked as edited
/// and go up on an explicit push; nothing is published by renaming a tag.
#[tauri::command]
pub async fn rename_tag(
    app: tauri::AppHandle,
    conn: State<'_, DatabaseConnection>,
    from: String,
    to: String,
) -> AppResult<TagRenamed> {
    let from = from.trim().to_string();
    let to = to.trim().to_string();
    if from.is_empty() || to.is_empty() {
        return Err(AppError::EmptyTag);
    }
    if from == to {
        return Ok(TagRenamed { changed: 0, skipped: Vec::new() });
    }

    // The listing decides only *which* posts to visit. What is written to each
    // is taken from the row as it stands at that moment — see `retag`.
    let ids: Vec<i32> = db::list_active_posts(conn.inner())
        .await?
        .into_iter()
        .filter(|p| tags_of(p).iter().any(|t| t == &from))
        .map(|p| p.id)
        .collect();

    retag(&app, conn.inner(), &ids, post_revision::TAG_RENAME, |tags| {
        rewrite_tags(tags, &from, &to)
    })
    .await
}

/// Add a tag to each of `ids`, leaving a post that already carries it alone.
#[tauri::command]
pub async fn add_tag_to_posts(
    app: tauri::AppHandle,
    conn: State<'_, DatabaseConnection>,
    ids: Vec<i32>,
    tag: String,
) -> AppResult<TagRenamed> {
    let tag = tag.trim().to_string();
    if tag.is_empty() {
        return Err(AppError::EmptyTag);
    }
    retag(&app, conn.inner(), &ids, post_revision::BULK_TAG, |mut tags| {
        if !tags.iter().any(|t| t == &tag) {
            tags.push(tag.clone());
        }
        tags
    })
    .await
}

/// Take a tag off each of `ids`, leaving a post that does not carry it alone.
#[tauri::command]
pub async fn remove_tag_from_posts(
    app: tauri::AppHandle,
    conn: State<'_, DatabaseConnection>,
    ids: Vec<i32>,
    tag: String,
) -> AppResult<TagRenamed> {
    let tag = tag.trim().to_string();
    if tag.is_empty() {
        return Err(AppError::EmptyTag);
    }
    retag(&app, conn.inner(), &ids, post_revision::BULK_TAG, |tags| {
        tags.into_iter().filter(|t| t != &tag).collect()
    })
    .await
}

/// Apply a change to the tags of each post named, safely.
/// What a tag sweep needs besides the database: a post's Markdown as this
/// machine holds it, and somewhere to file a revision before overwriting a row.
///
/// Both of those went through the `tauri::AppHandle` directly, which is what put
/// [`retag`] out of reach of a test — `db::connect_in_memory` can stand up a
/// database, and nothing could stand up a handle. Naming the two things the
/// sweep actually wants leaves the handle to the implementation and lets a test
/// supply its own. The decisions this exists to protect — whether a body is
/// here, whether it is current, whether the row is in the trash — are all
/// decisions about *whether* to write, so they are exactly what a fake can
/// exercise.
pub(crate) trait TagSweep {
    /// The post's cached Markdown, or `None` when it is not on this machine.
    fn cached_body(&self, slug: &str) -> impl std::future::Future<Output = Option<String>>;

    /// Record `post` as it stands before the sweep overwrites it. Failures are
    /// logged rather than returned: a lost revision must not abandon a sweep
    /// halfway.
    fn snapshot(
        &self,
        txn: &sea_orm::DatabaseTransaction,
        post: &PostModel,
        origin: &str,
    ) -> impl std::future::Future<Output = ()>;
}

impl TagSweep for tauri::AppHandle {
    async fn cached_body(&self, slug: &str) -> Option<String> {
        revisions::cached_body(self, slug).await
    }

    async fn snapshot(&self, txn: &sea_orm::DatabaseTransaction, post: &PostModel, origin: &str) {
        revisions::snapshot_or_log(self, txn, post, origin).await;
    }
}

///
/// One place for the four things that have to be true of any sweep over tags,
/// so a second caller cannot get one of them wrong:
///
/// 1. **The body is read first, and it has to be the real one.** Tags are part
///    of [`crate::sync_state::content_hash`], so a post can only be marked as
///    edited where there is a body to fingerprint. Without that mark
///    `db::upsert_post_from_remote` treats the row as clean and the next
///    Refresh takes the cloud's tags back — undoing the change with nothing
///    said. A post whose body is not here is therefore **left alone** and
///    reported, rather than written and quietly reverted.
///
///    A *stale* body is left alone for the opposite reason: writing it would
///    mark the row edited, and `read_post_markdown` only refreshes a stale body
///    while the row is **not** edited. Fingerprinting text known to be behind
///    the cloud therefore switches off the refresh that would have replaced it,
///    and the app serves that text from then on as if the author had written
///    it. `body_search` already draws this distinction — a cached-but-stale
///    body is `Unchecked::BodyStale` there, on the grounds that it says nothing
///    about the version readers are being served.
/// 2. **The row is re-read inside the transaction that writes it.**
///    `into_update` writes every column, so acting on a row read before the
///    sweep began would revert a title, excerpt, publication or series that
///    somebody changed in between.
/// 3. **A revision is snapshotted first**, like every other path that
///    overwrites a post. A sweep is the hardest kind of edit to undo by hand,
///    which makes the history worth more here than anywhere.
/// 4. **The fingerprint is written in the same transaction as the row**, so
///    there is no moment where one landed and the other did not.
/// 5. **A trashed post is left alone.** Trash is a separate table, so
///    `db::get` hands one back like any other row — nothing here would have
///    noticed. `rename_tag` never sees one because it filters through
///    `db::list_active_posts` first, but the id lists behind
///    `add_tag_to_posts` and `remove_tag_from_posts` come from the frontend.
///    Checked twice for the reason [`crate::commands::refuse_if_trashed`]
///    gives: cheaply up front to skip the body read, and again inside the
///    transaction, because the answer that matters is the one at the moment of
///    the write.
///
/// `change` is given the post's current tags and returns what they should be.
/// Returning them unchanged is not an edit, and nothing is written — nor
/// reported: a post the change does not touch is simply unaffected, not one
/// that was skipped. Asked twice, cheaply up front so an unaffected post never
/// reaches the body checks, and again inside the transaction where the row it
/// compares is the one being written.
async fn retag(
    env: &impl TagSweep,
    conn: &DatabaseConnection,
    ids: &[i32],
    origin: &'static str,
    change: impl Fn(Vec<String>) -> Vec<String>,
) -> AppResult<TagRenamed> {
    let mut changed = 0usize;
    let mut skipped = Vec::new();

    for &post_id in ids {
        // Outside the transaction: reading a body is slow, and there is no point
        // opening one for a post that cannot be finished.
        let Some(post) = db::get::<PostModel>(conn, post_id).await? else {
            continue;
        };
        if db::trash_get(conn, post_id).await?.is_some() {
            continue;
        }
        // Nothing to do is not the same as could not be done. Asked here, on
        // the row already in hand, so a post the change does not touch never
        // reaches the body checks below — being told that a post was "left
        // unchanged" because its text is missing, when the tag was not on it in
        // the first place, sends the reader looking for a problem they do not
        // have. `remove_tag_from_posts` is where this shows: its ids are a
        // selection, not a filtered list like `rename_tag`'s.
        //
        // This is a shortcut, not the decision. The row can move between here
        // and the write, so the comparison that governs is the one inside the
        // transaction below, against the row read there.
        if change(tags_of(&post)) == tags_of(&post) {
            continue;
        }
        let Some(body) = env.cached_body(&post.slug).await else {
            skipped.push(Skipped {
                id: post_id,
                title: post.title,
                reason: SkipReason::BodyNotCached,
            });
            continue;
        };
        if db::body_is_stale(conn, &post.slug).await? {
            skipped.push(Skipped {
                id: post_id,
                title: post.title,
                reason: SkipReason::BodyStale,
            });
            continue;
        }

        let txn = conn.begin().await?;

        let Some(current) = db::get::<PostModel>(&txn, post_id).await? else {
            txn.rollback().await?;
            continue;
        };
        if db::trash_get(&txn, post_id).await?.is_some() {
            txn.rollback().await?;
            continue;
        }
        let before = tags_of(&current);
        let after = change(before.clone());
        if after == before {
            txn.rollback().await?;
            continue;
        }

        env.snapshot(&txn, &current, origin).await;

        let updated = PostModel {
            tags: Some(serde_json::to_string(&after).unwrap_or_else(|_| "[]".to_string())),
            updated_at: now_ts(),
            ..current
        };
        let updated = db::update::<PostModel>(&txn, updated).await?;
        db::sync_set_local(&txn, post_id, crate::sync_state::content_hash(&updated, &body)).await?;

        txn.commit().await?;
        changed += 1;
    }

    Ok(TagRenamed { changed, skipped })
}

/// Whether `post` may be renamed to `slug`, and why not when it may not.
///
/// Split out so it is the same code the tests ask. Held together in one place
/// for the same reason `retag`'s guarantees are: the reasons are easy to state
/// and easy to leave one of out.
async fn refuse_rename(conn: &DatabaseConnection, post: &PostModel, slug: &str) -> AppResult<()> {
    refuse_if_trashed(conn, post).await?;

    if post.published {
        return Err(AppError::SlugFixedByPublication(post.slug.clone()));
    }
    if db::schedule_get(conn, &post.slug).await?.is_some() {
        // A schedule uploads the body and images at the moment it is set, not
        // when the post goes live — see `schedule_post` — so this post's objects
        // are already in R2 under the old slug even though the row reads
        // unpublished.
        return Err(AppError::SlugFixedBySchedule(post.slug.clone()));
    }
    if db::sync_get(conn, post.id).await?.is_some_and(|row| row.synced_hash.is_some()) {
        // Published once and taken down again still leaves the objects behind.
        return Err(AppError::SlugFixedByPublication(post.slug.clone()));
    }

    // `slug` is unique in the table, and the trash keeps its rows there — so a
    // collision with a trashed post is a real collision, and letting the update
    // fail would surface as a raw constraint error.
    if db::post_by_slug(conn, slug).await?.is_some() {
        return Err(AppError::SlugTaken(slug.to_string()));
    }

    Ok(())
}

/// Give a post a different slug.
///
/// Only while nothing of it has left this machine. The slug is not merely the
/// row's key: it is the R2 body's key (`posts/<slug>.md`), the thumbnail's, and
/// the prefix every one of the post's images sits under — and publishing
/// rewrites body image references into absolute URLs with the slug baked in. A
/// rename after any of that has happened is a move of several objects plus a
/// rewrite of the text pointing at them, and it strands every link anybody has
/// already followed. That is a larger piece of work with a question in front of
/// it — whether the blog can be made to redirect the old URL — and this is
/// deliberately not it.
///
/// So: refused for a post that is published, scheduled, or that has ever been
/// pushed. Each is asked separately because each is a different reason, and
/// "you cannot rename this" without saying why is not an answer.
///
/// What is left is the case worth having: a draft whose title was wrong, or
/// whose slug was derived from a title since changed, corrected before anybody
/// has seen it.
#[tauri::command]
pub async fn rename_post_slug(
    app: tauri::AppHandle,
    conn: State<'_, DatabaseConnection>,
    id: i32,
    slug: String,
) -> AppResult<PostModel> {
    let slug = slug.trim().to_string();
    if !media_keys::is_safe_slug(&slug) {
        return Err(AppError::InvalidSlug(slug));
    }

    let post = db::get::<PostModel>(conn.inner(), id).await?.ok_or(AppError::PostNotFound(id))?;
    if post.slug == slug {
        return Ok(post);
    }

    refuse_rename(conn.inner(), &post, &slug).await?;

    let old = post.slug.clone();
    let renamed = db::update::<PostModel>(
        conn.inner(),
        PostModel { slug: slug.clone(), updated_at: now_ts(), ..post },
    )
    .await?;

    // The cached body is named for the slug. Moved after the row, and best
    // effort: a post that has never been pushed has nothing in R2 to re-fetch,
    // so a file that cannot be moved leaves its text on disk under the old name
    // and the editor showing an empty body. That is recoverable and visible.
    // Failing the rename here would not be: the row is already written, and
    // there is no second name for a post to have.
    if let Ok(dir) = app.path().app_data_dir() {
        let dir = dir.join("posts");
        if let Err(e) =
            tokio::fs::rename(dir.join(format!("{old}.md")), dir.join(format!("{slug}.md"))).await
        {
            log::warn!("Renamed post {id} to `{slug}` but could not move its cached body: {e}");
        }
    }

    Ok(renamed)
}

// ── Posts: local SQLite ─────────────────────────────────────────────────────────

#[tauri::command]
pub async fn create_post(
    conn: State<'_, DatabaseConnection>,
    post: PostModel,
) -> AppResult<PostModel> {
    let mut post = post;
    let now = now_ts();
    post.created_at = now;
    post.updated_at = now;
    let created = db::create::<PostModel>(conn.inner(), post).await?;
    // New posts start staged as Draft.
    db::stage_set(
        conn.inner(),
        post_stage::Model {
            post_id: created.id,
            stage: post_stage::DRAFT.to_string(),
            staged_at: now,
        },
    )
    .await?;
    Ok(created)
}

/// The library, trash excluded — what every screen except the trash view means
/// by "the posts".
#[tauri::command]
pub async fn list_posts(conn: State<'_, DatabaseConnection>) -> AppResult<Vec<PostModel>> {
    db::list_active_posts(conn.inner()).await
}

/// One post for the editor, refusing anything in the trash.
///
/// A trashed post is deleted as far as the app is concerned, but the editor can
/// still be pointed at one through a bookmark or browser history. Saying so is
/// better than opening it: an editor on a trashed post writes into the copy
/// being kept for recovery, and its Publish button puts a deleted post on the
/// blog.
#[tauri::command]
pub async fn get_post(
    conn: State<'_, DatabaseConnection>,
    id: i32,
) -> AppResult<Option<PostModel>> {
    let Some(post) = db::get::<PostModel>(conn.inner(), id).await? else {
        return Ok(None);
    };
    refuse_if_trashed(conn.inner(), &post).await?;
    Ok(Some(post))
}

#[tauri::command]
pub async fn update_post(
    conn: State<'_, DatabaseConnection>,
    post: PostModel,
) -> AppResult<PostModel> {
    // Every other write path refuses a trashed post; this one is the raw row
    // update, and leaving it open would make the rule a matter of which command
    // somebody happened to call.
    refuse_if_trashed(conn.inner(), &post).await?;
    let mut post = post;
    post.updated_at = now_ts();
    db::update::<PostModel>(conn.inner(), post).await
}

// ── Trash ───────────────────────────────────────────────────────────────────────

/// Move a post to the trash: it leaves every listing, and nothing else about it
/// changes.
///
/// This is what the delete button does. The body, the staging and sync rows and
/// the entire revision history stay exactly where they are, which is what makes
/// [`restore_post`] a single row deletion rather than a reconstruction.
///
/// **Nothing here reaches the cloud.** A published post that is trashed is still
/// on the blog and stays there; taking it down is `unpublish_post`, deliberately
/// and separately. Deleting a local copy and unpublishing are different
/// intentions, and a delete button that quietly did both would be the more
/// destructive of the two guesses.
#[tauri::command]
pub async fn trash_post(
    conn: State<'_, DatabaseConnection>,
    id: i32,
) -> AppResult<crate::entities::post_trash::Model> {
    // One transaction for the read and the write. A refresh deleting this post
    // — because the cloud no longer has it — checks the trash inside its own
    // transaction, so without this the two interleave: the post is read here,
    // the refresh commits the deletion, and the trash row lands afterwards
    // pointing at nothing. `post_trash` has no foreign key and the trash view
    // lists only rows that still have a post, so the result would be a success
    // message over a post that had been permanently deleted, and an orphan
    // nobody could see.
    let txn = conn.inner().begin().await?;

    // Refuse a post that is not there rather than writing a trash row pointing
    // at nothing, which would be invisible in every listing including the trash.
    let post = db::get::<PostModel>(&txn, id)
        .await?
        .ok_or(AppError::PostNotFound(id))?;

    // A pending publication is carried out in Cloudflare, not here, so trashing
    // the post would not stop it: the Worker would put a post on the blog that
    // this machine has thrown away. Cancelling it needs the network, and a
    // delete button that silently required a connection — and silently failed
    // without one — is worse than one that says what it needs first.
    if db::schedule_get(&txn, &post.slug)
        .await?
        .is_some_and(|s| s.is_in_flight())
    {
        return Err(AppError::ScheduledPostCannotBeTrashed(post.slug));
    }

    let trashed = db::trash_set(&txn, id, now_ts()).await?;
    txn.commit().await?;
    Ok(trashed)
}

/// Take a post back out of the trash, with everything it had when it went in.
///
/// Read and clear in one transaction, because Restore and Delete forever sit on
/// the same row. `purge` checks for the trash row inside *its* transaction, so
/// with two statements here the two overlap: purge commits between them,
/// `trash_clear` succeeds as a no-op, and this hands back a post whose row,
/// history and Markdown are gone. One transaction each means exactly one of the
/// two can win.
#[tauri::command]
pub async fn restore_post(conn: State<'_, DatabaseConnection>, id: i32) -> AppResult<PostModel> {
    let txn = conn.inner().begin().await?;
    let post = db::get::<PostModel>(&txn, id)
        .await?
        .ok_or(AppError::PostNotFound(id))?;
    db::trash_clear(&txn, id).await?;
    txn.commit().await?;
    Ok(post)
}

/// The trash, most recently thrown away first.
#[tauri::command]
pub async fn list_trashed_posts(
    conn: State<'_, DatabaseConnection>,
) -> AppResult<Vec<TrashedPost>> {
    Ok(db::list_trashed_posts(conn.inner())
        .await?
        .into_iter()
        .map(|(post, trash)| TrashedPost { trashed_at: trash.trashed_at, post })
        .collect())
}

/// A post in the trash, as the trash view reads it.
#[derive(serde::Serialize)]
pub struct TrashedPost {
    #[serde(flatten)]
    pub post: PostModel,
    /// Unix seconds. The view sorts and dates by this rather than by the post's
    /// own timestamps, which describe when it was written, not when it went.
    pub trashed_at: i64,
}

/// Delete a post from this machine for good: the row, the cached Markdown, the
/// staging and sync rows, and the revision history.
///
/// Only reachable from the trash, and only from a control that says what it
/// does. Everything else in this file can be walked back; this is the one thing
/// that cannot, which is why it is behind two deliberate steps rather than one.
///
/// **Local only, like trashing.** A post that is live on the blog stays live
/// after this — the local copy is what goes. Deleting the published article is
/// `unpublish_post` followed by the cloud's own delete, and doing it silently
/// from here would mean a control labelled "delete from this machine" quietly
/// editing the blog.
#[tauri::command]
pub async fn delete_post_permanently(
    app: tauri::AppHandle,
    conn: State<'_, DatabaseConnection>,
    id: i32,
) -> AppResult<()> {
    let post = db::get::<PostModel>(conn.inner(), id)
        .await?
        .ok_or(AppError::PostNotFound(id))?;
    purge(&app, conn.inner(), &post).await
}

/// Empty the trash, returning how many posts went.
///
/// One failure does not stop the rest: a body file that cannot be removed is
/// worth logging, not worth leaving the other twelve posts in a trash the person
/// has just asked to empty.
#[tauri::command]
pub async fn empty_trash(
    app: tauri::AppHandle,
    conn: State<'_, DatabaseConnection>,
) -> AppResult<usize> {
    let trashed = db::list_trashed_posts(conn.inner()).await?;
    let mut removed = 0usize;
    for (post, _) in trashed {
        // Re-read per post rather than trusted from the listing. Emptying the
        // trash walks it one post at a time while the trash view stays live and
        // interactive, so somebody can pull a post back out partway through —
        // and this loop, working from the snapshot it started with, would then
        // permanently delete the post they had just rescued.
        if db::trash_get(conn.inner(), post.id).await?.is_none() {
            log::info!("Post {} was restored while the trash was emptying; keeping it", post.id);
            continue;
        }
        match purge(&app, conn.inner(), &post).await {
            Ok(()) => removed += 1,
            Err(e) => log::error!("Could not permanently delete post {}: {e}", post.id),
        }
    }
    Ok(removed)
}

/// Remove every local trace of a post. Shared by the single and bulk deletes.
///
/// The row goes last. Everything before it is a side table keyed by the post's
/// id, and leaving one behind would attach a stranger's staging, sync record or
/// draft history to whichever post is assigned that id next.
async fn purge(
    app: &tauri::AppHandle,
    conn: &DatabaseConnection,
    post: &PostModel,
) -> AppResult<()> {
    // The body is moved aside *before* the transaction and removed after it —
    // the same staged-write idea `StagedBody` uses for saving, run backwards.
    //
    // Deleting the file first would mean a database error afterwards reporting
    // the deletion as unsuccessful with the text already gone: for a local-only
    // draft, content destroyed by an operation that said it had failed. But
    // deleting it *after* the commit is no better, because the commit frees the
    // slug: another window can create and save a post under the same name in
    // that gap, and the cleanup would take its body instead.
    //
    // A rename settles both. Nothing is destroyed before the commit — a failure
    // puts the file straight back — and the slug's file is already out of the
    // way before the slug becomes available, so no replacement post's Markdown
    // can be standing there when the removal happens.
    // Asked before the file is touched at all. The transaction below checks the
    // same thing and is what actually decides, but a deletion that has already
    // lost the race to Restore should not be moving anybody's Markdown around
    // in the meantime: while the archive is aside, the post is live with no
    // body, and anything that opens it reads a cache miss.
    if db::trash_get(conn, post.id).await?.is_none() {
        return Err(AppError::PostNotInTrash(post.slug.clone()));
    }

    let staged = ArchivedBody::take(app, &post.slug).await?;

    let removed = async {
        let txn = conn.begin().await?;

        // The precondition, checked inside the transaction that acts on it.
        // Restore and Delete forever are two buttons on the same row, and the
        // list reloads asynchronously between them: without this, confirming a
        // deletion just after a restore would permanently delete the post that
        // was rescued. A check outside the transaction would only narrow that
        // window rather than close it.
        if db::trash_get(&txn, post.id).await?.is_none() {
            return Err(AppError::PostNotInTrash(post.slug.clone()));
        }

        db::stage_clear(&txn, post.id).await?;
        db::sync_clear(&txn, post.id).await?;
        db::revisions_clear(&txn, post.id).await?;
        db::trash_clear(&txn, post.id).await?;
        // Keyed by slug rather than id, and a slug outlives the post: nothing
        // stops a later post being given the same title. Left behind, the mark
        // would tell that post's first read to ignore its own cached body and
        // fetch the deleted post's from R2, which is still there — the cloud's
        // copy is untouched by design.
        db::body_stale_clear(&txn, &post.slug).await?;
        // Same reasoning, same key. The schedule is settled by now — a post
        // with one still in flight cannot be trashed, let alone deleted — so
        // this is the spent record of a publication, and leaving it would show
        // it against whatever post takes the slug next.
        db::schedule_clear(&txn, &post.slug).await?;
        db::delete::<PostModel>(&txn, post.id).await?;
        // The one thing this deletion leaves behind, and the reason it can be
        // called permanent: the cloud's copy is untouched by design, so without
        // a record that this slug was deleted here the next refresh would pull
        // the post straight back in. See `post_tombstone`.
        db::tombstone_set(&txn, &post.slug, now_ts()).await?;
        txn.commit().await?;
        Ok::<(), AppError>(())
    }
    .await;

    match removed {
        Ok(()) => {
            staged.discard().await;
            // No sweep afterwards: `db::require_post` refuses a side-table write
            // for a post that no longer exists, so a save still finishing behind
            // this deletion cannot recreate the rows in the first place. A sweep
            // here would only have covered the moment it happened to run.
            Ok(())
        }
        // Nothing was deleted, so the post keeps its text.
        Err(e) => {
            staged.restore().await;
            Err(e)
        }
    }
}

/// A deleted post's Markdown, moved out of the way while its row is removed.
///
/// See [`purge`] for why neither ordering works without this: before the commit
/// the content must still be recoverable, and after it the slug is free for
/// somebody else's post to occupy.
struct ArchivedBody {
    /// Where the file went, and where it came from — `None` when the post had
    /// no cached body at all, which is an ordinary state for a post pulled from
    /// the cloud and never opened.
    moved: Option<(std::path::PathBuf, std::path::PathBuf)>,
}

impl ArchivedBody {
    /// Rename `posts/<slug>.md` aside. A missing file is not a failure.
    async fn take(app: &tauri::AppHandle, slug: &str) -> AppResult<Self> {
        if !media_keys::is_safe_slug(slug) {
            return Ok(Self { moved: None });
        }
        let dir = posts_dir(app).await?;
        let from = dir.join(format!("{slug}.md"));
        let to = dir.join(format!(".purge-{}.md.tmp", uuid::Uuid::new_v4().simple()));
        match tokio::fs::rename(&from, &to).await {
            Ok(()) => Ok(Self { moved: Some((from, to)) }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self { moved: None }),
            Err(e) => Err(AppError::io("Failed to set the post's body aside", e)),
        }
    }

    /// Put it back, because the deletion did not happen after all.
    ///
    /// Only onto an empty space. A deletion that failed because the post was
    /// restored leaves it live and editable, so an autosave or an MCP edit can
    /// have written a newer body by the time this runs — and a plain rename
    /// replaces files, which would lose that newer text under an older one that
    /// was only ever set aside. Where something is already there, it is by
    /// definition more recent than the copy this archived, and it wins.
    ///
    /// The check and the rename are not atomic, and cannot be: there is no
    /// rename-if-absent. It narrows the window to the width of one syscall
    /// pair, which is as far as the filesystem allows.
    async fn restore(self) {
        let Some((from, to)) = self.moved else { return };
        if !tokio::fs::try_exists(&from).await.unwrap_or(false) {
            if let Err(e) = tokio::fs::rename(&to, &from).await {
                log::error!("Could not put {} back after a failed deletion: {e}", from.display());
            }
            return;
        }

        // Something is already there. It may be newer than what was archived —
        // a save that landed while this was aside — or it may be *older*: the
        // post was live with no cached body for a moment, so anything that
        // opened it read a cache miss and may have refilled the file from R2,
        // which for a post carrying unpublished edits is the previous version.
        //
        // The two are indistinguishable from here, and overwriting either way
        // could destroy the better copy. So the archive is neither restored nor
        // deleted: it is left under a name the app ignores, and said out loud.
        let kept = to.with_file_name(format!(
            ".recovered-{}-{}.md",
            from.file_stem().and_then(|s| s.to_str()).unwrap_or("post"),
            uuid::Uuid::new_v4().simple()
        ));
        match tokio::fs::rename(&to, &kept).await {
            Ok(()) => log::error!(
                "{} was rewritten while its deletion was failing; the copy from before is kept at {}",
                from.display(),
                kept.display()
            ),
            Err(e) => log::error!("Could not set aside the archived body {}: {e}", to.display()),
        }
    }

    /// Throw it away, the deletion having gone through.
    ///
    /// Best effort: the post is already gone and cannot come back, so a failure
    /// here is untidiness rather than something to report as the deletion having
    /// failed.
    async fn discard(self) {
        if let Some((_, to)) = self.moved {
            if let Err(e) = tokio::fs::remove_file(&to).await {
                log::warn!("Could not remove {}: {e}", to.display());
            }
        }
    }
}

/// Every post's sync state, keyed by post id — what the list needs to show
/// which published posts are carrying edits readers have not seen.
///
/// Returned for the whole library in one call rather than per post: the list
/// renders all of them at once, and two queries beat one per row.
#[tauri::command]
pub async fn list_sync_states(
    conn: State<'_, DatabaseConnection>,
) -> AppResult<Vec<PostSyncState>> {
    let stages: std::collections::HashMap<i32, post_stage::Model> = db::stages_all(conn.inner())
        .await?
        .into_iter()
        .map(|s| (s.post_id, s))
        .collect();
    let syncs: std::collections::HashMap<i32, crate::entities::post_sync::Model> =
        db::sync_all(conn.inner())
            .await?
            .into_iter()
            .map(|s| (s.post_id, s))
            .collect();

    Ok(db::list_active_posts(conn.inner())
        .await?
        .into_iter()
        .map(|post| PostSyncState {
            post_id: post.id,
            state: crate::sync_state::derive(stages.get(&post.id), syncs.get(&post.id)),
        })
        .collect())
}

/// One post's sync state, as the frontend reads it.
#[derive(serde::Serialize)]
pub struct PostSyncState {
    pub post_id: i32,
    pub state: crate::sync_state::SyncState,
}

// ── Revisions: local SQLite ─────────────────────────────────────────────────────

/// One entry in a post's history, as the editor's panel lists them.
///
/// Bodies are left out: a list of fifty versions of the same post would ship the
/// whole post fifty times to render a column of timestamps. `get_revision`
/// fetches the one the person actually opened.
#[derive(serde::Serialize)]
pub struct RevisionSummary {
    pub id: i32,
    pub post_id: i32,
    pub title: String,
    /// Why the snapshot was taken — one of the constants in
    /// [`crate::entities::post_revision`].
    pub origin: String,
    pub created_at: i64,
    /// Whether the post was live at the time. Shown because a version's text
    /// alone does not say whether readers were seeing it.
    pub published: bool,
    /// Characters of Markdown in the snapshot, or `None` when it carries no body
    /// at all — which the panel renders as "metadata only", since restoring one
    /// deliberately leaves the text alone.
    pub body_chars: Option<usize>,
}

impl From<crate::entities::post_revision::Model> for RevisionSummary {
    fn from(r: crate::entities::post_revision::Model) -> Self {
        Self {
            id: r.id,
            post_id: r.post_id,
            title: r.title,
            origin: r.origin,
            created_at: r.created_at,
            published: r.published,
            body_chars: r.body.as_deref().map(str::len),
        }
    }
}

/// A post's saved versions, newest first.
#[tauri::command]
pub async fn list_revisions(
    conn: State<'_, DatabaseConnection>,
    post_id: i32,
) -> AppResult<Vec<RevisionSummary>> {
    Ok(db::revisions_for_post(conn.inner(), post_id)
        .await?
        .into_iter()
        .map(RevisionSummary::from)
        .collect())
}

/// One saved version in full, body included — what the panel previews before
/// anyone commits to restoring it.
#[tauri::command]
pub async fn get_revision(
    conn: State<'_, DatabaseConnection>,
    revision_id: i32,
) -> AppResult<crate::entities::post_revision::Model> {
    db::revision_get(conn.inner(), revision_id)
        .await?
        .ok_or(AppError::RevisionNotFound(revision_id))
}

/// Put a post back to the version this snapshot holds.
///
/// Local only, and deliberately so: a restore writes the row and the cached
/// Markdown and stops there, leaving the post exactly as `modified` as any other
/// unpublished edit. Pushing it would publish a rollback nobody asked to
/// publish, from a button labelled "restore" — and would do it without the
/// approval gate for a post an MCP client had been editing.
///
/// **Restoring is itself an edit, so it takes its own snapshot first.** That is
/// what makes the operation reversible: restoring the wrong version leaves the
/// version you were on sitting at the top of the history, one click away. It is
/// also why nothing here deletes revisions — history only ever grows, until the
/// cap prunes its oldest end.
///
/// Unlike the edit paths, this snapshot is not best effort. A restore that could
/// not record where it came from would overwrite the current text with no way
/// back, which is precisely the failure this feature exists to prevent.
#[tauri::command]
pub async fn restore_revision(
    app: tauri::AppHandle,
    conn: State<'_, DatabaseConnection>,
    revision_id: i32,
) -> AppResult<PostModel> {
    let revision = db::revision_get(conn.inner(), revision_id)
        .await?
        .ok_or(AppError::RevisionNotFound(revision_id))?;
    let current = db::get::<PostModel>(conn.inner(), revision.post_id)
        .await?
        .ok_or(AppError::PostNotFound(revision.post_id))?;
    // A post in the trash keeps its history precisely so that restoring it
    // brings everything back — but the restoring happens after it comes out of
    // the trash, not into it.
    refuse_if_trashed(conn.inner(), &current).await?;

    // Where we are now, before it is replaced.
    revisions::snapshot(&app, conn.inner(), &current, post_revision::RESTORE).await?;

    // A snapshot with no body records metadata that was captured while the text
    // was not — see `post_revision::Model::body`. Restoring it must therefore
    // leave the text alone rather than blank the post, so the body that ends up
    // on disk is either the snapshot's or the one already there.
    let dir = posts_dir(&app).await?;
    let staged = match revision.body.as_deref() {
        Some(body) => Some((StagedBody::write(&dir, body).await?, body.to_string())),
        None => None,
    };

    // A restore replaces a live body, so it takes the same lock a save does and
    // holds it across the row and the rename together. Otherwise a read that
    // found the cached copy stale can put the cloud's version on disk between
    // the two, and the post is left describing the revision it was rolled back
    // to while holding the published text. See `lock_body_commits`.
    //
    // Held to the end of the function: nothing below it reaches the network, and
    // the fingerprint it finishes with is the very thing a reader consults to
    // decide whether this body is safe to replace.
    let _body_guard = lock_body_commits().await;

    // Re-checked inside the transaction that writes. The guard above ran before
    // the snapshot and the staged body, and another window can throw the post
    // away in that time — leaving Restore-from-trash handing back a version
    // changed after the deletion rather than the one discarded.
    let txn = conn.inner().begin().await?;
    if db::trash_get(&txn, current.id).await?.is_some() {
        if let Some((staged, _)) = staged {
            staged.discard().await;
        }
        return Err(AppError::PostInTrash(current.slug));
    }
    let restored = db::update::<PostModel>(
        &txn,
        PostModel { updated_at: now_ts(), ..revisions::apply(current.clone(), &revision) },
    )
    .await?;
    txn.commit().await?;

    let body = match staged {
        Some((staged, body)) => {
            // Cleared before the rename, and allowed to fail the restore. The
            // other order leaves a database outage between the two with new text
            // on disk, the mark still standing and no fingerprint — which a later
            // read resolves by fetching the published copy over it. See the same
            // ordering in `commands::r2::save`.
            let was_stale = db::body_is_stale(conn.inner(), &restored.slug).await?;
            db::body_stale_clear(conn.inner(), &restored.slug).await?;
            if let Err(e) = staged.commit(&dir.join(format!("{}.md", restored.slug))).await {
                if was_stale {
                    let _ = db::body_stale_set(conn.inner(), &restored.slug, now_ts()).await;
                }
                // The row is already back at the old version and its body is
                // not, so the row has to go forward again rather than sit there
                // describing text that was never written. Best effort: the
                // failure above is the one worth reporting.
                if let Err(undo) = db::update::<PostModel>(conn.inner(), current).await {
                    log::error!("Could not undo a restore whose body did not land: {undo}");
                }
                return Err(e);
            }
            body
        }
        // Nothing replaced the file, so what is on disk is what the fingerprint
        // below has to be taken over.
        None => revisions::cached_body(&app, &restored.slug).await.unwrap_or_default(),
    };

    // A restore changes what this machine holds and nothing else, which is
    // exactly what an unsynced fingerprint records. The stage is left alone on
    // purpose: a live post is still live, serving the version it was serving
    // before, and this rollback is one more edit waiting to be published.
    db::sync_set_local(
        conn.inner(),
        restored.id,
        crate::sync_state::content_hash(&restored, &body),
    )
    .await?;

    Ok(restored)
}

// ── Series: local SQLite ────────────────────────────────────────────────────────

#[tauri::command]
pub async fn create_series(
    conn: State<'_, DatabaseConnection>,
    series: SeriesModel,
) -> AppResult<SeriesModel> {
    let mut series = series;
    series.created_at = now_ts();
    // The slug is derived here rather than taken as given. It is the identity
    // both databases agree on, so it has to be unique and URL-safe whatever the
    // caller sent — and a title is the only thing the screen asks for.
    let base = {
        let s = slugify(if series.slug.trim().is_empty() { &series.title } else { &series.slug });
        if s.is_empty() { format!("series-{}", series.created_at) } else { s }
    };
    series.slug = unique_series_slug(conn.inner(), &base).await?;
    db::create::<SeriesModel>(conn.inner(), series).await
}

#[tauri::command]
pub async fn list_series(conn: State<'_, DatabaseConnection>) -> AppResult<Vec<SeriesModel>> {
    db::list::<SeriesModel>(conn.inner()).await
}

#[tauri::command]
pub async fn get_series(
    conn: State<'_, DatabaseConnection>,
    id: i32,
) -> AppResult<Option<SeriesModel>> {
    db::get::<SeriesModel>(conn.inner(), id).await
}

#[tauri::command]
pub async fn update_series(
    conn: State<'_, DatabaseConnection>,
    series: SeriesModel,
) -> AppResult<SeriesModel> {
    // Title and description are editable; `slug` and `created_at` are not, and
    // are read back from the stored row rather than taken from the caller.
    //
    // The slug is how the local table and D1 recognise the same series. Letting
    // a rename move it would make the renamed series a *different* series to the
    // cloud: the next push would insert a second row, and every post filed here
    // would cross over pointing at the old one.
    let stored = db::get::<SeriesModel>(conn.inner(), series.id)
        .await?
        .ok_or(AppError::SeriesNotFound(series.id))?;
    let series = SeriesModel {
        slug: stored.slug,
        created_at: stored.created_at,
        ..series
    };
    db::update::<SeriesModel>(conn.inner(), series).await
}

#[tauri::command]
pub async fn delete_series(conn: State<'_, DatabaseConnection>, id: i32) -> AppResult<u64> {
    // The posts come out of the series in the same transaction that removes it.
    // A delete on its own leaves them pointing at an id that no longer names
    // anything: still a number, so nothing complains, and the post reads as
    // filed under a series nobody can look up.
    //
    // Returns how many posts were unfiled, so the screen can say what happened
    // rather than leaving it to be discovered.
    let txn = conn.inner().begin().await?;
    let unfiled = db::unfile_series(&txn, id).await?;
    db::delete::<SeriesModel>(&txn, id).await?;
    txn.commit().await?;
    Ok(unfiled)
}

/// File a post under a series, or take it out of one.
///
/// Separate from the editor's save because it is a different kind of change:
/// [`crate::sync_state::content_hash`] covers what a reader would notice of the
/// post's own content, and membership of a series is not part of that. A post
/// whose series changed is not "edited" in the sense the Edited filter means,
/// and marking it so would put it there on a change to nothing it contains.
///
/// It still reaches the cloud: a push sends every post, not only the changed
/// ones, so the new filing crosses with the next "Push to cloud".
#[tauri::command]
pub async fn set_post_series(
    conn: State<'_, DatabaseConnection>,
    post_id: i32,
    series_id: Option<i32>,
    series_order: Option<i32>,
) -> AppResult<PostModel> {
    let post = db::get::<PostModel>(conn.inner(), post_id)
        .await?
        .ok_or(AppError::PostNotFound(post_id))?;

    // A series that is not there cannot be filed under: the id would be stored
    // and read back as a series nobody can name, which is the state
    // `delete_series` exists to prevent.
    if let Some(id) = series_id {
        if db::get::<SeriesModel>(conn.inner(), id).await?.is_none() {
            return Err(AppError::SeriesNotFound(id));
        }
    }

    let updated = PostModel {
        series_id,
        // An order without a series is a number about nothing.
        series_order: series_id.and(series_order),
        updated_at: now_ts(),
        ..post
    };
    db::update::<PostModel>(conn.inner(), updated).await
}

/// Set (or clear) a post's local staging stage without publishing.
#[tauri::command]
pub async fn set_post_stage(
    conn: State<'_, DatabaseConnection>,
    post_id: i32,
    stage: String,
) -> AppResult<post_stage::Model> {
    validate_stage(&stage)?;
    db::stage_set(
        conn.inner(),
        post_stage::Model { post_id, stage, staged_at: now_ts() },
    )
    .await
}

#[tauri::command]
pub async fn get_post_stage(
    conn: State<'_, DatabaseConnection>,
    post_id: i32,
) -> AppResult<Option<post_stage::Model>> {
    db::stage_get(conn.inner(), post_id).await
}

#[tauri::command]
pub async fn list_posts_by_stage(
    conn: State<'_, DatabaseConnection>,
    stage: String,
) -> AppResult<Vec<PostModel>> {
    validate_stage(&stage)?;
    db::posts_in_stage(conn.inner(), stage).await
}

/// Tests for the tag sweep.
///
/// Every one of these covers a bug that shipped without a test because nothing
/// could construct a `tauri::AppHandle`. [`TagSweep`] exists so they can.
#[cfg(test)]
mod tag_sweep_tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;
    use crate::db::connect_in_memory;
    use crate::entities::post;

    /// A stand-in for the app: bodies come from a map, revisions are counted
    /// rather than written.
    struct FakeSweep {
        bodies: HashMap<String, String>,
        snapshots: Mutex<Vec<i32>>,
    }

    impl FakeSweep {
        fn with(bodies: &[(&str, &str)]) -> Self {
            Self {
                bodies: bodies.iter().map(|(s, b)| (s.to_string(), b.to_string())).collect(),
                snapshots: Mutex::new(Vec::new()),
            }
        }

        fn snapshotted(&self) -> Vec<i32> {
            self.snapshots.lock().unwrap().clone()
        }
    }

    impl TagSweep for FakeSweep {
        async fn cached_body(&self, slug: &str) -> Option<String> {
            self.bodies.get(slug).cloned()
        }

        async fn snapshot(
            &self,
            _txn: &sea_orm::DatabaseTransaction,
            post: &PostModel,
            _origin: &str,
        ) {
            self.snapshots.lock().unwrap().push(post.id);
        }
    }

    fn a_post(slug: &str, tags: &[&str]) -> post::Model {
        post::Model {
            id: 0,
            slug: slug.to_string(),
            title: slug.to_string(),
            excerpt: None,
            tags: Some(serde_json::to_string(tags).unwrap()),
            published: true,
            published_at: None,
            series_id: None,
            series_order: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    /// Adds `new` to whatever the post carries — the shape `add_tag_to_posts`
    /// uses, without going through the command and its `State`.
    fn add(new: &'static str) -> impl Fn(Vec<String>) -> Vec<String> {
        move |mut tags: Vec<String>| {
            if !tags.iter().any(|t| t == new) {
                tags.push(new.to_string());
            }
            tags
        }
    }

    async fn locally_edited(conn: &DatabaseConnection, id: i32) -> bool {
        db::sync_get(conn, id)
            .await
            .unwrap()
            .is_some_and(|row| crate::sync_state::local_changed(&row))
    }

    /// The ordinary case, so the guards below are known to be refusing
    /// something that would otherwise have gone through.
    #[tokio::test]
    async fn a_post_with_a_body_is_tagged_and_marked_edited() {
        let conn = connect_in_memory().await.unwrap();
        let post = db::create::<post::Model>(&conn, a_post("hello", &["rust"])).await.unwrap();
        let env = FakeSweep::with(&[("hello", "# Hello\n")]);

        let out = retag(&env, &conn, &[post.id], post_revision::BULK_TAG, add("tauri")).await.unwrap();

        assert_eq!(out.changed, 1);
        assert!(out.skipped.is_empty());
        assert_eq!(tags_of(&db::get::<PostModel>(&conn, post.id).await.unwrap().unwrap()), vec![
            "rust".to_string(),
            "tauri".to_string()
        ]);
        assert!(locally_edited(&conn, post.id).await, "the edit was not marked, so a refresh would undo it");
        assert_eq!(env.snapshotted(), vec![post.id]);
    }

    /// #124. Fingerprinting a body known to be behind the cloud marks the row
    /// edited, and `read_post_markdown` only refreshes a stale body while the
    /// row is *not* edited — so doing it here switches off the refresh that
    /// would have replaced the text, permanently.
    #[tokio::test]
    async fn a_stale_body_is_not_fingerprinted() {
        let conn = connect_in_memory().await.unwrap();
        let post = db::create::<post::Model>(&conn, a_post("stale", &["rust"])).await.unwrap();
        db::body_stale_set(&conn, "stale", 100).await.unwrap();
        let env = FakeSweep::with(&[("stale", "text this machine has not caught up with\n")]);

        let out = retag(&env, &conn, &[post.id], post_revision::BULK_TAG, add("tauri")).await.unwrap();

        assert_eq!(out.changed, 0);
        assert_eq!(out.skipped.len(), 1);
        assert_eq!(out.skipped[0].reason, SkipReason::BodyStale);
        assert!(
            !locally_edited(&conn, post.id).await,
            "a stale body was fingerprinted, which stops the refresh that would have replaced it"
        );
        assert_eq!(tags_of(&db::get::<PostModel>(&conn, post.id).await.unwrap().unwrap()), vec!["rust".to_string()]);
        assert!(env.snapshotted().is_empty());
    }

    /// The other half of #124's reasoning: a body that is simply not here is
    /// reported differently, because the sentence shown to the user is not the
    /// same one.
    #[tokio::test]
    async fn a_missing_body_is_reported_as_not_cached() {
        let conn = connect_in_memory().await.unwrap();
        let post = db::create::<post::Model>(&conn, a_post("absent", &["rust"])).await.unwrap();
        let env = FakeSweep::with(&[]);

        let out = retag(&env, &conn, &[post.id], post_revision::BULK_TAG, add("tauri")).await.unwrap();

        assert_eq!(out.changed, 0);
        assert_eq!(out.skipped.len(), 1);
        assert_eq!(out.skipped[0].reason, SkipReason::BodyNotCached);
    }

    /// #123. Trash is its own table, so `db::get` hands a trashed row back like
    /// any other and nothing in the sweep used to notice.
    #[tokio::test]
    async fn a_trashed_post_is_left_alone() {
        let conn = connect_in_memory().await.unwrap();
        let post = db::create::<post::Model>(&conn, a_post("binned", &["rust"])).await.unwrap();
        db::trash_set(&conn, post.id, 100).await.unwrap();
        let env = FakeSweep::with(&[("binned", "# Binned\n")]);

        let out = retag(&env, &conn, &[post.id], post_revision::BULK_TAG, add("tauri")).await.unwrap();

        assert_eq!(out.changed, 0);
        assert!(out.skipped.is_empty(), "a trashed post was never eligible, so it is not a skip to report");
        assert_eq!(tags_of(&db::get::<PostModel>(&conn, post.id).await.unwrap().unwrap()), vec!["rust".to_string()]);
        assert!(!locally_edited(&conn, post.id).await);
        assert!(env.snapshotted().is_empty());
    }

    /// #127. A post the change does not touch is unaffected, not skipped —
    /// reporting it said its text was missing and sent the user looking for a
    /// problem they did not have.
    #[tokio::test]
    async fn a_post_with_nothing_to_change_is_not_reported() {
        let conn = connect_in_memory().await.unwrap();
        // Carries the tag already, and has no body — the combination that used
        // to produce the misleading report.
        let post = db::create::<post::Model>(&conn, a_post("already", &["rust", "tauri"])).await.unwrap();
        let env = FakeSweep::with(&[]);

        let out = retag(&env, &conn, &[post.id], post_revision::BULK_TAG, add("tauri")).await.unwrap();

        assert_eq!(out.changed, 0);
        assert!(
            out.skipped.is_empty(),
            "a post that needed no edit was reported as one the edit could not be recorded for"
        );
    }
}

/// Tests for the slug rename.
///
/// The value here is in the refusals. Renaming a draft is a column update;
/// renaming something whose objects are already in R2 under the old name is the
/// bug this exists to prevent, and each way a post can have got there is asked
/// about separately.
#[cfg(test)]
mod rename_slug_tests {
    use super::*;
    use crate::db::connect_in_memory;
    use crate::entities::{post, post_schedule};

    fn a_post(slug: &str, published: bool) -> post::Model {
        post::Model {
            id: 0,
            slug: slug.to_string(),
            title: slug.to_string(),
            excerpt: None,
            tags: None,
            published,
            published_at: None,
            series_id: None,
            series_order: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    /// The command's own guard, asked directly. Only the cached-body move is
    /// left out, which needs an `AppHandle` — it is best effort and logged
    /// either way, and nothing about it can refuse a rename.
    async fn refusal_for(conn: &DatabaseConnection, id: i32, to: &str) -> Option<String> {
        let post = db::get::<PostModel>(conn, id).await.unwrap().unwrap();
        match refuse_rename(conn, &post, to).await {
            Ok(()) => None,
            Err(AppError::SlugFixedByPublication(_)) => Some("published".into()),
            Err(AppError::SlugFixedBySchedule(_)) => Some("scheduled".into()),
            Err(AppError::SlugTaken(_)) => Some("taken".into()),
            Err(e) => Some(format!("unexpected: {e}")),
        }
    }

    #[tokio::test]
    async fn a_draft_that_has_never_left_this_machine_may_be_renamed() {
        let conn = connect_in_memory().await.unwrap();
        let post = db::create::<post::Model>(&conn, a_post("typoo", false)).await.unwrap();
        // Edited locally, never pushed — `synced_hash` stays `None`.
        db::sync_set_local(&conn, post.id, "local".to_string()).await.unwrap();

        assert_eq!(refusal_for(&conn, post.id, "typo").await, None);
    }

    #[tokio::test]
    async fn a_published_post_is_refused() {
        let conn = connect_in_memory().await.unwrap();
        let post = db::create::<post::Model>(&conn, a_post("live", true)).await.unwrap();

        assert_eq!(refusal_for(&conn, post.id, "different").await.as_deref(), Some("published"));
    }

    /// Scheduling uploads the body and images when the schedule is set, not when
    /// the post goes live, so a scheduled post is in R2 already even though its
    /// row still reads unpublished.
    #[tokio::test]
    async fn a_scheduled_post_is_refused_even_though_it_is_not_published() {
        let conn = connect_in_memory().await.unwrap();
        let post = db::create::<post::Model>(&conn, a_post("pending", false)).await.unwrap();
        db::schedule_set(
            &conn,
            post_schedule::Model {
                slug: "pending".to_string(),
                publish_at: 100,
                state: post_schedule::PENDING.to_string(),
                error: None,
                updated_at: 0,
            },
        )
        .await
        .unwrap();

        assert!(!db::get::<PostModel>(&conn, post.id).await.unwrap().unwrap().published);
        assert_eq!(refusal_for(&conn, post.id, "different").await.as_deref(), Some("scheduled"));
    }

    /// Published once and taken down again: the row reads unpublished, and the
    /// objects are still in R2 under the old slug.
    #[tokio::test]
    async fn a_post_that_was_pushed_and_unpublished_is_refused() {
        let conn = connect_in_memory().await.unwrap();
        let post = db::create::<post::Model>(&conn, a_post("was-live", false)).await.unwrap();
        db::sync_agree(&conn, post.id, "pushed".to_string(), Some(100), 100).await.unwrap();

        // Same refusal as a live post, and the same reason: its objects are in R2.
        assert_eq!(refusal_for(&conn, post.id, "different").await.as_deref(), Some("published"));
    }

    /// The trash keeps its rows in the posts table, so its slugs are still
    /// taken — a collision there would otherwise surface as a raw constraint
    /// error from the update.
    #[tokio::test]
    async fn a_slug_held_by_a_trashed_post_is_refused() {
        let conn = connect_in_memory().await.unwrap();
        let draft = db::create::<post::Model>(&conn, a_post("draft", false)).await.unwrap();
        let binned = db::create::<post::Model>(&conn, a_post("wanted", false)).await.unwrap();
        db::trash_set(&conn, binned.id, 100).await.unwrap();

        assert_eq!(refusal_for(&conn, draft.id, "wanted").await.as_deref(), Some("taken"));
    }
}
