use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Local-only record of how a post's content compares with what the cloud last
/// accepted, keyed by the post's id.
///
/// Kept apart from `post_stage` on purpose. A stage is an *editorial* decision —
/// draft or published, made by a person — whereas this is a *synchronisation*
/// fact about whether the bytes here match the bytes out there. Folding the two
/// into one column is what left a post reading plainly `Published` while
/// carrying edits no reader had seen.
///
/// Like `post_stage`, this table has no D1 counterpart: it describes this
/// machine's relationship with the cloud, which is nobody else's business.
///
/// The columns are deliberately a superset of what [`crate::sync_state`] needs
/// today. Conflict detection wants to know about the *remote* side as well —
/// when it last changed, and whether it changed since we last looked — and that
/// belongs in this row next to its local counterpart rather than in a second
/// table describing the same relationship.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "post_sync")]
pub struct Model {
    /// The `blog-db` post id this record belongs to.
    #[sea_orm(primary_key, auto_increment = false)]
    pub post_id: i32,
    /// Fingerprint of the post's content as it stands on this machine.
    pub local_hash: String,
    /// Fingerprint of the content the cloud last accepted, or `None` for a post
    /// that has never been pushed. Equal to `local_hash` exactly when there is
    /// nothing local waiting to go up.
    pub synced_hash: Option<String>,
    /// When that push completed (Unix seconds).
    pub synced_at: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
