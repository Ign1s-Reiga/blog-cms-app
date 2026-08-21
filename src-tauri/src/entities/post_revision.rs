use sea_orm::entity::prelude::*;
use sea_orm::Set;
use serde::{Deserialize, Serialize};

/// What prompted a snapshot to be taken. Recorded so the history reads as a
/// story — "before the MCP edit", "before the publish" — rather than as a list
/// of identical timestamps.
pub const SAVE: &str = "save";
pub const PUBLISH: &str = "publish";
pub const MCP: &str = "mcp";
pub const RESTORE: &str = "restore";
/// The editor's own background flush, which happens seconds apart rather than
/// at human speed — see [`crate::revisions::AUTOSAVE_COALESCE_SECS`] for what
/// keeps a typing session from filling the history on its own.
pub const AUTOSAVE: &str = "autosave";
/// The local copy, kept just before a conflict was settled by taking the
/// cloud's — the one overwrite in the app that is not the author's own typing.
pub const CONFLICT_KEEP_REMOTE: &str = "conflict_keep_remote";

/// A library-wide tag rename or merge. Its own origin because it is the one
/// edit nobody made to a particular post: an accidental merge is undone from
/// here, and an inverse rename cannot recover which posts carried which name.
pub const TAG_RENAME: &str = "tag_rename";

/// Local-only snapshot of a post as it stood *before* one particular edit,
/// keyed by its own id and pointing at the post it belongs to.
///
/// ## Why "before", not "after"
///
/// A revision is written by the path that is about to change something, holding
/// what that path is about to overwrite. Snapshotting the *result* instead would
/// leave the very first edit of an existing post — the upgrade case, and the one
/// most likely to be a mistake — with nothing behind it to go back to, because
/// the content that mattered was never recorded. Taking the "before" makes the
/// guarantee unconditional: whatever the app overwrites, it wrote down first.
///
/// The current content is not duplicated here, because it is already the post:
/// the row in `blog-db` plus `<app_data>/posts/<slug>.md` *is* the head of the
/// history.
///
/// ## Full snapshots
///
/// Bodies are stored whole rather than as diffs. A personal blog's revisions are
/// a few kilobytes each, and a diff chain is only cheaper until one link in it is
/// pruned — at which point every revision after it is unreadable. See
/// [`crate::db::REVISIONS_PER_POST`] for the cap that keeps the table bounded.
///
/// Like `post_stage` and `post_sync`, this table has no D1 counterpart. It is
/// this machine's editing history, written on paths that never touch the
/// network, so it works offline and stays out of the blog.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "post_revision")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    /// The `blog-db` post this snapshot is of.
    pub post_id: i32,
    pub title: String,
    pub excerpt: Option<String>,
    /// JSON-encoded `string[]`, exactly as the post's own column stores it.
    pub tags: Option<String>,
    pub published: bool,
    /// The Markdown body, or `None` when there was no locally cached body to
    /// snapshot.
    ///
    /// The distinction is load-bearing rather than pedantic. A post pulled from
    /// the cloud and never opened has its body in R2 and nowhere on this
    /// machine, and reaching for it would put the network in the middle of a
    /// path that must work offline. Recording an empty string instead would be
    /// worse than recording nothing: restoring it would blank the post,
    /// destroying the content this table exists to protect. `None` says what is
    /// true — the metadata was captured and the body was not — and
    /// [`crate::commands::restore_revision`] leaves the body alone for it.
    pub body: Option<String>,
    /// Which path took this snapshot: one of the constants above.
    pub origin: String,
    /// When it was taken (Unix seconds).
    pub created_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl crate::entities::record::Record for Model {
    type Entity = Entity;

    fn order_column() -> Column {
        Column::CreatedAt
    }

    fn into_insert(self) -> ActiveModel {
        ActiveModel {
            id: sea_orm::ActiveValue::NotSet,
            post_id: Set(self.post_id),
            title: Set(self.title),
            excerpt: Set(self.excerpt),
            tags: Set(self.tags),
            published: Set(self.published),
            body: Set(self.body),
            origin: Set(self.origin),
            created_at: Set(self.created_at),
        }
    }

    fn into_update(self) -> ActiveModel {
        ActiveModel {
            id: sea_orm::ActiveValue::Unchanged(self.id),
            post_id: Set(self.post_id),
            title: Set(self.title),
            excerpt: Set(self.excerpt),
            tags: Set(self.tags),
            published: Set(self.published),
            body: Set(self.body),
            origin: Set(self.origin),
            created_at: Set(self.created_at),
        }
    }
}
