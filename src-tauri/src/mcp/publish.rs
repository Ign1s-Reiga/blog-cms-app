//! The human-in-the-loop gate for publishes requested over MCP.
//!
//! An MCP client may ask for a post to go live, but nothing reaches R2 or D1
//! until someone approves the request in the app. A request is therefore only
//! ever a *record of intent*: [`enqueue`] stores it, the Settings screen lists
//! it, and `mcp_approve_publish` is what actually runs the publish.
//!
//! The queue is deliberately in-memory. An unapproved request should not
//! survive a restart — the person who would have approved it is no longer
//! looking at the session that produced it, and a stale request approved days
//! later would publish a body nobody re-read.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use schemars::JsonSchema;
use serde::Serialize;

use crate::error::{AppError, AppResult};

/// Where a publish request stands.
///
/// The lifecycle is one way through:
///
/// ```text
/// AwaitingApproval ─┬─> Publishing ─┬─> Published
///                   │               └─> Failed
///                   └─> Rejected
/// ```
///
/// [`PublishState::Publishing`] exists so that claiming a request and running
/// it are not two separately-observable steps — see [`claim_for_publish`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PublishState {
    /// Waiting for someone to approve or reject it in the app.
    AwaitingApproval,
    /// Approved, and the publish is running right now. Nobody else may claim or
    /// reject it from here; the only ways out are `Published` and `Failed`.
    Publishing,
    /// A human declined it. The post stays as it was.
    Rejected,
    /// Approved, and the publish succeeded.
    Published,
    /// Approved, but the publish itself failed — see `error`.
    Failed,
}

impl PublishState {
    /// Whether this request is finished with — nothing more will happen to it.
    ///
    /// `Publishing` is deliberately *not* terminal: the request is still in
    /// flight, so it must keep blocking duplicates for the same post the way an
    /// unapproved one does.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Rejected | Self::Published | Self::Failed)
    }
}

/// One request to publish a post, from the moment an MCP client asks until a
/// human resolves it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct PublishRequest {
    /// Opaque id the MCP client polls with.
    pub id: String,
    pub post_id: i32,
    /// Captured when the request was made, so the approval UI can name the post
    /// without a database round-trip.
    pub slug: String,
    pub title: String,
    /// The justification the MCP client gave, shown to whoever decides.
    pub reason: Option<String>,
    pub requested_at: i64,
    pub state: PublishState,
    /// Why the publish failed, when `state` is `failed`.
    pub error: Option<String>,
}

/// Requests keyed by id.
///
/// A `std::sync::Mutex` is right here because every access is a brief map
/// operation; the guard is never held across an `.await` (the publish itself
/// runs after [`claim_for_publish`] has returned and released it, which is why
/// the claim has to leave a mark behind).
static QUEUE: OnceLock<Mutex<HashMap<String, PublishRequest>>> = OnceLock::new();

fn queue() -> &'static Mutex<HashMap<String, PublishRequest>> {
    QUEUE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A poisoned lock means another thread panicked mid-update. The map is a plain
/// collection with no cross-entry invariant, so recovering the guard is safe and
/// keeps one panic from disabling approvals for the rest of the session.
fn lock() -> std::sync::MutexGuard<'static, HashMap<String, PublishRequest>> {
    queue().lock().unwrap_or_else(|e| e.into_inner())
}

/// Record a new request and return it. The caller is expected to notify the
/// frontend so the approval shows up without a refresh.
pub fn enqueue(post_id: i32, slug: String, title: String, reason: Option<String>) -> PublishRequest {
    let request = PublishRequest {
        id: uuid::Uuid::new_v4().simple().to_string(),
        post_id,
        slug,
        title,
        reason: reason.filter(|r| !r.trim().is_empty()),
        requested_at: chrono::Utc::now().timestamp(),
        state: PublishState::AwaitingApproval,
        error: None,
    };
    lock().insert(request.id.clone(), request.clone());
    request
}

/// Look up a request by id.
pub fn get(id: &str) -> Option<PublishRequest> {
    lock().get(id).cloned()
}

/// Every request this session, newest first — what the Settings screen renders.
pub fn list() -> Vec<PublishRequest> {
    let mut all: Vec<PublishRequest> = lock().values().cloned().collect();
    all.sort_by(|a, b| b.requested_at.cmp(&a.requested_at).then(b.id.cmp(&a.id)));
    all
}

/// Whether a post already has a request that hasn't finished — either waiting on
/// a human or mid-publish.
///
/// Without this, an agent that polls impatiently could queue the same post a
/// dozen times and bury the approval list.
pub fn open_for_post(post_id: i32) -> Option<PublishRequest> {
    lock()
        .values()
        .find(|r| r.post_id == post_id && !r.state.is_terminal())
        .cloned()
}

/// Claim an approved request for execution, moving it to
/// [`PublishState::Publishing`]. The caller then runs the publish and reports
/// back through [`settle`].
///
/// The check and the mark happen under one lock, which is the whole point: a
/// claim that only *read* the state would let two approvals arriving together
/// both see `AwaitingApproval` and both publish, because the request stays
/// unclaimed across every `.await` in `mcp_approve_publish` until `settle` runs.
/// Claiming it here closes that window to nothing, so the second caller is
/// turned away.
///
/// The request stays in the map — removing it would lose the record if the
/// publish then failed.
pub fn claim_for_publish(id: &str) -> AppResult<PublishRequest> {
    let mut guard = lock();
    let request = guard
        .get_mut(id)
        .ok_or_else(|| AppError::NoPublishRequest(id.to_string()))?;
    match request.state {
        PublishState::AwaitingApproval => {
            request.state = PublishState::Publishing;
            Ok(request.clone())
        }
        state => Err(AppError::PublishRequestNotPending { id: id.to_string(), state }),
    }
}

/// Reject a pending request. Returns the updated record.
///
/// Only from `AwaitingApproval`: a request already being published is past the
/// point where declining it would stop anything, and pretending otherwise would
/// leave a `Rejected` request that had in fact gone live.
pub fn reject(id: &str) -> AppResult<PublishRequest> {
    let mut guard = lock();
    let request = guard
        .get_mut(id)
        .ok_or_else(|| AppError::NoPublishRequest(id.to_string()))?;
    if request.state != PublishState::AwaitingApproval {
        return Err(AppError::PublishRequestNotPending {
            id: id.to_string(),
            state: request.state,
        });
    }
    request.state = PublishState::Rejected;
    Ok(request.clone())
}

/// Record how an approved publish turned out, ending the `Publishing` state
/// that [`claim_for_publish`] began. Only ever reached with a claim in hand.
pub fn settle(id: &str, outcome: Result<(), String>) -> Option<PublishRequest> {
    let mut guard = lock();
    let request = guard.get_mut(id)?;
    match outcome {
        Ok(()) => {
            request.state = PublishState::Published;
            request.error = None;
        }
        Err(e) => {
            request.state = PublishState::Failed;
            request.error = Some(e);
        }
    }
    Some(request.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ids must be unique, or one request's outcome would overwrite another's.
    #[test]
    fn enqueued_requests_get_distinct_ids() {
        let a = enqueue(1, "a".into(), "A".into(), None);
        let b = enqueue(1, "a".into(), "A".into(), None);
        assert_ne!(a.id, b.id);
        assert_eq!(a.state, PublishState::AwaitingApproval);
    }

    /// The guard that makes a double-click harmless: only the first claim wins.
    #[test]
    fn a_request_can_only_be_settled_once() {
        let req = enqueue(2, "b".into(), "B".into(), Some("  ".into()));
        // A blank reason is not a reason.
        assert_eq!(req.reason, None);

        assert!(claim_for_publish(&req.id).is_ok());
        settle(&req.id, Ok(()));
        assert_eq!(get(&req.id).unwrap().state, PublishState::Published);

        // Already terminal, so neither path may re-enter it.
        assert!(claim_for_publish(&req.id).is_err());
        assert!(reject(&req.id).is_err());
    }

    /// The race this state exists for. `mcp_approve_publish` awaits the database,
    /// R2 and D1 between claiming a request and settling it, so a claim that only
    /// read the state would let a second approval arriving in that window publish
    /// the same post again.
    #[test]
    fn a_claim_cannot_be_made_twice_before_settlement() {
        let req = enqueue(6, "f".into(), "F".into(), None);

        assert!(claim_for_publish(&req.id).is_ok());
        assert_eq!(get(&req.id).unwrap().state, PublishState::Publishing);
        // *Before* settling — the whole window the old code left open.
        assert!(claim_for_publish(&req.id).is_err());

        settle(&req.id, Ok(()));
        assert_eq!(get(&req.id).unwrap().state, PublishState::Published);
    }

    /// Declining a publish that is already running would not stop it, and would
    /// leave a `Rejected` record for a post that had gone live.
    #[test]
    fn a_request_being_published_cannot_be_rejected() {
        let req = enqueue(7, "g".into(), "G".into(), None);
        claim_for_publish(&req.id).unwrap();

        assert!(reject(&req.id).is_err());
        assert_eq!(get(&req.id).unwrap().state, PublishState::Publishing);
    }

    /// A publish in flight is not finished, so it still has to keep an agent
    /// from queueing the same post again.
    #[test]
    fn a_publish_in_flight_still_blocks_a_duplicate_request() {
        let req = enqueue(8, "h".into(), "H".into(), None);
        claim_for_publish(&req.id).unwrap();
        assert_eq!(open_for_post(8).map(|r| r.state), Some(PublishState::Publishing));

        settle(&req.id, Ok(()));
        assert!(open_for_post(8).is_none());
    }

    #[test]
    fn rejection_is_terminal_and_records_no_error() {
        let req = enqueue(3, "c".into(), "C".into(), None);
        let rejected = reject(&req.id).unwrap();
        assert_eq!(rejected.state, PublishState::Rejected);
        assert_eq!(rejected.error, None);
        assert!(reject(&req.id).is_err());
    }

    #[test]
    fn a_failed_publish_keeps_the_reason() {
        let req = enqueue(4, "d".into(), "D".into(), None);
        claim_for_publish(&req.id).unwrap();
        settle(&req.id, Err("R2 unreachable".into()));
        let settled = get(&req.id).unwrap();
        assert_eq!(settled.state, PublishState::Failed);
        assert_eq!(settled.error.as_deref(), Some("R2 unreachable"));
    }

    /// Only unresolved requests block a new one for the same post.
    #[test]
    fn only_pending_requests_count_as_open() {
        let req = enqueue(5, "e".into(), "E".into(), None);
        assert!(open_for_post(5).is_some());
        reject(&req.id).unwrap();
        assert!(open_for_post(5).is_none());
    }

    #[test]
    fn unknown_ids_are_reported_rather_than_panicking() {
        assert!(get("nope").is_none());
        assert!(claim_for_publish("nope").is_err());
        assert!(reject("nope").is_err());
        assert!(settle("nope", Ok(())).is_none());
    }
}
