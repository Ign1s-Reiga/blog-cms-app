use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Local-only record that a post's cached Markdown is older than the cloud's,
/// keyed by slug.
///
/// A refresh writes metadata and never fetches bodies — they live in R2 and come
/// down per post — so when the cloud's copy has moved since this machine last
/// agreed with it, `<slug>.md` is an older version described by metadata saying
/// it is current. Nothing downstream could tell: the editor prefers the cache
/// and would open the stale text, and the media survey would read it as the
/// post's present references.
///
/// ## Why a row rather than just deleting the file
///
/// The obvious move is for the refresh to delete the cached body and be done.
/// But the deletion can fail — a file locked by an indexer, a permissions
/// change — and by then the metadata and the sync baseline have advanced, so
/// the next refresh sees the same remote timestamp and says nothing is stale.
/// The old Markdown would then sit there indefinitely, with everything treating
/// it as current and no mechanism left to notice.
///
/// So nothing is deleted. The row is written in the same transaction as the
/// metadata, so the fact survives whatever happens to the file, and every reader
/// of a cached body consults it. It is cleared when a fresh copy is fetched to
/// replace the old one, which is the moment the two agree again.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "post_body_stale")]
pub struct Model {
    /// The slug whose cached body can no longer be trusted.
    #[sea_orm(primary_key, auto_increment = false)]
    pub slug: String,
    /// When the refresh noticed (Unix seconds).
    pub since: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
