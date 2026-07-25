use sea_orm::entity::prelude::*;
use sea_orm::Set;
use serde::{Deserialize, Serialize};

/// A blog post's metadata row, shared by the local SQLite cache and Cloudflare
/// D1. Every column is TEXT so the same values map cleanly onto D1's HTTP query
/// params. Dates are RFC 3339 strings; `r2_key` is the R2 object key of the
/// post's `.md` file.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "posts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub title: String,
    pub status: String,
    pub tags: String,
    pub r2_key: String,
    pub upload_date: String,
    pub last_updated_date: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl Model {
    /// An `ActiveModel` with every column marked `Set` — for inserts and full
    /// upserts where all values are written.
    pub fn into_active_set(self) -> ActiveModel {
        ActiveModel {
            id: Set(self.id),
            title: Set(self.title),
            status: Set(self.status),
            tags: Set(self.tags),
            r2_key: Set(self.r2_key),
            upload_date: Set(self.upload_date),
            last_updated_date: Set(self.last_updated_date),
        }
    }
}
