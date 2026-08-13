use sea_orm::entity::prelude::*;
use sea_orm::Set;
use serde::{Deserialize, Deserializer, Serialize};

/// A blog post. Stored in the `blog-db` table (name taken verbatim from the
/// Drizzle schema).
///
/// Storage conventions mirror Drizzle so the same D1 database interoperates with
/// the web app:
/// - `created_at` / `updated_at` / `published_at` are Unix timestamps in seconds.
/// - `tags` is a JSON-encoded `string[]` (e.g. `["rust","tauri"]`), nullable.
/// - `published` is stored as an integer `0`/`1`.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "blog-db")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(unique)]
    pub slug: String,
    pub title: String,
    pub excerpt: Option<String>,
    pub tags: Option<String>,
    #[serde(deserialize_with = "de_flexible_bool")]
    pub published: bool,
    pub published_at: Option<i64>,
    pub series_id: Option<i32>,
    pub series_order: Option<i32>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter)]
pub enum Relation {
    Series,
}

impl RelationTrait for Relation {
    fn def(&self) -> RelationDef {
        match self {
            Relation::Series => Entity::belongs_to(super::series::Entity)
                .from(Column::SeriesId)
                .to(super::series::Column::Id)
                .into(),
        }
    }
}

impl Related<super::series::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Series.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

impl crate::entities::record::Record for Model {
    type Entity = Entity;

    fn order_column() -> Column {
        Column::CreatedAt
    }

    /// `ActiveModel` for an insert — the primary key is auto-assigned.
    fn into_insert(self) -> ActiveModel {
        ActiveModel {
            id: sea_orm::ActiveValue::NotSet,
            slug: Set(self.slug),
            title: Set(self.title),
            excerpt: Set(self.excerpt),
            tags: Set(self.tags),
            published: Set(self.published),
            published_at: Set(self.published_at),
            series_id: Set(self.series_id),
            series_order: Set(self.series_order),
            created_at: Set(self.created_at),
            updated_at: Set(self.updated_at),
        }
    }

    /// `ActiveModel` for an update — locate by the (unchanged) id, write the rest.
    fn into_update(self) -> ActiveModel {
        ActiveModel {
            id: sea_orm::ActiveValue::Unchanged(self.id),
            slug: Set(self.slug),
            title: Set(self.title),
            excerpt: Set(self.excerpt),
            tags: Set(self.tags),
            published: Set(self.published),
            published_at: Set(self.published_at),
            series_id: Set(self.series_id),
            series_order: Set(self.series_order),
            created_at: Set(self.created_at),
            updated_at: Set(self.updated_at),
        }
    }
}

/// Accept `published` as a JSON bool or as an integer — D1 returns the stored
/// `0`/`1` as a number, while the frontend sends a real boolean.
fn de_flexible_bool<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum BoolOrInt {
        Bool(bool),
        Int(i64),
    }
    Ok(match BoolOrInt::deserialize(deserializer)? {
        BoolOrInt::Bool(b) => b,
        BoolOrInt::Int(i) => i != 0,
    })
}
