use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// The two editorial publish stages a post can be staged in.
pub const DRAFT: &str = "draft";
pub const PUBLISHED: &str = "published";

/// Local-only staging table tracking each post's editorial publish stage
/// (`"draft"` | `"published"`), keyed by the post's id.
///
/// This table is **not** synced to D1 — it's the local workflow state. A publish
/// action promotes the stage here and writes the post's `published` field
/// through to Cloudflare D1 (see `commands::publish_post`).
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "post_stage")]
pub struct Model {
    /// The `blog-db` post id this stage belongs to.
    #[sea_orm(primary_key, auto_increment = false)]
    pub post_id: i32,
    /// `"draft"` or `"published"`.
    pub stage: String,
    /// When the stage was last set (Unix seconds).
    pub staged_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
