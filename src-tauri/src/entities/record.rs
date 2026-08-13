//! What a model must offer for the generic CRUD helpers to work on it.
//!
//! `post` and `series` had five near-identical persistence functions each, in
//! both `db.rs` and `cloudflare.rs`, differing only in the entity they named.
//! They could not be written once because `into_insert` and `into_update` were
//! *inherent* methods on each `Model`, and an inherent method cannot be reached
//! through a generic parameter.
//!
//! Moving them onto a trait removes that obstacle: [`db`](crate::db) and
//! [`cloudflare`](crate::cloudflare) now carry one implementation of each
//! operation, and adding a third entity means writing this impl instead of
//! another ten functions.

use sea_orm::{EntityTrait, PrimaryKeyTrait};

/// A model that can be persisted by the generic CRUD helpers.
///
/// Implemented on the `Model` rather than the `Entity` so callers name the type
/// they already hold — `db::list::<post::Model>(db)` rather than threading the
/// entity type through separately.
pub trait Record: Sized + Send {
    /// The entity this model belongs to.
    type Entity: EntityTrait<Model = Self>;

    /// The column rows are listed by, newest first.
    fn order_column() -> <Self::Entity as EntityTrait>::Column;

    /// `ActiveModel` for an insert — the primary key is auto-assigned.
    fn into_insert(self) -> <Self::Entity as EntityTrait>::ActiveModel;

    /// `ActiveModel` for an update — locate by the (unchanged) id, write the rest.
    fn into_update(self) -> <Self::Entity as EntityTrait>::ActiveModel;
}

/// The primary-key value type for a record's entity, as accepted by
/// `find_by_id` and `delete_by_id`.
pub type Id<M> =
    <<<M as Record>::Entity as EntityTrait>::PrimaryKey as PrimaryKeyTrait>::ValueType;
