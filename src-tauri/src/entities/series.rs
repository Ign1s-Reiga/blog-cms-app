use sea_orm::entity::prelude::*;
use sea_orm::Set;
use serde::{Deserialize, Serialize};

/// A series groups related posts. Mirrors the Drizzle `series` table.
///
/// `created_at` is a Unix timestamp in **seconds** (Drizzle
/// `integer(mode: 'timestamp')`).
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "series")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(unique)]
    pub slug: String,
    pub title: String,
    pub description: Option<String>,
    pub created_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter)]
pub enum Relation {
    Post,
}

impl RelationTrait for Relation {
    fn def(&self) -> RelationDef {
        match self {
            Relation::Post => Entity::has_many(super::post::Entity).into(),
        }
    }
}

impl Related<super::post::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Post.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

impl Model {
    /// `ActiveModel` for an insert — the primary key is auto-assigned.
    pub fn into_insert(self) -> ActiveModel {
        ActiveModel {
            id: sea_orm::ActiveValue::NotSet,
            slug: Set(self.slug),
            title: Set(self.title),
            description: Set(self.description),
            created_at: Set(self.created_at),
        }
    }

    /// `ActiveModel` for an update — locate by the (unchanged) id, write the rest.
    pub fn into_update(self) -> ActiveModel {
        ActiveModel {
            id: sea_orm::ActiveValue::Unchanged(self.id),
            slug: Set(self.slug),
            title: Set(self.title),
            description: Set(self.description),
            created_at: Set(self.created_at),
        }
    }
}
