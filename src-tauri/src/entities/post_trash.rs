use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Local-only record that a post has been moved to the trash, keyed by the
/// post's id.
///
/// A row here means "deleted" everywhere the library is listed, while the post,
/// its body, its staging and sync rows and its whole revision history stay
/// exactly where they were. Restoring is deleting this one row; there is nothing
/// to put back, because nothing was taken away.
///
/// ## Why a table and not a column
///
/// `post::Model` is not only the local shape — it *is* the statement sent to D1
/// (see `cloudflare::d1_insert` and `d1_post_upsert`, which build their SQL from
/// the entity). `blog-db`'s columns belong to the blog's own schema, so a
/// `trashed_at` column on the post would have to be migrated into D1 by hand
/// before this app could write anything at all, and would put a local editorial
/// state into a table the blog reads. The same reasoning already keeps
/// `post_stage`, `post_sync` and `post_revision` out of the post row.
///
/// ## Trashing is local
///
/// Nothing here reaches the network. A published post that is trashed is still
/// on the blog, and stays there until somebody unpublishes it deliberately —
/// which is why the trash view says so in words rather than leaving it to be
/// discovered. See [`crate::db::mirror_posts`] for the other half of that: a
/// trashed post drops out of the conversation with the cloud entirely, so a
/// refresh neither overwrites it nor deletes it for being absent upstream.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "post_trash")]
pub struct Model {
    /// The `blog-db` post id this row hides.
    #[sea_orm(primary_key, auto_increment = false)]
    pub post_id: i32,
    /// When it was trashed (Unix seconds) — what the trash view sorts by.
    pub trashed_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
