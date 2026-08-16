use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Local-only record that a post was permanently deleted here, keyed by the
/// slug the two databases agree on.
///
/// ## Why "forever" needs a marker
///
/// Deleting from the trash removes the local row, and deliberately leaves the
/// cloud's copy alone — taking a published post off the blog is `unpublish_post`,
/// not a side effect of clearing local storage. But the refresh is a mirror: a
/// remote post with no local counterpart is one the local library has not seen
/// yet, so the next pull would insert the post straight back, body and all, and
/// "Delete forever" would last until somebody pressed Refresh.
///
/// A tombstone is the missing fact. It says *this machine has deleted that
/// slug*, which is exactly what distinguishes "never seen" from "seen and thrown
/// away for good".
///
/// ## It cleans up after itself
///
/// A tombstone is only consulted while no local post holds the slug, and
/// [`crate::db::mirror_posts`] drops any whose slug the cloud no longer has —
/// once the remote copy is gone there is nothing left to keep out. So the table
/// stays as small as the set of posts deleted here but still live on the blog,
/// and it empties itself as those are taken down.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "post_tombstone")]
pub struct Model {
    /// The slug that was deleted. Slugs, not ids: the local id is gone with the
    /// row, and it never meant anything in D1 anyway.
    #[sea_orm(primary_key, auto_increment = false)]
    pub slug: String,
    /// When it was deleted (Unix seconds).
    pub deleted_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
