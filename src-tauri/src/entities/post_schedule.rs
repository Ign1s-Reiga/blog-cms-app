use sea_orm::entity::prelude::*;
use sea_orm::Set;
use serde::{Deserialize, Serialize};

/// A publication that has been asked for and has not happened yet.
pub const PENDING: &str = "pending";
/// Claimed by a Worker run that is carrying it out. Written only by the Worker,
/// and only ever briefly — a row that sits here is from a run that died, and a
/// later one takes it over. See `worker/README.md`.
pub const PUBLISHING: &str = "publishing";
/// The Worker ran it. The post is live and D1 says so.
pub const PUBLISHED: &str = "published";
/// The Worker tried and could not. `error` says what it hit, and the row stays
/// where it is — a schedule that failed silently would be indistinguishable from
/// one that never came due.
pub const FAILED: &str = "failed";
/// Called off before it came due.
pub const CANCELLED: &str = "cancelled";

/// How long a claim can sit in `publishing` before the app stops calling it
/// normal.
///
/// The Worker's own `STALE_CLAIM_SECONDS`, which is when it takes a claim over
/// from a run that died. Duplicated rather than shared because the two are
/// deployed separately and a mismatch is harmless in one direction only: too
/// long here just delays the warning, while too short would cry stuck at a run
/// that is still going. Kept equal so the app reports trouble at the moment the
/// Worker would already have retried it.
const STALE_CLAIM_SECONDS: i64 = 900;

/// A post's pending publication, keyed by slug.
///
/// ## Why this table exists, in both databases
///
/// The desktop app may well be closed when a post falls due, so the publication
/// has to be executed by something that is always running — a Worker on a cron
/// trigger. That means the schedule has to live where the Worker can read it,
/// which is D1.
///
/// It is deliberately **not** a column on `blog-db`. That table's shape belongs
/// to the blog's own schema in another repository, and `post::Model` *is* the
/// statement this app sends to it: a new column there would have to be migrated
/// by hand before this app could write anything at all. A table of its own is
/// created by the Worker's migration, read by the Worker, and written by the
/// app, without the blog's schema moving an inch.
///
/// ## Keyed by slug
///
/// Local ids and D1 ids are unrelated integers (see `db::SeriesMap` for the same
/// problem with series), so the identity the two sides agree on is the slug —
/// which is also what the Worker needs to name the post it is publishing.
///
/// The same row shape is kept locally, so the app can show what is scheduled
/// while offline. The local copy is a *mirror*: the cloud's is authoritative,
/// because that is the one the Worker acts on and writes back to.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "post_schedule")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub slug: String,
    /// When the post should go live (Unix seconds).
    pub publish_at: i64,
    /// One of the constants above.
    pub state: String,
    /// Why the last attempt failed, when it did.
    pub error: Option<String>,
    /// When this row last changed (Unix seconds).
    pub updated_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl crate::entities::record::Record for Model {
    type Entity = Entity;

    fn order_column() -> Column {
        Column::PublishAt
    }

    /// The primary key is the slug, so it is written rather than assigned.
    fn into_insert(self) -> ActiveModel {
        ActiveModel {
            slug: Set(self.slug),
            publish_at: Set(self.publish_at),
            state: Set(self.state),
            error: Set(self.error),
            updated_at: Set(self.updated_at),
        }
    }

    fn into_update(self) -> ActiveModel {
        ActiveModel {
            slug: sea_orm::ActiveValue::Unchanged(self.slug),
            publish_at: Set(self.publish_at),
            state: Set(self.state),
            error: Set(self.error),
            updated_at: Set(self.updated_at),
        }
    }
}

impl Model {
    /// Is this publication still going to happen?
    ///
    /// `publishing` counts. It is a claim held by a Worker run that is carrying
    /// the publication out, so a caller asking "is anything still going to put
    /// this post on the blog?" — which is what deleting a post needs to know —
    /// gets the same answer for both.
    pub fn is_in_flight(&self) -> bool {
        matches!(self.state.as_str(), PENDING | PUBLISHING)
    }

    /// How the schedule reads on screen, which is not quite what the column
    /// says.
    ///
    /// The difference is `overdue`: a row still `pending` after its time has
    /// passed means the Worker has not run it — a cron that is not deployed, an
    /// account that is out of credit, a migration never applied. Nothing writes
    /// that state, because nothing is there to write it; it can only be noticed
    /// by comparing the row against the clock, which is why it is derived here
    /// rather than stored.
    ///
    /// Derived in Rust rather than in the frontend so the desktop and anything
    /// else reading this agree on what a row means.
    pub fn display_state(&self, now: i64) -> &'static str {
        match self.state.as_str() {
            PENDING if self.publish_at <= now => "overdue",
            PENDING => "scheduled",
            // A claim that has outlasted any run that could still be holding
            // it. The Worker retries these, so seeing one means the retry is
            // not coming either — the same "nothing is running this" that
            // `overdue` reports for a pending row, and just as invisible if it
            // is reported as normal.
            PUBLISHING if self.updated_at + STALE_CLAIM_SECONDS <= now => "overdue",
            // Someone is on it right now. Reported as still scheduled rather
            // than as a state of its own: it lasts a moment, and a badge that
            // flickers through it teaches nobody anything.
            PUBLISHING => "scheduled",
            PUBLISHED => "published",
            FAILED => "failed",
            CANCELLED => "cancelled",
            // A state this build does not know about. Named rather than
            // guessed: a newer Worker writing something new should not have its
            // rows quietly displayed as one of these.
            _ => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(state: &str, publish_at: i64) -> Model {
        Model {
            slug: "a-post".into(),
            publish_at,
            state: state.into(),
            error: None,
            updated_at: 0,
        }
    }

    /// The state that nothing writes down: still pending after its time is the
    /// only evidence that the Worker did not run.
    #[test]
    fn a_pending_schedule_past_its_time_reads_as_overdue() {
        assert_eq!(row(PENDING, 2_000).display_state(1_000), "scheduled");
        assert_eq!(row(PENDING, 1_000).display_state(2_000), "overdue");
        // Exactly due counts as overdue: the Worker's cron has fired by then.
        assert_eq!(row(PENDING, 1_000).display_state(1_000), "overdue");
    }

    /// What "still going to happen" covers, and what it does not. Deleting a
    /// post asks this question, and a claim held by a Worker mid-run is every
    /// bit as much a publication on its way as an unclaimed one.
    #[test]
    fn a_claim_counts_as_still_going_to_happen() {
        assert!(row(PENDING, 1_000).is_in_flight());
        assert!(row(PUBLISHING, 1_000).is_in_flight());

        assert!(!row(PUBLISHED, 1_000).is_in_flight());
        assert!(!row(FAILED, 1_000).is_in_flight());
        assert!(!row(CANCELLED, 1_000).is_in_flight());
    }

    /// A claim is normal while a run could still be holding it, and evidence of
    /// a dead run once it could not. Reported the same way as a pending row
    /// nobody ran, because it is the same problem from the other side.
    #[test]
    fn a_claim_nobody_came_back_from_reads_as_overdue() {
        let mut claimed = row(PUBLISHING, 1_000);
        claimed.updated_at = 1_000;

        assert_eq!(claimed.display_state(1_100), "scheduled");
        // Right up to the Worker's own retry threshold.
        assert_eq!(claimed.display_state(1_000 + 899), "scheduled");
        assert_eq!(claimed.display_state(1_000 + 900), "overdue");
    }

    /// Everything the Worker writes is reported as it wrote it, and the clock
    /// has no say in it.
    #[test]
    fn settled_states_are_reported_as_written() {
        for (state, expected) in [
            (PUBLISHED, "published"),
            (FAILED, "failed"),
            (CANCELLED, "cancelled"),
        ] {
            assert_eq!(row(state, 1_000).display_state(9_999), expected);
        }
        assert_eq!(row("something-new", 1_000).display_state(9_999), "unknown");
    }
}
